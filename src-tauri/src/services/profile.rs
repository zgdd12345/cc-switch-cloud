//! Profile 服务层（increment 3a）
//!
//! ProfileService 负责把一个 Profile 的内容规格（skills/commands/agents/mcp + provider）
//! 应用到对应应用的运行态：
//!
//! 1. 复用 ProviderService::switch 切换 provider（若 profile 固定了 provider）。
//! 2. 将 4 个启用面（commands / agents / skills / mcp）**完全覆盖**为 profile.spec 指定的集合。
//! 3. 每类协调器（reconciler）**无条件**在最后运行一次，把 DB 状态物化到磁盘。
//! 4. 最后设置 active profile，从而中途失败时不会破坏先前的 active profile。
//!
//! 3a 范围内**不**做 manifest / dotfile 渲染 / `${}` 模板 / `@tag`（延后到 3b）。

use std::collections::HashSet;

use crate::app_config::AppType;
use crate::error::AppError;
use crate::services::agent::AgentService;
use crate::services::command::CommandService;
use crate::services::profile_render::{
    remove_whole_file_if_owned, render_whole_file, validate_rel_path,
};
use crate::services::prompt::PromptService;
use crate::services::provider::{write_live_with_common_config, ProviderService};
use crate::services::{McpService, SkillService};
use crate::store::AppState;

/// activate 的结果：携带非致命警告。
#[derive(Debug, serde::Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ActivateResult {
    pub warnings: Vec<String>,
}

/// Profile 业务逻辑服务（无状态单元结构，镜像 ProviderService / McpService）。
pub struct ProfileService;

impl ProfileService {
    /// 激活指定 Profile。
    ///
    /// 不变量（3a + 3b-1）：
    /// - 步骤 4 的 reconciler 永远在最后无条件运行；
    /// - 步骤 7 的 set_active_profile 在 reconcile 成功之后才执行，因此中途失败会保留
    ///   先前的 active profile；
    /// - whole-file dotfile（statusline.sh 等）与 manifest 生命周期：先记录 INCOMING、
    ///   再移除 OUTGOING 不再需要的、最后基于「现在已是 incoming」重建 settings.json；
    /// - 绝不删除用户亲手放置/编辑的文件（hash 不匹配则跳过 + warn）。
    pub fn activate(
        state: &AppState,
        app_type: AppType,
        profile_id: &str,
    ) -> Result<ActivateResult, AppError> {
        let mut result = ActivateResult::default();

        // ---- 1. 加载 profile 并校验 app_type 一致 ----
        let profile = state
            .db
            .get_profile(profile_id)?
            .ok_or_else(|| AppError::Message("profile not found".into()))?;
        if profile.app_type != app_type.as_str() {
            return Err(AppError::Message(format!(
                "profile app_type mismatch: profile is {}, requested {}",
                profile.app_type,
                app_type.as_str()
            )));
        }

        // 捕获 OUTGOING（在任何 active 变化之前）。用于步骤 6 的旧文件回收。
        let outgoing = state.db.get_active_profile(app_type.as_str())?;

        // ---- 2. Provider 切换（复用 ProviderService::switch）----
        if let Some(pid) = profile.current_provider_id.as_deref() {
            if state
                .db
                .get_provider_by_id(pid, app_type.as_str())?
                .is_none()
            {
                result
                    .warnings
                    .push(format!("provider missing, skipped switch: {pid}"));
            } else {
                let sr = ProviderService::switch(state, app_type.clone(), pid)?;
                result.warnings.extend(sr.warnings);
            }
        }

        // ---- 3. 把 4 个启用面完全覆盖为 spec ----
        let content = &profile.spec.content;

        // 3a. commands（键 = command.name）
        let want_commands: HashSet<&str> = content.commands.iter().map(|s| s.as_str()).collect();
        let mut matched_commands: HashSet<String> = HashSet::new();
        for cmd in state.db.get_all_installed_commands()? {
            let want = want_commands.contains(cmd.name.as_str());
            state.db.set_command_enabled(&cmd.id, want)?;
            if want {
                matched_commands.insert(cmd.name.clone());
            }
        }
        for name in &content.commands {
            if !matched_commands.contains(name) {
                result.warnings.push(format!("command not found: {name}"));
            }
        }

        // 3b. agents（键 = agent.name）
        let want_agents: HashSet<&str> = content.agents.iter().map(|s| s.as_str()).collect();
        let mut matched_agents: HashSet<String> = HashSet::new();
        for agent in state.db.get_all_installed_agents()? {
            let want = want_agents.contains(agent.name.as_str());
            state.db.set_agent_enabled(&agent.id, want)?;
            if want {
                matched_agents.insert(agent.name.clone());
            }
        }
        for name in &content.agents {
            if !matched_agents.contains(name) {
                result.warnings.push(format!("agent not found: {name}"));
            }
        }

        // 3c. skills（per-app，键 = skill.directory；只触碰本 app_type 标志位）
        let want_skills: HashSet<&str> = content.skills.iter().map(|s| s.as_str()).collect();
        let mut matched_skills: HashSet<String> = HashSet::new();
        for skill in state.db.get_all_installed_skills()?.values() {
            let want = want_skills.contains(skill.directory.as_str());
            let mut apps = skill.apps.clone();
            apps.set_enabled_for(&app_type, want);
            state.db.update_skill_apps(&skill.id, &apps)?;
            if want {
                matched_skills.insert(skill.directory.clone());
            }
        }
        for dir in &content.skills {
            if !matched_skills.contains(dir) {
                result.warnings.push(format!("skill not found: {dir}"));
            }
        }

        // 3d. mcp（per-app，键 = server id；只触碰本 app_type 标志位）
        let want_mcp: HashSet<&str> = content.mcp.iter().map(|s| s.as_str()).collect();
        let mut matched_mcp: HashSet<String> = HashSet::new();
        let mut servers = state.db.get_all_mcp_servers()?;
        for server in servers.values_mut() {
            let want = want_mcp.contains(server.id.as_str());
            server.apps.set_enabled_for(&app_type, want);
            state.db.save_mcp_server(server)?;
            if want {
                matched_mcp.insert(server.id.clone());
            }
        }
        for id in &content.mcp {
            if !matched_mcp.contains(id) {
                result.warnings.push(format!("mcp not found: {id}"));
            }
        }

        // ---- 4. 协调器：每类一次，无条件，最后运行 ----
        // 注意：MCP 必须在此再次 sync（即使 step 2 的 provider switch 可能已 sync 过），
        // 因为 step 3 之后才改了 MCP 标志位；sync_all_enabled 是幂等的。
        SkillService::sync_to_app(&state.db, &app_type)
            .map_err(|e| AppError::Message(format!("skill sync_to_app failed: {e}")))?;
        CommandService::new(state.db.clone()).reconcile()?;
        AgentService::new(state.db.clone()).reconcile()?;
        McpService::sync_all_enabled(state)?;

        // ---- 5. WHOLE-FILE dotfiles（INCOMING）----
        // settings.json 由步骤 8 的确定性重建负责（第三层 deep-merge），CLAUDE.md 延后到 3b-2，
        // 故此处仅处理其余整文件 dotfile（例如 statusline.sh）。
        //
        // 为支持「重复激活」幂等：先清除 INCOMING 已有的 whole_file manifest 行，
        // 再依据当前 spec 重新记录 —— 避免每次激活都堆积陈旧行。其他 kind 的行不动。
        //
        // 但在清除之前必须**快照**这些行的 (target_path -> content_hash)，以便重新渲染时
        // 把旧行的 hash 作为 prior_owned_hash 传给 render_whole_file。否则同一 profile 的
        // 重复激活会出现：磁盘上已存在我们自己写的文件，但 prior_owned_hash=None =>
        // owned=false => render_whole_file 跳过(Ok(None)) + 误报「被用户编辑」警告，且行已被
        // 删除而再也不会重新记录（manifest 行丢失），破坏后续 OUTGOING 移除与 deactivate 拆除保证。
        let prior_owned_hashes: std::collections::HashMap<String, Option<String>> = state
            .db
            .get_manifest_for_profile(profile_id, app_type.as_str())?
            .into_iter()
            .filter(|r| r.kind == "whole_file")
            .map(|r| (r.target_path.clone(), r.content_hash.clone()))
            .collect();
        let incoming_whole_file_ids: Vec<i64> = state
            .db
            .get_manifest_for_profile(profile_id, app_type.as_str())?
            .into_iter()
            .filter(|r| r.kind == "whole_file")
            .map(|r| r.id)
            .collect();
        state.db.delete_manifest_entries(&incoming_whole_file_ids)?;

        let mut written_targets: HashSet<String> = HashSet::new();
        for df in state.db.get_profile_dotfiles(profile_id)? {
            if df.rel_path == "settings.json" || df.rel_path == "CLAUDE.md" {
                continue;
            }
            // 把该 dotfile 解析到的绝对 target 与快照里的旧行匹配，取出 prior_owned_hash。
            // 路径非法时回退到 None（render_whole_file 会再次校验并返回 Err/skip）。
            let prior_owned_hash: Option<String> = validate_rel_path(&df.rel_path)
                .ok()
                .and_then(|p| {
                    prior_owned_hashes
                        .get(&p.to_string_lossy().to_string())
                        .cloned()
                })
                .flatten();
            // 3b-2: 在写盘前渲染整文件 dotfile（如 statusline.sh）中的 `${VAR}`（shell
            // 文本，不做 JSON 转义），warnings 透出到 result.warnings。`profile` 是步骤 1
            // 已加载的 INCOMING profile。content_hash 现在基于「已渲染」字节计算 —— 归属
            // 检测仍成立，因为变量不变时重复激活会渲染出相同字节。CLAUDE.md 仍被跳过（3b-3）。
            let rendered = crate::services::profile_vars::render_with_profile_vars(
                state.db.as_ref(),
                &app_type,
                &profile,
                &df.content,
                /*json_escape=*/ false,
                &mut result.warnings,
            )?;
            match render_whole_file(
                state.db.as_ref(),
                profile_id,
                &app_type,
                &df.rel_path,
                &rendered,
                prior_owned_hash.as_deref(),
            ) {
                Ok(Some(entry)) => {
                    written_targets.insert(entry.target_path.clone());
                    state.db.record_manifest_entry(&entry)?;
                }
                Ok(None) => {
                    result.warnings.push(format!(
                        "dotfile skipped (unmanaged/user-edited): {}",
                        df.rel_path
                    ));
                }
                Err(e) => {
                    result
                        .warnings
                        .push(format!("dotfile render failed for {}: {e}", df.rel_path));
                }
            }
        }

        // ---- 6. 移除 OUTGOING 不再需要的 whole-file ----
        // 仅当存在 OUTGOING 且与 INCOMING 不同：对 OUTGOING 的每条 whole_file manifest 行，
        // 若其 target 未被本次写入（不在 written_targets 中），则按归属删除磁盘文件
        // （hash 匹配才删，否则跳过 + warn），随后删除对应 manifest 行。
        if let Some(out) = &outgoing {
            if out.id != profile_id {
                let mut stale_ids: Vec<i64> = Vec::new();
                for row in state
                    .db
                    .get_manifest_for_profile(&out.id, app_type.as_str())?
                {
                    if row.kind != "whole_file" || written_targets.contains(&row.target_path) {
                        continue;
                    }
                    let removed =
                        remove_whole_file_if_owned(&row.target_path, row.content_hash.as_deref())?;
                    if !removed {
                        result.warnings.push(format!(
                            "outgoing dotfile kept (unmanaged/user-edited): {}",
                            row.target_path
                        ));
                    }
                    stale_ids.push(row.id);
                }
                state.db.delete_manifest_entries(&stale_ids)?;
            }
        }

        // ---- 7. 设置 active profile（倒数第二步）----
        state.db.set_active_profile(app_type.as_str(), profile_id)?;

        // ---- 5b. CLAUDE.md 仲裁（3b-3，HARD-GATED 到 AppType::Claude）----
        // CLAUDE.md 是 Claude 独有的全局提示词文件，且**字面量**（不渲染 `${VAR}`）。
        // 它经由一条隐藏的 `__profile__:<id>` prompt 行接入既有的「单启用」prompt 体系：
        // - 启用唯一只能走 enable_prompt（执行 single-enabled sweep + 跳过隐藏行回填）；
        // - 绝不通过 upsert_prompt(enabled=true) 启用隐藏行（upsert 不 sweep）。
        // 放在步骤 7（set_active）之后、步骤 8（settings 重建）之前，与 set_active 同处倒数。
        if matches!(app_type, AppType::Claude) {
            let hidden_id = format!("__profile__:{profile_id}");
            match state.db.get_profile_dotfile(profile_id, "CLAUDE.md")? {
                Some(df) if !df.content.trim().is_empty() => {
                    // 用**字面量**内容（NO ${VAR} 渲染）构建/更新隐藏行；enabled 由随后的
                    // enable_prompt 翻转（save_prompt 仅持久化，enabled=false）。
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    let row = crate::prompt::Prompt {
                        id: hidden_id.clone(),
                        name: format!("(profile) {}", profile.name),
                        content: df.content.clone(),
                        description: Some(
                            "AgentHub profile-managed CLAUDE.md (hidden)".to_string(),
                        ),
                        enabled: false,
                        hidden: true,
                        created_at: Some(now),
                        updated_at: Some(now),
                    };
                    state.db.save_prompt("claude", &row)?;
                    PromptService::enable_prompt(state, AppType::Claude, &hidden_id)?;

                    // 仲裁后 warn 检查：enable_prompt 应已让本 profile 的隐藏行成为唯一
                    // 启用项。若不是（理论上不应发生），透出非致命警告而非静默失败。
                    let enabled_now = state
                        .db
                        .get_prompts_with_hidden("claude")?
                        .into_iter()
                        .find(|(_, p)| p.enabled)
                        .map(|(id, _)| id);
                    if enabled_now.as_deref() != Some(hidden_id.as_str()) {
                        result.warnings.push(format!(
                            "CLAUDE.md not owned by profile: active prompt is {}",
                            enabled_now.as_deref().unwrap_or("<none>")
                        ));
                    }
                }
                // None 或空内容 -> 确保隐藏行不再是 CLAUDE.md 的活跃写入者。
                _ => {
                    if let Some(mut row) = state.db.get_prompt_with_hidden("claude", &hidden_id)? {
                        if row.enabled {
                            row.enabled = false;
                            PromptService::upsert_prompt(state, AppType::Claude, &hidden_id, row)?;
                        }
                    }
                }
            }
        }

        // ---- 8. 确定性重建 settings.json ----
        // active 现已是 INCOMING，故 build_effective_settings_with_common_config（T3）
        // 会把 INCOMING 的 settings.json 片段 deep-merge 进去；此写入是幂等且权威的。
        match Self::current_provider(state, &app_type)? {
            Some(p) => write_live_with_common_config(state.db.as_ref(), &app_type, &p)?,
            None => result
                .warnings
                .push("no current provider; profile settings.json fragment not applied".into()),
        }

        Ok(result)
    }

