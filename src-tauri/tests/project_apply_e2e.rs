//! E2E: project bind → apply → detach lifecycle, incl. HOME-collapse refusal
//! and user-file safety. Uses the public agenthub_lib API.

use std::sync::Arc;

// FLAT crate-root re-exports only — every source module in lib.rs is PRIVATE
// (`mod app_config;` etc.), so module-qualified paths like
// `agenthub_lib::app_config::Project` do NOT compile. Task 10b step 2 added the
// Project/ProjectSpec/ProfileContent/InstalledCommand/ProjectApplyService/ProjectBase
// re-exports; Database + AppState + AppType were already re-exported.
use agenthub_lib::{
    AppState, AppType, Database, InstalledCommand, ProfileContent, Project, ProjectApplyService,
    ProjectBase, ProjectSpec,
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
