//! E2E: project bind → apply → detach lifecycle, incl. HOME-collapse refusal
//! and user-file safety. Uses the public agenthub_lib API.

use std::sync::Arc;

// FLAT crate-root re-exports only — every source module in lib.rs is PRIVATE
// (`mod app_config;` etc.), so module-qualified paths like
// `agenthub_lib::app_config::Project` do NOT compile. Task 10b step 2 added the
// Project/ProjectSpec/ProfileContent/InstalledCommand/ProjectApplyService/ProjectBase
// re-exports; Database + AppState + AppType were already re-exported.
use agenthub_lib::{
    AppState, AppType, Database, InstalledCommand, McpApps, McpServer, OwnedKeysEnvelope,
    ProfileContent, Project, ProjectApplyService, ProjectBase, ProjectSpec,
};
use serial_test::serial;
use tempfile::TempDir;

struct TempHome {
    dir: TempDir,
    oh: Option<String>,
    ot: Option<String>,
}
impl TempHome {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let oh = std::env::var("HOME").ok();
        let ot = std::env::var("CC_SWITCH_TEST_HOME").ok();
        std::env::set_var("HOME", dir.path());
        std::env::set_var("CC_SWITCH_TEST_HOME", dir.path());
        Self { dir, oh, ot }
    }
}
impl Drop for TempHome {
    fn drop(&mut self) {
        match &self.oh {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match &self.ot {
            Some(v) => std::env::set_var("CC_SWITCH_TEST_HOME", v),
            None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
        }
    }
}

#[test]
#[serial]
fn project_apply_detach_lifecycle() {
    let home = TempHome::new();
    let db = Arc::new(Database::memory().unwrap());
    let state = AppState::new(db.clone());

    db.save_command(&InstalledCommand {
        id: "local:e2e-cmd".into(),
        name: "e2e-cmd".into(),
        content: "E2E".into(),
        description: None,
        tags: vec![],
        enabled_claude: false,
        installed_at: 0,
    })
    .unwrap();

    let root = home.dir.path().join("e2e-repo");
    std::fs::create_dir_all(&root).unwrap();
    let canon = root.canonicalize().unwrap();
    let proj = Project {
        id: "proj:e2e".into(),
        project_path: canon.to_string_lossy().to_string(),
        entered_path: root.to_string_lossy().to_string(),
        app_type: "claude".into(),
        name: Some("E2E".into()),
        spec: ProjectSpec {
            content: ProfileContent {
                skills: vec![],
                commands: vec!["e2e-cmd".into()],
                agents: vec![],
                mcp: vec![],
            },
            vars: serde_json::Map::new(),
            dotfiles: agenthub_lib::ProjectDotfiles {
                claude_md: "# e2e project memory\n".into(),
                settings: String::new(),
            },
        },
        enabled: true,
        created_at: 0,
        updated_at: 0,
    };
    db.save_project(&proj).unwrap();

    // HOME-collapse refusal: binding $HOME is rejected.
    assert!(ProjectBase::resolve(home.dir.path().to_str().unwrap(), &AppType::Claude).is_err());

    // apply → file exists + manifest recorded
    ProjectApplyService::apply(&state, "proj:e2e").unwrap();
    let f = canon.join(".claude").join("commands").join("e2e-cmd.md");
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "E2E");

    // user file in same dir survives detach
    let user = canon.join(".claude").join("commands").join("mine.md");
    std::fs::write(&user, "MINE").unwrap();

    ProjectApplyService::detach(&state, "proj:e2e").unwrap();
    assert!(!f.exists(), "owned file removed");
    assert!(user.is_file(), "user file untouched");

    // (re-apply to re-materialize after detach removed it) — apply once more to assert
    // CLAUDE.md materializes at ROOT, then a fresh detach removes it.
    ProjectApplyService::apply(&state, "proj:e2e").unwrap();
    let claude_md = canon.join("CLAUDE.md");
    assert_eq!(
        std::fs::read_to_string(&claude_md).unwrap(),
        "# e2e project memory\n",
        "project CLAUDE.md materialized at ROOT"
    );
    assert!(
        !canon.join(".claude").join("CLAUDE.md").exists(),
        "never under .claude/"
    );
    ProjectApplyService::detach(&state, "proj:e2e").unwrap();
    assert!(!claude_md.exists(), "owned CLAUDE.md removed on detach");
}