    /// 取消激活指定 app_type 的当前 Profile（3b 确定性 teardown）。
    ///
    /// 3a 仅清除 DB active 标志、不拆除磁盘（no-teardown）。3b 改为：
    /// 1. 读取并清除 active；
    /// 2. 对原 active 的每条 whole_file manifest 行，按归属删除磁盘文件
    ///    （hash 匹配才删，否则跳过；绝不删用户编辑过的文件），随后清空其 manifest；
    /// 3. 以「无 profile 层」确定性重建 settings.json（active 已清除 -> T3 不再 merge 片段，
    ///    settings.json = provider + common config）。
    ///
    /// 该路径不依赖任何 diff-removal，因此无 3a 切换时的 diff 移除隐患。
    pub fn deactivate(state: &AppState, app_type: AppType) -> Result<ActivateResult, AppError> {
        let mut result = ActivateResult::default();

        // ---- 1. 读取并清除 active ----
        let cur = state.db.get_active_profile(app_type.as_str())?;
        state.db.clear_active_profile(app_type.as_str())?;

        // ---- 2. 拆除原 active 的 whole-file dotfiles ----
        if let Some(c) = &cur {
            for row in state
                .db
                .get_manifest_for_profile(&c.id, app_type.as_str())?
            {
                if row.kind != "whole_file" {
                    continue;
                }
                let removed =
                    remove_whole_file_if_owned(&row.target_path, row.content_hash.as_deref())?;
                if !removed {
                    result.warnings.push(format!(
                        "dotfile kept (unmanaged/user-edited): {}",
                        row.target_path
                    ));
                }
            }
            state
                .db
                .clear_manifest_for_profile(&c.id, app_type.as_str())?;

            // 2b. CLAUDE.md 拆除（3b-3，HARD-GATED 到 claude profile）：若原 active 是
            // claude profile 且其隐藏行仍是 CLAUDE.md 的活跃写入者，则禁用之。
            // upsert_prompt(enabled=false) 内部依 T3 any_enabled 决定是否清空文件
            // （无其它启用 prompt -> 清空；仍有可见 prompt 启用 -> 保留）。
            if c.app_type == AppType::Claude.as_str() {
                let hidden_id = format!("__profile__:{}", c.id);
                if let Some(mut row) = state.db.get_prompt_with_hidden("claude", &hidden_id)? {
                    if row.enabled {
                        row.enabled = false;
                        PromptService::upsert_prompt(state, AppType::Claude, &hidden_id, row)?;
                    }
                }
            }
        }

        // ---- 3. 无 profile 层确定性重建 settings.json ----
        match Self::current_provider(state, &app_type)? {
            Some(p) => write_live_with_common_config(state.db.as_ref(), &app_type, &p)?,
            None => result
                .warnings
                .push("no current provider; settings.json not rebuilt on deactivate".into()),
        }

        Ok(result)
    }

