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
use crate::services::provider::ProviderService;
use crate::services::{McpService, SkillService};
use crate::store::AppState;

/// activate 的结果：携带非致命警告。
//
// NOTE: `#[allow(dead_code)]` 暂留——ProfileService 的 Tauri command 层封装
// （`commands/profile.rs`）在后续任务接入；届时移除这些 allow。
#[allow(dead_code)]
#[derive(Debug, serde::Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ActivateResult {
    pub warnings: Vec<String>,
}

/// Profile 业务逻辑服务（无状态单元结构，镜像 ProviderService / McpService）。
#[allow(dead_code)]
pub struct ProfileService;

#[allow(dead_code)]
impl ProfileService {
    /// 激活指定 Profile。
    ///
    /// 不变量：步骤 4 的 reconciler 永远在最后无条件运行；步骤 5 的 set_active_profile
    /// 在 reconcile 成功之后才执行，因此中途失败会保留先前的 active profile。
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

        // ---- 5. 设置 active profile（最后一步）----
        state.db.set_active_profile(app_type.as_str(), profile_id)?;

        Ok(result)
    }

    /// 取消激活指定 app_type 的当前 Profile。
    ///
    /// 3a 范围内仅清除 DB 中的 active 标志，**不**拆除磁盘上的内容（无 teardown）。
    /// 切换 = activate(other)：step 3 的完全覆盖会移除先前 profile 的多余内容。
    pub fn deactivate(state: &AppState, app_type: AppType) -> Result<(), AppError> {
        state.db.clear_active_profile(app_type.as_str())
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

        // profile spec: commands=[b], agents=[x]
        let p = profile("p1", content(&[], &["b"], &["x"], &[]), None);
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
}
