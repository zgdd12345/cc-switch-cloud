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
}