    /// 读取指定 app 当前的 provider 的完整对象。
    ///
    /// 用 `crate::settings::get_effective_current_provider`（设备本地 override 优先于
    /// DB 的 is_current，与 switch_normal / 所有磁盘物化点一致）解析当前 provider id，
    /// 再 `get_provider_by_id` 取完整对象。若改用裸 DB is_current，会在本地 override
    /// 与 DB is_current 不一致的设备上选错 provider 来重建 settings.json。
    fn current_provider(
        state: &AppState,
        app_type: &AppType,
    ) -> Result<Option<crate::provider::Provider>, AppError> {
        match crate::settings::get_effective_current_provider(&state.db, app_type)? {
            Some(id) => state.db.get_provider_by_id(&id, app_type.as_str()),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_config::{
        InstalledAgent, InstalledCommand, InstalledSkill, McpApps, McpServer, Profile,
        ProfileContent, ProfileSpec, SkillApps,
    };
    use crate::database::Database;
    use crate::provider::Provider;
    use crate::services::skill::SkillService;
    use crate::store::AppState;
    use serde_json::json;
    use serial_test::serial;
    use std::env;
    use std::fs;
    use std::sync::Arc;
    use tempfile::TempDir;

    /// 在 SSOT 目录中物化一个最小可同步的 skill（带 SKILL.md），
    /// 否则 SkillService::sync_to_app 会因源缺失而报错。
    fn materialize_ssot_skill(directory: &str) {
        let ssot = SkillService::get_ssot_dir().expect("ssot dir");
        let skill_dir = ssot.join(directory);
        fs::create_dir_all(&skill_dir).expect("mkdir ssot skill");
        fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {directory}\n---\n# {directory}\n"),
        )
        .expect("write SKILL.md");
    }

    /// 测试用临时 HOME 守卫（镜像 command.rs / backup.rs 的模式）。
    struct TempHome {
        #[allow(dead_code)]
        dir: TempDir,
        original_home: Option<String>,
        original_userprofile: Option<String>,
        original_test_home: Option<String>,
    }

    impl TempHome {
        fn new() -> Self {
            let dir = TempDir::new().expect("failed to create temp home");
            let original_home = env::var("HOME").ok();
            let original_userprofile = env::var("USERPROFILE").ok();
            let original_test_home = env::var("CC_SWITCH_TEST_HOME").ok();

            env::set_var("HOME", dir.path());
            env::set_var("USERPROFILE", dir.path());
            env::set_var("CC_SWITCH_TEST_HOME", dir.path());

            Self {
                dir,
                original_home,
                original_userprofile,
                original_test_home,
            }
        }

        fn claude_dir(&self) -> std::path::PathBuf {
            self.dir.path().join(".claude")
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            match &self.original_home {
                Some(v) => env::set_var("HOME", v),
                None => env::remove_var("HOME"),
            }
            match &self.original_userprofile {
                Some(v) => env::set_var("USERPROFILE", v),
                None => env::remove_var("USERPROFILE"),
            }
            match &self.original_test_home {
                Some(v) => env::set_var("CC_SWITCH_TEST_HOME", v),
                None => env::remove_var("CC_SWITCH_TEST_HOME"),
            }
        }
    }

    fn cmd(id: &str, name: &str, enabled: bool) -> InstalledCommand {
        InstalledCommand {
            id: id.into(),
            name: name.into(),
            content: format!("# {name}\nbody"),
            description: None,
            tags: vec![],
            enabled_claude: enabled,
            installed_at: 1,
        }
    }

    fn agent(id: &str, name: &str, enabled: bool) -> InstalledAgent {
        InstalledAgent {
            id: id.into(),
            name: name.into(),
            content: format!("# {name}\nbody"),
            description: None,
            tags: vec![],
            enabled_claude: enabled,
            installed_at: 1,
        }
    }

    fn skill(id: &str, directory: &str, apps: SkillApps) -> InstalledSkill {
        InstalledSkill {
            id: id.into(),
            name: directory.into(),
            description: None,
            directory: directory.into(),
            repo_owner: None,
            repo_name: None,
            repo_branch: None,
            readme_url: None,
            apps,
            installed_at: 1,
            content_hash: None,
            updated_at: 0,
        }
    }

    fn mcp(id: &str, apps: McpApps) -> McpServer {
        McpServer {
            id: id.into(),
            name: id.into(),
            server: json!({ "command": "echo", "args": [] }),
            apps,
            description: None,
            homepage: None,
            docs: None,
            tags: vec![],
        }
    }

    fn profile(id: &str, content: ProfileContent, provider: Option<&str>) -> Profile {
        Profile {
            id: id.into(),
            app_type: "claude".into(),
            name: id.into(),
            description: None,
            is_active: false,
            current_provider_id: provider.map(|s| s.to_string()),
            spec: ProfileSpec {
                content,
                vars: serde_json::Map::new(),
            },
            sort_index: 0,
            created_at: 1,
        }
    }

    fn content(
        skills: &[&str],
        commands: &[&str],
        agents: &[&str],
        mcp: &[&str],
    ) -> ProfileContent {
        ProfileContent {
            skills: skills.iter().map(|s| s.to_string()).collect(),
            commands: commands.iter().map(|s| s.to_string()).collect(),
            agents: agents.iter().map(|s| s.to_string()).collect(),
            mcp: mcp.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn claude_provider(id: &str) -> Provider {
        Provider::with_id(
            id.into(),
            format!("Provider {id}"),
            json!({
                "env": {
                    "ANTHROPIC_API_KEY": format!("token-{id}"),
                    "ANTHROPIC_BASE_URL": "https://api.example",
                    "ANTHROPIC_MODEL": "model-x"
                },
                "permissions": { "allow": ["Bash"] }
            }),
            None,
        )
    }

    #[test]
    #[serial]
    fn activate_flips_command_and_agent_flags_to_exactly_spec() {
        let _home = TempHome::new();
        crate::settings::reload_settings().expect("reload settings");
        let db = Arc::new(Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());

        // seed commands [a,b,c] + agents [x,y], all enabled
        for c in [
            cmd("c:a", "a", true),
            cmd("c:b", "b", true),
            cmd("c:c", "c", true),
        ] {
            db.save_command(&c).expect("save command");
        }
        for a in [agent("a:x", "x", true), agent("a:y", "y", true)] {
            db.save_agent(&a).expect("save agent");
        }

        // 3b: pin a provider so step-8 deterministic settings.json rebuild succeeds
        // without emitting a "no current provider" warning (this test asserts none).
        db.save_provider("claude", &claude_provider("prov"))
            .expect("save provider");

        // profile spec: commands=[b], agents=[x]
        let p = profile("p1", content(&[], &["b"], &["x"], &[]), Some("prov"));
        db.save_profile(&p).expect("save profile");

        let res = ProfileService::activate(&state, AppType::Claude, "p1").expect("activate");
        assert!(res.warnings.is_empty(), "no warnings expected: {res:?}");

        let cmds: std::collections::HashMap<String, bool> = db
            .get_all_installed_commands()
            .unwrap()
            .into_iter()
            .map(|c| (c.name, c.enabled_claude))
            .collect();
        assert_eq!(cmds.get("b"), Some(&true), "b should be enabled");
        assert_eq!(cmds.get("a"), Some(&false), "a should be disabled");
        assert_eq!(cmds.get("c"), Some(&false), "c should be disabled");

        let agents: std::collections::HashMap<String, bool> = db
            .get_all_installed_agents()
            .unwrap()
            .into_iter()
            .map(|a| (a.name, a.enabled_claude))
            .collect();
        assert_eq!(agents.get("x"), Some(&true), "x should be enabled");
        assert_eq!(agents.get("y"), Some(&false), "y should be disabled");
    }

    #[test]
    #[serial]
    fn activate_flips_skill_and_mcp_flags_for_app_only() {
        let _home = TempHome::new();
        crate::settings::reload_settings().expect("reload settings");
        let db = Arc::new(Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());

        // skill s1: claude=false, codex=true (codex must stay true)
        // skill s2: claude=true,  codex=true
        let s1_apps = SkillApps {
            claude: false,
            codex: true,
            ..Default::default()
        };
        let s2_apps = SkillApps {
            claude: true,
            codex: true,
            ..Default::default()
        };
        db.save_skill(&skill("sk:1", "s1", s1_apps)).expect("save");
        db.save_skill(&skill("sk:2", "s2", s2_apps)).expect("save");
        // s1 becomes claude-enabled after activate → reconcile needs its SSOT source.
        materialize_ssot_skill("s1");

        // mcp m1: claude=false, codex=true ; m2: claude=true, codex=true
        let m1_apps = McpApps {
            claude: false,
            codex: true,
            ..Default::default()
        };
        let m2_apps = McpApps {
            claude: true,
            codex: true,
            ..Default::default()
        };
        db.save_mcp_server(&mcp("m1", m1_apps)).expect("save");
        db.save_mcp_server(&mcp("m2", m2_apps)).expect("save");

        // claude profile: skills=[s1], mcp=[m1]  (subset; s2/m2 should be claude-disabled)
        let p = profile("p1", content(&["s1"], &[], &[], &["m1"]), None);
        db.save_profile(&p).expect("save profile");

        ProfileService::activate(&state, AppType::Claude, "p1").expect("activate");

        let skills = db.get_all_installed_skills().unwrap();
        let s1 = skills.get("sk:1").unwrap();
        let s2 = skills.get("sk:2").unwrap();
        assert!(s1.apps.claude, "s1 claude should be enabled (in spec)");
        assert!(
            !s2.apps.claude,
            "s2 claude should be disabled (not in spec)"
        );
        // codex flag UNCHANGED on both
        assert!(s1.apps.codex, "s1 codex must be UNCHANGED (true)");
        assert!(s2.apps.codex, "s2 codex must be UNCHANGED (true)");

        let servers = db.get_all_mcp_servers().unwrap();
        let m1 = servers.get("m1").unwrap();
        let m2 = servers.get("m2").unwrap();
        assert!(m1.apps.claude, "m1 claude should be enabled (in spec)");
        assert!(
            !m2.apps.claude,
            "m2 claude should be disabled (not in spec)"
        );
        assert!(m1.apps.codex, "m1 codex must be UNCHANGED (true)");
        assert!(m2.apps.codex, "m2 codex must be UNCHANGED (true)");
    }

    #[test]
    #[serial]
    fn activate_with_missing_provider_warns_not_aborts() {
        let _home = TempHome::new();
        crate::settings::reload_settings().expect("reload settings");
        let db = Arc::new(Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());

        db.save_command(&cmd("c:a", "a", false)).expect("save");

        // The profiles table has a FK (current_provider_id, app_type) -> providers
        // with ON DELETE SET NULL, so a dangling provider id cannot be created via
        // save_profile. Simulate the dangling state (manual DB edit / migration race)
        // that activate's defensive check guards against by inserting the row with
        // FK enforcement temporarily disabled.
        {
            let conn = db.conn.lock().unwrap();
            conn.execute("PRAGMA foreign_keys = OFF;", []).unwrap();
            conn.execute(
                "INSERT INTO profiles (id, app_type, name, description, is_active,
                    current_provider_id, spec, sort_index, created_at)
                 VALUES (?1, 'claude', 'p1', NULL, 0, 'ghost', ?2, 0, 1)",
                rusqlite::params![
                    "p1",
                    serde_json::to_string(&ProfileSpec {
                        content: content(&[], &["a"], &[], &[]),
                        vars: serde_json::Map::new(),
                    })
                    .unwrap()
                ],
            )
            .unwrap();
            conn.execute("PRAGMA foreign_keys = ON;", []).unwrap();
        }

        let res = ProfileService::activate(&state, AppType::Claude, "p1").expect("activate ok");
        assert!(
            res.warnings.iter().any(|w| w.contains("provider missing")),
            "warnings should mention missing provider: {res:?}"
        );
        // content flags still flipped
        let cmds = db.get_all_installed_commands().unwrap();
        assert!(cmds[0].enabled_claude, "command a should be enabled");
        // is_active still set
        let active = db.get_active_profile("claude").unwrap().expect("active");
        assert_eq!(active.id, "p1");
    }

    #[test]
    #[serial]
    fn activate_reuses_provider_switch_sets_is_current() {
        let _home = TempHome::new();
        crate::settings::reload_settings().expect("reload settings");
        let db = Arc::new(Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());

        db.save_provider("claude", &claude_provider("p-existing"))
            .expect("save provider");

        let p = profile("p1", content(&[], &[], &[], &[]), Some("p-existing"));
        db.save_profile(&p).expect("save profile");

        ProfileService::activate(&state, AppType::Claude, "p1").expect("activate");

        let current = db.get_current_provider("claude").unwrap();
        assert_eq!(
            current.as_deref(),
            Some("p-existing"),
            "pinned provider should be is_current after activate"
        );
    }

    #[test]
    #[serial]
    fn activate_collects_warnings_for_unknown_spec_names() {
        let _home = TempHome::new();
        crate::settings::reload_settings().expect("reload settings");
        let db = Arc::new(Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());

        db.save_command(&cmd("c:real", "real", false))
            .expect("save");
        // spec lists a non-existent command name plus the real one
        let p = profile("p1", content(&[], &["real", "ghost-cmd"], &[], &[]), None);
        db.save_profile(&p).expect("save profile");

        let res = ProfileService::activate(&state, AppType::Claude, "p1").expect("activate");
        assert!(
            res.warnings
                .iter()
                .any(|w| w == "command not found: ghost-cmd"),
            "warnings should mention unknown command: {res:?}"
        );
        // the real command converged (enabled)
        let cmds = db.get_all_installed_commands().unwrap();
        assert!(cmds[0].enabled_claude, "real command should be enabled");
    }

    #[test]
    #[serial]
    fn deactivate_clears_active_leaves_disk() {
        let home = TempHome::new();
        crate::settings::reload_settings().expect("reload settings");
        let db = Arc::new(Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());

        // seed a command and an agent that profile A enables (materialize to disk)
        db.save_command(&cmd("c:a", "acmd", false)).expect("save");
        db.save_agent(&agent("a:a", "aagent", false)).expect("save");
        let p = profile("A", content(&[], &["acmd"], &["aagent"], &[]), None);
        db.save_profile(&p).expect("save profile");

        ProfileService::activate(&state, AppType::Claude, "A").expect("activate A");

        let cmd_file = home.claude_dir().join("commands").join("acmd.md");
        let agent_file = home.claude_dir().join("agents").join("aagent.md");
        assert!(cmd_file.exists(), "enabled command should be on disk");
        assert!(agent_file.exists(), "enabled agent should be on disk");

        // deactivate
        ProfileService::deactivate(&state, AppType::Claude).expect("deactivate");

        // active cleared
        assert!(
            db.get_active_profile("claude").unwrap().is_none(),
            "active profile should be cleared"
        );
        // disk untouched (no teardown in 3a)
        assert!(cmd_file.exists(), "command file must still be present");
        assert!(agent_file.exists(), "agent file must still be present");
    }

    // ========== T7: 对抗式安全测试套件 ==========

    /// Step 1: 切换 profile 时绝不删除用户亲手放进 commands 目录、无 DB 记录的文件。
    /// reconcile 只遍历 DB 行，故用户文件天然幸免；先激活 A(commands=[a])，
    /// 再激活 B(commands=[b])，整个过程 mine.md 必须始终存在。
    #[test]
    #[serial]
    fn switching_profiles_never_deletes_user_authored_command_file() {
        let home = TempHome::new();
        crate::settings::reload_settings().expect("reload settings");
        let db = Arc::new(Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());

        // 用户亲手放进 commands 目录、无 DB 记录的文件
        let cmd_dir = home.claude_dir().join("commands");
        fs::create_dir_all(&cmd_dir).expect("mkdir commands");
        let mine = cmd_dir.join("mine.md");
        fs::write(&mine, "user authored, no db row").expect("write mine.md");

        // 两个互斥 profile：A 启用 a，B 启用 b
        db.save_command(&cmd("c:a", "a", false)).expect("save a");
        db.save_command(&cmd("c:b", "b", false)).expect("save b");
        db.save_profile(&profile("A", content(&[], &["a"], &[], &[]), None))
            .expect("save A");
        db.save_profile(&profile("B", content(&[], &["b"], &[], &[]), None))
            .expect("save B");

        ProfileService::activate(&state, AppType::Claude, "A").expect("activate A");
        assert!(mine.exists(), "mine.md must survive activate A");
        assert!(cmd_dir.join("a.md").exists(), "a.md materialized by A");

        ProfileService::activate(&state, AppType::Claude, "B").expect("activate B");
        // b 启用、a 被移除（完全覆盖），但用户文件必须岿然不动
        assert!(cmd_dir.join("b.md").exists(), "b.md materialized by B");
        assert!(
            !cmd_dir.join("a.md").exists(),
            "a.md removed on switch to B"
        );
        assert!(
            mine.exists(),
            "switching profiles must NOT delete a user-authored command file"
        );
        assert_eq!(
            fs::read_to_string(&mine).unwrap(),
            "user authored, no db row"
        );
    }

    /// Step 1 (twin): agents 目录下用户亲手放进的、无 DB 记录的文件同样必须幸免。
    #[test]
    #[serial]
    fn switching_profiles_never_deletes_user_authored_agent_file() {
        let home = TempHome::new();
        crate::settings::reload_settings().expect("reload settings");
        let db = Arc::new(Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());

        let agent_dir = home.claude_dir().join("agents");
        fs::create_dir_all(&agent_dir).expect("mkdir agents");
        let mine = agent_dir.join("mine.md");
        fs::write(&mine, "user authored agent, no db row").expect("write mine.md");

        db.save_agent(&agent("a:x", "x", false)).expect("save x");
        db.save_agent(&agent("a:y", "y", false)).expect("save y");
        db.save_profile(&profile("A", content(&[], &[], &["x"], &[]), None))
            .expect("save A");
        db.save_profile(&profile("B", content(&[], &[], &["y"], &[]), None))
            .expect("save B");

        ProfileService::activate(&state, AppType::Claude, "A").expect("activate A");
        assert!(mine.exists(), "mine.md must survive activate A");
        assert!(agent_dir.join("x.md").exists(), "x.md materialized by A");

        ProfileService::activate(&state, AppType::Claude, "B").expect("activate B");
        assert!(agent_dir.join("y.md").exists(), "y.md materialized by B");
        assert!(
            !agent_dir.join("x.md").exists(),
            "x.md removed on switch to B"
        );
        assert!(
            mine.exists(),
            "switching profiles must NOT delete a user-authored agent file"
        );
        assert_eq!(
            fs::read_to_string(&mine).unwrap(),
            "user authored agent, no db row"
        );
    }

    /// Step 2: 真实（非 symlink）用户目录与某个「被 profile 启用」的 skill 同名时，
    /// 激活该 profile 绝不能销毁用户目录。这测试的是 T5 加固的 **写入通路**
    /// （sync_to_app_dir 的 materialize 保护），而非失活时的孤儿清理：碰撞名 s1
    /// 明确在 profile 的启用集合中，因此会走到 sync_to_app_dir。
    #[test]
    #[serial]
    fn switching_profiles_never_deletes_user_authored_skill_dir() {
        let _home = TempHome::new();
        crate::settings::reload_settings().expect("reload settings");
        let db = Arc::new(Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());

        // skill s1：DB 中标记为 claude 已启用，并物化 SSOT 源（否则源缺失会让
        // sync 报错而非走到 materialize 保护分支）。
        let s1_apps = SkillApps {
            claude: true,
            ..Default::default()
        };
        db.save_skill(&skill("sk:1", "s1", s1_apps))
            .expect("save s1");
        materialize_ssot_skill("s1");

        // 在 app skills 目录下放一个与 s1 同名的「真实用户目录」+ 哨兵文件。
        // 该目录是用户亲手创建的（非 AgentHub 托管的 symlink/副本）。
        let app_skills_dir = SkillService::get_app_skills_dir(&AppType::Claude).expect("app dir");
        let collision = app_skills_dir.join("s1");
        fs::create_dir_all(&collision).expect("mkdir collision");
        let sentinel = collision.join("DO_NOT_DELETE.txt");
        fs::write(&sentinel, "precious user data").expect("write sentinel");

        // profile 启用 s1（碰撞名在启用集合内 -> 触发写入通路）
        db.save_profile(&profile("A", content(&["s1"], &[], &[], &[]), None))
            .expect("save A");

        ProfileService::activate(&state, AppType::Claude, "A").expect("activate A");

        // T5 加固：materialize 检测到 dest 是真实目录 -> 跳过 + warn，绝不 remove_dir_all。
        assert!(
            sentinel.exists(),
            "user-authored skill dir sentinel must survive profile activation (T5 write-pass guard)"
        );
        assert_eq!(fs::read_to_string(&sentinel).unwrap(), "precious user data");
        let is_symlink = fs::symlink_metadata(&collision)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        assert!(
            !is_symlink && collision.is_dir(),
            "collision path must remain the user's real directory, not be replaced by a symlink"
        );
    }

    /// Step 3: profile spec 含路径穿越 / 分隔符名（"../evil"、"a/b"）。
    /// 由于这些名字没有匹配的 DB 行，activate 对它们是 no-op（仅 warn），
    /// 不应返回 Err，也不得在受管 commands 目录之外写出任何文件。
    /// 另外刻意塞入一行非法命令名的脏数据，验证 reconcile 的 validate_name 会跳过它。
    #[test]
    #[serial]
    fn profile_name_path_traversal_blocked() {
        let home = TempHome::new();
        crate::settings::reload_settings().expect("reload settings");
        let db = Arc::new(Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());

        // 受管 commands 目录的「外面」一层，用来检测逃逸写入。
        let claude_dir = home.claude_dir();
        let cmd_dir = claude_dir.join("commands");
        fs::create_dir_all(&cmd_dir).expect("mkdir commands");

        // 直接经 DAO 注入一行非法命令名的脏数据（save_command 不做名称校验），
        // 该行被标为启用，以确保 reconcile 会尝试物化它 —— validate_name 应拦截。
        db.save_command(&cmd("c:evil", "../evil", true))
            .expect("save illegal-named row");

        // profile spec 引用两个非法名（无匹配 DB 行 -> no-op）。
        db.save_profile(&profile(
            "A",
            content(&[], &["../evil", "a/b"], &[], &[]),
            None,
        ))
        .expect("save A");

        // activate 不得返回 Err。
        let res = ProfileService::activate(&state, AppType::Claude, "A").expect("activate ok");

        // 受管 commands 目录内不得出现 evil.md（reconcile validate_name 跳过脏行）。
        assert!(
            !cmd_dir.join("../evil.md").exists() && !cmd_dir.join("evil.md").exists(),
            "no file should be materialized for an illegal command name"
        );
        // 逃逸目标：~/.claude 下不得出现 evil.md（穿越未越过 commands 目录）。
        assert!(
            !claude_dir.join("evil.md").exists(),
            "path traversal must NOT write outside the managed commands dir"
        );
        // commands 目录内除潜在 .tmp 外不应有任何 .md 文件被写出。
        let md_count = fs::read_dir(&cmd_dir)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("md"))
                    .count()
            })
            .unwrap_or(0);
        assert_eq!(md_count, 0, "no .md file should be written: {res:?}");
    }

    // ========== T5: dotfile render + manifest lifecycle ==========

    /// 激活带 settings.json 片段 + statusline.sh 的 profile：
    /// - settings.json 应同时含 provider env 与 profile 片段（statusLine.x==1）
    /// - statusline.sh 应被写入为片段内容
    /// - manifest 应有一条 whole_file 记录（statusline.sh）
    #[test]
    #[serial]
    fn activate_applies_settings_fragment_and_statusline() {
        let home = TempHome::new();
        crate::settings::reload_settings().expect("reload settings");
        let db = Arc::new(Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());

        db.save_provider("claude", &claude_provider("p-existing"))
            .expect("save provider");

        let p = profile("p1", content(&[], &[], &[], &[]), Some("p-existing"));
        db.save_profile(&p).expect("save profile");
        db.set_profile_dotfile("p1", "settings.json", r#"{"statusLine":{"x":1}}"#)
            .expect("set settings.json fragment");
        db.set_profile_dotfile("p1", "statusline.sh", "echo hi")
            .expect("set statusline.sh");

        let res = ProfileService::activate(&state, AppType::Claude, "p1").expect("activate");

        // settings.json: provider env + profile fragment
        let settings_path = home.claude_dir().join("settings.json");
        assert!(settings_path.exists(), "settings.json should be written");
        let settings: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&settings_path).unwrap()).unwrap();
        assert_eq!(
            settings["env"]["ANTHROPIC_API_KEY"],
            json!("token-p-existing"),
            "provider env must be present: {res:?}"
        );
        assert_eq!(
            settings["statusLine"]["x"],
            json!(1),
            "profile settings.json fragment must be merged: {res:?}"
        );

        // statusline.sh written verbatim
        let statusline = home.claude_dir().join("statusline.sh");
        assert!(statusline.exists(), "statusline.sh should be written");
        assert_eq!(fs::read_to_string(&statusline).unwrap(), "echo hi");

        // manifest has a whole_file row for statusline.sh (NOT settings.json)
        let rows = db.get_manifest_for_profile("p1", "claude").unwrap();
        assert!(
            rows.iter()
                .any(|r| r.kind == "whole_file" && r.target_path.ends_with("statusline.sh")),
            "manifest should record statusline.sh whole_file: {rows:?}"
        );
        assert!(
            !rows
                .iter()
                .any(|r| r.target_path.ends_with("settings.json")),
            "settings.json must NOT be tracked as a whole_file: {rows:?}"
        );
    }

    // ========== 3b-2 T2: ${VAR} render on activate (settings.json + statusline) ==========

    /// 构造一个带 `vars` 的 profile（其余字段同 `profile()` 助手）。
    fn profile_with_vars(id: &str, provider: Option<&str>, vars: &[(&str, &str)]) -> Profile {
        let mut p = profile(id, content(&[], &[], &[], &[]), provider);
        for (k, v) in vars {
            p.spec.vars.insert((*k).to_string(), json!(*v));
        }
        p
    }

    /// settings.json 片段中的 `${VAR}` 应被 profile var 渲染后再 merge。
    #[test]
    #[serial]
    fn activate_renders_settings_fragment_var() {
        let home = TempHome::new();
        crate::settings::reload_settings().expect("reload settings");
        let db = Arc::new(Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());

        db.save_provider("claude", &claude_provider("prov"))
            .expect("save provider");

        let p = profile_with_vars("p1", Some("prov"), &[("GREETING", "hi")]);
        db.save_profile(&p).expect("save profile");
        db.set_profile_dotfile(
            "p1",
            "settings.json",
            r#"{"statusLine":{"msg":"${GREETING}"}}"#,
        )
        .expect("set settings.json fragment");

        ProfileService::activate(&state, AppType::Claude, "p1").expect("activate");

        let settings_path = home.claude_dir().join("settings.json");
        let settings: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&settings_path).unwrap()).unwrap();
        assert_eq!(settings["statusLine"]["msg"], json!("hi"));
    }

    /// statusline.sh 中的 `${VAR}` 应被 profile var 渲染（无 JSON 转义）。
    #[test]
    #[serial]
    fn activate_renders_statusline_var() {
        let home = TempHome::new();
        crate::settings::reload_settings().expect("reload settings");
        let db = Arc::new(Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());

        db.save_provider("claude", &claude_provider("prov"))
            .expect("save provider");

        let p = profile_with_vars("p1", Some("prov"), &[("NAME", "alba")]);
        db.save_profile(&p).expect("save profile");
        db.set_profile_dotfile("p1", "statusline.sh", "echo ${NAME}")
            .expect("set statusline.sh");

        ProfileService::activate(&state, AppType::Claude, "p1").expect("activate");

        let statusline = home.claude_dir().join("statusline.sh");
        assert_eq!(fs::read_to_string(&statusline).unwrap(), "echo alba");
    }

    /// provider env 的变量（无同名 profile var）应在 settings.json 片段中解析。
    #[test]
    #[serial]
    fn provider_env_resolves_in_fragment() {
        let home = TempHome::new();
        crate::settings::reload_settings().expect("reload settings");
        let db = Arc::new(Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());

        // claude_provider 的 env.ANTHROPIC_MODEL == "model-x".
        db.save_provider("claude", &claude_provider("prov"))
            .expect("save provider");

        let p = profile("p1", content(&[], &[], &[], &[]), Some("prov"));
        db.save_profile(&p).expect("save profile");
        db.set_profile_dotfile("p1", "settings.json", r#"{"model":"${ANTHROPIC_MODEL}"}"#)
            .expect("set settings.json fragment");

        ProfileService::activate(&state, AppType::Claude, "p1").expect("activate");

        let settings_path = home.claude_dir().join("settings.json");
        let settings: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&settings_path).unwrap()).unwrap();
        assert_eq!(settings["model"], json!("model-x"));
    }

    /// 同名时 profile var 覆盖 provider env。
    #[test]
    #[serial]
    fn profile_var_overrides_provider_env() {
        let home = TempHome::new();
        crate::settings::reload_settings().expect("reload settings");
        let db = Arc::new(Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());

        // provider env.ANTHROPIC_MODEL == "model-x" (provider 层).
        db.save_provider("claude", &claude_provider("prov"))
            .expect("save provider");

        // profile var 同名覆盖为 "prof".
        let p = profile_with_vars("p1", Some("prov"), &[("ANTHROPIC_MODEL", "prof")]);
        db.save_profile(&p).expect("save profile");
        db.set_profile_dotfile("p1", "settings.json", r#"{"model":"${ANTHROPIC_MODEL}"}"#)
            .expect("set settings.json fragment");

        ProfileService::activate(&state, AppType::Claude, "p1").expect("activate");

        let settings_path = home.claude_dir().join("settings.json");
        let settings: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&settings_path).unwrap()).unwrap();
        assert_eq!(settings["model"], json!("prof"));
    }

    /// SAME-PROVIDER no-bleed (3b-2 T3): profiles A and B BOTH pin provider P with
    /// DIFFERENT settings.json fragments. Activating A then B drives
    /// `ProviderService::switch(state, P)` twice; because `current_id == id == P`,
    /// `switch_normal` takes the no-backfill branch (`if current_id != id`), so the
    /// outgoing fragment is never re-captured into P and cannot bleed. The forward
    /// step-8 deterministic rebuild (active is now B) re-renders P + B's fragment, so
    /// live settings.json reflects B (not A). Asserts P.settings_config is unchanged
    /// across both activations AND live carries B's fragment, not A's.
    ///
    /// (Confirms the no-backfill => no-bleed reasoning: a same-provider switch never
    /// reverse-strips, so there is no rendered-fragment leak path on this branch.)
    #[test]
    #[serial]
    fn same_provider_switch_no_bleed() {
        let home = TempHome::new();
        crate::settings::reload_settings().expect("reload settings");
        let db = Arc::new(Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());

        // Single provider P; capture its stored config as the immutable baseline.
        db.save_provider("claude", &claude_provider("P"))
            .expect("save provider P");
        let original_p_config = db
            .get_provider_by_id("P", "claude")
            .expect("get P")
            .expect("P exists")
            .settings_config
            .clone();

        // Two profiles, both pinned to P, with DIFFERENT fragment-exclusive keys.
        let a = profile("A", content(&[], &[], &[], &[]), Some("P"));
        let b = profile("B", content(&[], &[], &[], &[]), Some("P"));
        db.save_profile(&a).expect("save profile A");
        db.save_profile(&b).expect("save profile B");
        db.set_profile_dotfile("A", "settings.json", r#"{"statusLine":{"who":"A"}}"#)
            .expect("set A fragment");
        db.set_profile_dotfile("B", "settings.json", r#"{"statusLine":{"who":"B"}}"#)
            .expect("set B fragment");

        // Activate A (switch to P, then activate B (switch P->P: same provider).
        ProfileService::activate(&state, AppType::Claude, "A").expect("activate A");
        let after_a = db
            .get_provider_by_id("P", "claude")
            .expect("get P after A")
            .expect("P exists")
            .settings_config
            .clone();
        assert_eq!(
            after_a, original_p_config,
            "P.settings_config must be unchanged after activating A (forward-only, no bleed)"
        );

        ProfileService::activate(&state, AppType::Claude, "B").expect("activate B");
        let after_b = db
            .get_provider_by_id("P", "claude")
            .expect("get P after B")
            .expect("P exists")
            .settings_config
            .clone();
        assert_eq!(
            after_b, original_p_config,
            "P.settings_config must be unchanged after same-provider switch A->B \
             (no-backfill branch => no fragment bleed)"
        );

        // Live settings.json must reflect B's fragment, NOT A's: step-8 forward rebuild
        // re-renders with active=B, so A's statusLine is overwritten by B's.
        let settings_path = home.claude_dir().join("settings.json");
        let live: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&settings_path).unwrap()).unwrap();
        assert_eq!(
            live["statusLine"]["who"],
            json!("B"),
            "live settings.json must reflect the incoming (B) fragment after same-provider switch: {live}"
        );
        assert_eq!(
            live["env"]["ANTHROPIC_API_KEY"],
            json!("token-P"),
            "provider P env must remain present in live settings: {live}"
        );
    }

    /// 未知变量在 statusline 中保留字面量并在 warnings 中提及。
    #[test]
    #[serial]
    fn unknown_var_kept_literal_with_warning() {
        let home = TempHome::new();
        crate::settings::reload_settings().expect("reload settings");
        let db = Arc::new(Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());

        db.save_provider("claude", &claude_provider("prov"))
            .expect("save provider");

        let p = profile("p1", content(&[], &[], &[], &[]), Some("prov"));
        db.save_profile(&p).expect("save profile");
        db.set_profile_dotfile("p1", "statusline.sh", "x ${MISSING}")
            .expect("set statusline.sh");

        let res = ProfileService::activate(&state, AppType::Claude, "p1").expect("activate");

        let statusline = home.claude_dir().join("statusline.sh");
        assert_eq!(fs::read_to_string(&statusline).unwrap(), "x ${MISSING}");
        assert!(
            res.warnings.iter().any(|w| w.contains("MISSING")),
            "warnings should mention MISSING: {res:?}"
        );
    }

    /// 切换时移除 OUTGOING 的 statusline，但不删 INCOMING 的：
    /// activate P (statusline "P") -> activate Q (无 statusline) -> statusline 被删除；
    /// activate R (statusline "R") -> statusline == "R"。
    #[test]
    #[serial]
    fn switch_removes_outgoing_statusline_not_incoming() {
        let home = TempHome::new();
        crate::settings::reload_settings().expect("reload settings");
        let db = Arc::new(Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());

        db.save_provider("claude", &claude_provider("prov"))
            .expect("save provider");

        // P: statusline "P"
        db.save_profile(&profile("P", content(&[], &[], &[], &[]), Some("prov")))
            .expect("save P");
        db.set_profile_dotfile("P", "statusline.sh", "P")
            .expect("statusline P");
        // Q: no statusline
        db.save_profile(&profile("Q", content(&[], &[], &[], &[]), Some("prov")))
            .expect("save Q");
        // R: statusline "R"
        db.save_profile(&profile("R", content(&[], &[], &[], &[]), Some("prov")))
            .expect("save R");
        db.set_profile_dotfile("R", "statusline.sh", "R")
            .expect("statusline R");

        let statusline = home.claude_dir().join("statusline.sh");

        ProfileService::activate(&state, AppType::Claude, "P").expect("activate P");
        assert_eq!(fs::read_to_string(&statusline).unwrap(), "P");

        ProfileService::activate(&state, AppType::Claude, "Q").expect("activate Q");
        assert!(
            !statusline.exists(),
            "owned outgoing statusline.sh should be removed on switch to Q"
        );

        ProfileService::activate(&state, AppType::Claude, "R").expect("activate R");
        assert_eq!(
            fs::read_to_string(&statusline).unwrap(),
            "R",
            "incoming R statusline should be written"
        );
    }

    /// 用户手改过的 statusline 不被删除（hash 不匹配 -> skip + warn）。
    #[test]
    #[serial]
    fn switch_does_not_delete_user_modified_statusline() {
        let home = TempHome::new();
        crate::settings::reload_settings().expect("reload settings");
        let db = Arc::new(Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());

        db.save_provider("claude", &claude_provider("prov"))
            .expect("save provider");

        db.save_profile(&profile("P", content(&[], &[], &[], &[]), Some("prov")))
            .expect("save P");
        db.set_profile_dotfile("P", "statusline.sh", "P")
            .expect("statusline P");
        db.save_profile(&profile("Q", content(&[], &[], &[], &[]), Some("prov")))
            .expect("save Q");

        let statusline = home.claude_dir().join("statusline.sh");

        ProfileService::activate(&state, AppType::Claude, "P").expect("activate P");
        assert_eq!(fs::read_to_string(&statusline).unwrap(), "P");

        // user hand-edits statusline.sh -> hash no longer matches manifest
        fs::write(&statusline, "EDITED").expect("user edit");

        let res = ProfileService::activate(&state, AppType::Claude, "Q").expect("activate Q");
        assert_eq!(
            fs::read_to_string(&statusline).unwrap(),
            "EDITED",
            "user-edited statusline.sh must NOT be deleted (hash mismatch)"
        );
        assert!(
            res.warnings.iter().any(|w| w.contains("statusline.sh")),
            "a skip warning mentioning statusline.sh should be present: {res:?}"
        );
    }

    /// 回归（高危修复）：对同一 profile **重复激活**必须是幂等的 —— 不丢失 manifest 行、
    /// 不误报「被用户编辑」警告。否则 manifest 行丢失会让后续 OUTGOING 移除/拆除找不到行，
    /// 导致我们自己拥有的 statusline.sh 泄漏在磁盘上。
    /// 步骤：activate P (有 statusline) -> 再次 activate P (断言 manifest 仍有该行、无误报警告)
    /// -> activate Q (无 statusline)，断言我们拥有的 statusline.sh **被移除**（行未丢失）。
    #[test]
    #[serial]
    fn reactivate_same_profile_keeps_manifest_and_removes_on_switch() {
        let home = TempHome::new();
        crate::settings::reload_settings().expect("reload settings");
        let db = Arc::new(Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());

        db.save_provider("claude", &claude_provider("prov"))
            .expect("save provider");

        db.save_profile(&profile("P", content(&[], &[], &[], &[]), Some("prov")))
            .expect("save P");
        db.set_profile_dotfile("P", "statusline.sh", "P")
            .expect("statusline P");
        // Q has no statusline -> step 6 must remove the owned outgoing one.
        db.save_profile(&profile("Q", content(&[], &[], &[], &[]), Some("prov")))
            .expect("save Q");

        let statusline = home.claude_dir().join("statusline.sh");

        // First activate P: writes statusline.sh + records 1 whole_file manifest row.
        ProfileService::activate(&state, AppType::Claude, "P").expect("activate P (1st)");
        assert_eq!(fs::read_to_string(&statusline).unwrap(), "P");
        let rows1: Vec<_> = db
            .get_manifest_for_profile("P", "claude")
            .unwrap()
            .into_iter()
            .filter(|r| r.kind == "whole_file")
            .collect();
        assert_eq!(rows1.len(), 1, "first activate records the row: {rows1:?}");

        // Re-activate the SAME profile P. Must be idempotent: the manifest row must
        // persist (re-recorded) and NO spurious "user-edited" warning may appear.
        let res = ProfileService::activate(&state, AppType::Claude, "P").expect("re-activate P");
        assert!(
            !res.warnings
                .iter()
                .any(|w| w.contains("statusline.sh") && w.contains("skipped")),
            "re-activating the same profile must NOT emit a spurious skip warning: {res:?}"
        );
        let rows2: Vec<_> = db
            .get_manifest_for_profile("P", "claude")
            .unwrap()
            .into_iter()
            .filter(|r| r.kind == "whole_file")
            .collect();
        assert_eq!(
            rows2.len(),
            1,
            "re-activate must keep exactly one whole_file row (not lose it): {rows2:?}"
        );
        assert_eq!(
            fs::read_to_string(&statusline).unwrap(),
            "P",
            "statusline.sh content unchanged after idempotent re-activate"
        );

        // Now switch to Q (no statusline): the AgentHub-owned statusline.sh MUST be
        // removed. This only works if the re-activate kept the manifest row.
        ProfileService::activate(&state, AppType::Claude, "Q").expect("activate Q");
        assert!(
            !statusline.exists(),
            "owned statusline.sh must be removed on switch to Q (manifest row was retained)"
        );
    }

    /// deactivate 拆除 dotfiles，但保留 provider 键：
    /// activate P (fragment + statusline + provider env.token) -> deactivate ->
    /// statusline 消失；settings.json 含 provider env.token 但不含 statusLine；
    /// get_active_profile == None。
    #[test]
    #[serial]
    fn deactivate_tears_down_dotfiles_keeps_provider_keys() {
        let home = TempHome::new();
        crate::settings::reload_settings().expect("reload settings");
        let db = Arc::new(Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());

        db.save_provider("claude", &claude_provider("prov"))
            .expect("save provider");

        db.save_profile(&profile("P", content(&[], &[], &[], &[]), Some("prov")))
            .expect("save P");
        db.set_profile_dotfile("P", "settings.json", r#"{"statusLine":{"x":1}}"#)
            .expect("settings fragment");
        db.set_profile_dotfile("P", "statusline.sh", "echo hi")
            .expect("statusline");

        ProfileService::activate(&state, AppType::Claude, "P").expect("activate P");
        let statusline = home.claude_dir().join("statusline.sh");
        assert!(statusline.exists(), "statusline written by activate");

        ProfileService::deactivate(&state, AppType::Claude).expect("deactivate");

        // statusline torn down
        assert!(
            !statusline.exists(),
            "deactivate should tear down owned statusline.sh"
        );

        // active cleared
        assert!(
            db.get_active_profile("claude").unwrap().is_none(),
            "active profile should be cleared"
        );

        // settings.json: provider env survives, profile fragment gone
        let settings_path = home.claude_dir().join("settings.json");
        let settings: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&settings_path).unwrap()).unwrap();
        assert_eq!(
            settings["env"]["ANTHROPIC_API_KEY"],
            json!("token-prov"),
            "provider env token must remain after deactivate"
        );
        assert!(
            settings.get("statusLine").is_none(),
            "profile fragment statusLine must be gone after deactivate: {settings}"
        );
    }

    /// INTERACTION (3b-1): activate 的步骤 8（最终 settings.json 重建）必须用
    /// `get_effective_current_provider`（设备本地 override 优先于 DB is_current）解析
    /// 当前 provider，而非裸 DB is_current。否则在「本地 override != DB is_current」的
    /// 设备上会用错 provider 重建。
    ///
    /// 构造：DB is_current = B，设备本地 override = A，激活一个**不 pin** provider 的
    /// profile（故步骤 2 不切换、不改 is_current）；断言 settings.json 用 A（有效当前）
    /// 而非 B 重建（env token == token-A）。
    #[test]
    #[serial]
    fn activate_rebuild_uses_effective_current_provider_not_db_is_current() {
        let home = TempHome::new();
        crate::settings::reload_settings().expect("reload settings");
        let db = Arc::new(Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());

        // Two providers; DB is_current points at B, device-local override at A.
        db.save_provider("claude", &claude_provider("A"))
            .expect("save A");
        db.save_provider("claude", &claude_provider("B"))
            .expect("save B");
        db.set_current_provider("claude", "B")
            .expect("DB is_current = B");
        crate::settings::set_current_provider(&AppType::Claude, Some("A"))
            .expect("device-local current = A");

        // Profile pins NO provider -> step 2 switch is skipped, is_current untouched.
        db.save_profile(&profile("p1", content(&[], &[], &[], &[]), None))
            .expect("save profile");

        ProfileService::activate(&state, AppType::Claude, "p1").expect("activate p1");

        let settings_path = home.claude_dir().join("settings.json");
        let settings: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&settings_path).unwrap()).unwrap();
        assert_eq!(
            settings["env"]["ANTHROPIC_API_KEY"],
            json!("token-A"),
            "step 8 must rebuild from the EFFECTIVE current provider A (device-local \
             override), not DB is_current B: {settings}"
        );
    }

    /// 没有任何 current provider 时，activate 仍 Ok 且带警告，不 panic，不写 settings。
    #[test]
    #[serial]
    fn activate_missing_provider_warns_no_settings_write() {
        let _home = TempHome::new();
        crate::settings::reload_settings().expect("reload settings");
        let db = Arc::new(Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());

        // profile pins no provider AND no provider is current in DB
        db.save_profile(&profile("p1", content(&[], &[], &[], &[]), None))
            .expect("save profile");

        let res = ProfileService::activate(&state, AppType::Claude, "p1").expect("activate ok");
        assert!(
            res.warnings
                .iter()
                .any(|w| w.contains("no current provider")),
            "should warn about no current provider: {res:?}"
        );
        let active = db.get_active_profile("claude").unwrap().expect("active");
        assert_eq!(active.id, "p1");
    }

    // ========== 3b-3 T4: activate/deactivate CLAUDE.md via hidden prompt row ==========

    /// 构造一个 codex profile（app_type = "codex"，其余同 `profile()` 助手）。
    fn codex_profile(id: &str, content: ProfileContent, provider: Option<&str>) -> Profile {
        let mut p = profile(id, content, provider);
        p.app_type = "codex".into();
        p
    }

    /// 激活带 CLAUDE.md profile_dotfile 的 claude profile：
    /// - ~/.claude/CLAUDE.md == 字面量内容；
    /// - get_prompts（UI/已过滤）不含隐藏行；
    /// - get_prompts_with_hidden 含隐藏行且 enabled==true。
    #[test]
    #[serial]
    fn activate_claude_profile_writes_claude_md_via_hidden_row() {
        let home = TempHome::new();
        crate::settings::reload_settings().expect("reload settings");
        let db = Arc::new(Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());

        db.save_provider("claude", &claude_provider("prov"))
            .expect("save provider");

        let p = profile("p1", content(&[], &[], &[], &[]), Some("prov"));
        db.save_profile(&p).expect("save profile");
        db.set_profile_dotfile("p1", "CLAUDE.md", "hello")
            .expect("set CLAUDE.md dotfile");

        ProfileService::activate(&state, AppType::Claude, "p1").expect("activate");

        let claude_md = home.claude_dir().join("CLAUDE.md");
        assert_eq!(
            fs::read_to_string(&claude_md).unwrap(),
            "hello",
            "live CLAUDE.md must be the literal dotfile content"
        );

        let hidden_id = "__profile__:p1";
        let filtered = db.get_prompts("claude").unwrap();
        assert!(
            !filtered.contains_key(hidden_id),
            "UI-filtered get_prompts must NOT contain the hidden profile row: {filtered:?}"
        );

        let all = db.get_prompts_with_hidden("claude").unwrap();
        let hidden = all
            .get(hidden_id)
            .expect("get_prompts_with_hidden must contain the hidden row");
        assert!(hidden.enabled, "hidden profile row must be enabled");
        assert!(hidden.hidden, "profile row must be marked hidden");
        assert_eq!(hidden.content, "hello");
    }

    /// 非 claude profile（codex）即使带 CLAUDE.md dotfile，step 5b 也是 no-op：
    /// - 不为 codex 创建任何 `__profile__:` 隐藏行；
    /// - ~/.codex/AGENTS.md 不含该 CLAUDE.md 内容（"x"）。
    #[test]
    #[serial]
    fn non_claude_profile_with_claude_md_is_noop() {
        let home = TempHome::new();
        crate::settings::reload_settings().expect("reload settings");
        let db = Arc::new(Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());

        let p = codex_profile("cx", content(&[], &[], &[], &[]), None);
        db.save_profile(&p).expect("save codex profile");
        db.set_profile_dotfile("cx", "CLAUDE.md", "x")
            .expect("set CLAUDE.md dotfile on codex profile");

        ProfileService::activate(&state, AppType::Codex, "cx").expect("activate codex");

        // No hidden __profile__ row created for codex (CLAUDE.md is Claude-only).
        let all = db.get_prompts_with_hidden("codex").unwrap();
        assert!(
            !all.keys().any(|k| k.starts_with("__profile__:")),
            "codex activate must NOT create a __profile__ hidden row: {all:?}"
        );
        // And the codex CLAUDE.md hidden id is absent too (no cross-app leakage).
        assert!(
            db.get_prompt_with_hidden("codex", "__profile__:cx")
                .unwrap()
                .is_none(),
            "no hidden row for codex profile cx"
        );

        // ~/.codex/AGENTS.md must NOT contain the CLAUDE.md content.
        let agents_md = home.dir.path().join(".codex").join("AGENTS.md");
        if agents_md.exists() {
            let body = fs::read_to_string(&agents_md).unwrap();
            assert!(
                !body.contains('x'),
                "codex AGENTS.md must not receive the CLAUDE.md content: {body:?}"
            );
        }
    }

    /// 在 claude profile 间切换：CLAUDE.md 重新指向 INCOMING；
    /// OUTGOING 隐藏行 disabled，INCOMING 隐藏行 enabled。
    #[test]
    #[serial]
    fn switch_claude_profiles_repoints_claude_md() {
        let home = TempHome::new();
        crate::settings::reload_settings().expect("reload settings");
        let db = Arc::new(Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());

        db.save_provider("claude", &claude_provider("prov"))
            .expect("save provider");

        db.save_profile(&profile("A", content(&[], &[], &[], &[]), Some("prov")))
            .expect("save A");
        db.set_profile_dotfile("A", "CLAUDE.md", "A")
            .expect("CLAUDE.md A");
        db.save_profile(&profile("B", content(&[], &[], &[], &[]), Some("prov")))
            .expect("save B");
        db.set_profile_dotfile("B", "CLAUDE.md", "B")
            .expect("CLAUDE.md B");

        let claude_md = home.claude_dir().join("CLAUDE.md");

        ProfileService::activate(&state, AppType::Claude, "A").expect("activate A");
        assert_eq!(fs::read_to_string(&claude_md).unwrap(), "A");

        ProfileService::activate(&state, AppType::Claude, "B").expect("activate B");
        assert_eq!(
            fs::read_to_string(&claude_md).unwrap(),
            "B",
            "live CLAUDE.md must repoint to B after switch"
        );

        let all = db.get_prompts_with_hidden("claude").unwrap();
        assert!(
            !all.get("__profile__:A").unwrap().enabled,
            "outgoing A hidden row must be disabled after switch"
        );
        assert!(
            all.get("__profile__:B").unwrap().enabled,
            "incoming B hidden row must be enabled after switch"
        );
        assert_eq!(
            all.values().filter(|p| p.enabled).count(),
            1,
            "exactly one prompt row may be enabled"
        );
    }

    /// deactivate 清空 CLAUDE.md（无其它启用 prompt）。
    #[test]
    #[serial]
    fn deactivate_blanks_claude_md() {
        let home = TempHome::new();
        crate::settings::reload_settings().expect("reload settings");
        let db = Arc::new(Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());

        db.save_provider("claude", &claude_provider("prov"))
            .expect("save provider");

        db.save_profile(&profile("A", content(&[], &[], &[], &[]), Some("prov")))
            .expect("save A");
        db.set_profile_dotfile("A", "CLAUDE.md", "A")
            .expect("CLAUDE.md A");

        let claude_md = home.claude_dir().join("CLAUDE.md");
        ProfileService::activate(&state, AppType::Claude, "A").expect("activate A");
        assert_eq!(fs::read_to_string(&claude_md).unwrap(), "A");

        ProfileService::deactivate(&state, AppType::Claude).expect("deactivate");

        assert_eq!(
            fs::read_to_string(&claude_md).unwrap(),
            "",
            "deactivate must blank CLAUDE.md when no other prompt is enabled"
        );
        assert!(
            !db.get_prompt_with_hidden("claude", "__profile__:A")
                .unwrap()
                .unwrap()
                .enabled,
            "hidden row must be disabled after deactivate"
        );
    }

    /// 重新激活同一 profile：模板覆盖手改的 live CLAUDE.md（模板为权威）。
    #[test]
    #[serial]
    fn reactivate_same_profile_claude_md_template_wins() {
        let home = TempHome::new();
        crate::settings::reload_settings().expect("reload settings");
        let db = Arc::new(Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());

        db.save_provider("claude", &claude_provider("prov"))
            .expect("save provider");

        db.save_profile(&profile("A", content(&[], &[], &[], &[]), Some("prov")))
            .expect("save A");
        db.set_profile_dotfile("A", "CLAUDE.md", "A")
            .expect("CLAUDE.md A");

        let claude_md = home.claude_dir().join("CLAUDE.md");
        ProfileService::activate(&state, AppType::Claude, "A").expect("activate A (1st)");
        assert_eq!(fs::read_to_string(&claude_md).unwrap(), "A");

        // user hand-edits the live CLAUDE.md
        fs::write(&claude_md, "USER EDIT").expect("hand edit");

        // re-activate the SAME profile -> template must win (not the edit)
        ProfileService::activate(&state, AppType::Claude, "A").expect("re-activate A");
        assert_eq!(
            fs::read_to_string(&claude_md).unwrap(),
            "A",
            "re-activating must restore the authoritative template, not the user edit"
        );
        assert_eq!(
            db.get_prompt_with_hidden("claude", "__profile__:A")
                .unwrap()
                .unwrap()
                .content,
            "A",
            "hidden row content must remain the template (no live-edit capture)"
        );
    }
}