#[test]
#[serial]
fn e2e_settings_merge_apply_then_detach_preserves_user_keys() {
    let home = TempHome::new();
    let db = Arc::new(Database::memory().expect("db"));
    let state = AppState::new(db.clone());

    // a project bound with a settings fragment that uses ${VAR} + a user file on disk.
    let root = home.dir.path().join("e2e-set");
    std::fs::create_dir_all(&root).unwrap();
    let canon = root.canonicalize().unwrap();
    let target = canon.join(".claude").join("settings.json");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, r#"{"userKept": 7, "model": "USER_ORIGINAL"}"#).unwrap();

    let mut spec = ProjectSpec::default();
    spec.dotfiles.settings =
        r#"{"model": "${MODEL_NAME}", "permissions": {"defaultMode": "ask"}}"#.into();
    spec.vars.insert(
        "MODEL_NAME".into(),
        serde_json::Value::String("claude-e2e".into()),
    );
    let proj = Project {
        id: "proj:e2e-set".into(),
        project_path: canon.to_string_lossy().to_string(),
        entered_path: root.to_string_lossy().to_string(),
        app_type: "claude".into(),
        name: Some("e2e".into()),
        spec,
        enabled: true,
        created_at: 1,
        updated_at: 1,
    };
    db.save_project(&proj).unwrap();

    // apply: ${VAR} rendered, frag merged, user keys preserved, one envelope row.
    ProjectApplyService::apply(&state, &proj.id).expect("apply");
    let merged: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
    assert_eq!(merged["userKept"], serde_json::json!(7));
    assert_eq!(merged["model"], serde_json::json!("claude-e2e"));
    assert_eq!(
        merged["permissions"]["defaultMode"],
        serde_json::json!("ask")
    );

    let chan = format!("project:{}", canon.to_string_lossy());
    let rows = db.get_manifest_for_channel(&chan).unwrap();
    let env: OwnedKeysEnvelope = serde_json::from_str(
        rows.iter()
            .find(|r| r.kind == "settings_merge")
            .unwrap()
            .owned_keys
            .as_deref()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(env.v, 1);

    // detach: user keys survive; our leaves reversed ([model] restored, defaultMode removed).
    ProjectApplyService::detach(&state, &proj.id).expect("detach");
    assert!(target.exists(), "settings.json survives detach");
    let after: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
    assert_eq!(
        after["userKept"],
        serde_json::json!(7),
        "unrelated user key survives"
    );
    assert_eq!(
        after["model"],
        serde_json::json!("USER_ORIGINAL"),
        "overwritten user key restored"
    );
    assert!(
        after
            .get("permissions")
            .is_none_or(|p| p.get("defaultMode").is_none()),
        "our inserted defaultMode leaf removed"
    );
    assert_eq!(
        db.get_manifest_for_channel(&chan).unwrap().len(),
        0,
        "rows cleared"
    );
}

#[test]
#[serial]
fn e2e_mcp_merge_apply_then_detach_preserves_user_servers() {
    let home = TempHome::new();
    let db = Arc::new(Database::memory().expect("db"));
    let state = AppState::new(db.clone());

    // a server in the catalog, selected by the project's content.mcp.
    db.save_mcp_server(&McpServer {
        id: "e2e-srv".into(),
        name: "E2E Server".into(),
        server: serde_json::json!({"type":"stdio","command":"node","args":["s.js"],"enabled":true}),
        apps: McpApps::default(),
        description: None,
        homepage: None,
        docs: None,
        tags: vec![],
    })
    .unwrap();

    let root = home.dir.path().join("e2e-mcp");
    std::fs::create_dir_all(&root).unwrap();
    let canon = root.canonicalize().unwrap();
    // user pre-existing .mcp.json with their OWN server + a sibling top key.
    let target = canon.join(".mcp.json");
    std::fs::write(
        &target,
        br#"{"mcpServers":{"user-srv":{"type":"stdio","command":"u"}},"keep":42}"#,
    )
    .unwrap();

    let mut spec = ProjectSpec::default();
    spec.content.mcp = vec!["e2e-srv".into()];
    let proj = Project {
        id: "proj:e2e-mcp".into(),
        project_path: canon.to_string_lossy().to_string(),
        entered_path: root.to_string_lossy().to_string(),
        app_type: "claude".into(),
        name: Some("E2E MCP".into()),
        spec,
        enabled: true,
        created_at: 1,
        updated_at: 1,
    };
    db.save_project(&proj).unwrap();

    // apply: our server merged + stripped, ROOT .mcp.json, envelope v=1 row.
    ProjectApplyService::apply(&state, &proj.id).expect("apply");
    assert!(
        !canon.join(".claude").join(".mcp.json").exists(),
        "never under .claude/"
    );
    let merged: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
    assert_eq!(
        merged["mcpServers"]["e2e-srv"],
        serde_json::json!({"type":"stdio","command":"node","args":["s.js"]}),
        "our server merged + stripped (enabled gone)"
    );
    assert!(
        merged["mcpServers"].get("user-srv").is_some(),
        "user server kept"
    );
    assert_eq!(merged["keep"], serde_json::json!(42), "sibling key kept");

    let chan = format!("project:{}", canon.to_string_lossy());
    let rows = db.get_manifest_for_channel(&chan).unwrap();
    let env: OwnedKeysEnvelope = serde_json::from_str(
        rows.iter()
            .find(|r| r.kind == "mcp_merge")
            .unwrap()
            .owned_keys
            .as_deref()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(env.v, 1);

    // detach: our server reversed; user server + sibling key survive; file stays.
    ProjectApplyService::detach(&state, &proj.id).expect("detach");
    assert!(target.exists(), ".mcp.json survives detach");
    let after: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
    assert!(
        after["mcpServers"].get("e2e-srv").is_none(),
        "our server removed"
    );
    assert_eq!(
        after["mcpServers"]["user-srv"],
        serde_json::json!({"type":"stdio","command":"u"}),
        "user server survives detach"
    );
    assert_eq!(after["keep"], serde_json::json!(42), "sibling key survives");
}
