//! Project 命令层（增量 4a，Claude only）。
//!
//! 暴露项目绑定的 CRUD / set_enabled / apply / detach / manifest 读取。
//! seed-from-profile 是一次性快照（COPY spec.content），非活链接——
//! 之后编辑 profile 不会改变已绑定项目（决策 #3）。

use crate::app_config::AppType;
use crate::app_config::{ManifestEntry, Project, ProjectSpec};
use crate::services::project_apply::{ProjectApplyResult, ProjectApplyService};
use crate::services::project_paths::ProjectBase;
use crate::store::AppState;
use std::str::FromStr;
use tauri::State;

/// List all projects (device-local).
#[tauri::command]
pub fn project_list(state: State<'_, AppState>) -> Result<Vec<Project>, String> {
    state.db.get_all_projects().map_err(|e| e.to_string())
}

/// Get a single project by id.
#[tauri::command]
pub fn project_get(id: String, state: State<'_, AppState>) -> Result<Option<Project>, String> {
    state.db.get_project(&id).map_err(|e| e.to_string())
}

/// Create or update a project binding. Runs the path-safety gate; optionally
/// seeds spec.content from a profile as a ONE-TIME snapshot (not a live link).
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn project_save(
    id: Option<String>,
    app: String,
    entered_path: String,
    name: Option<String>,
    spec: ProjectSpec,
    seed_from_profile_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<Project, String> {
    run_save_logic(
        &state,
        id,
        &app,
        &entered_path,
        name,
        spec,
        seed_from_profile_id,
    )
    .map_err(|e| e.to_string())
}

/// Delete a project. Detach FIRST (owned-delete its files + clear its manifest
/// rows), THEN drop the projects row. project_id has NO FK CASCADE (design
/// decision, Task 1), so deleting the row alone would orphan its manifest rows
/// and leave its materialized files on disk; detach-then-delete avoids that.
/// detach is idempotent and safe to run even if nothing was applied.
#[tauri::command]
pub fn project_delete(id: String, state: State<'_, AppState>) -> Result<bool, String> {
    // best-effort detach (owned-delete + clear rows) before removing the row;
    // ignore "project not found" so a never-applied project still deletes.
    if state
        .db
        .get_project(&id)
        .map_err(|e| e.to_string())?
        .is_some()
    {
        ProjectApplyService::detach(&state, &id).map_err(|e| e.to_string())?;
    }
    state.db.delete_project(&id).map_err(|e| e.to_string())
}

/// Pause/resume a project (enabled flag; NOT an exclusive switch).
#[tauri::command]
pub fn project_set_enabled(
    id: String,
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    let Some(mut p) = state.db.get_project(&id).map_err(|e| e.to_string())? else {
        return Ok(false);
    };
    p.enabled = enabled;
    p.updated_at = chrono::Utc::now().timestamp();
    state.db.save_project(&p).map_err(|e| e.to_string())?;
    Ok(true)
}

/// Apply a project's content set into <project>/.claude (Claude only).
#[tauri::command]
pub fn project_apply(id: String, state: State<'_, AppState>) -> Result<ProjectApplyResult, String> {
    ProjectApplyService::apply(&state, &id).map_err(|e| e.to_string())
}

/// Detach: owned-delete the project's materialized files, keep dirs.
#[tauri::command]
pub fn project_detach(
    id: String,
    state: State<'_, AppState>,
) -> Result<ProjectApplyResult, String> {
    ProjectApplyService::detach(&state, &id).map_err(|e| e.to_string())
}

/// Read the applied-files manifest for a project (its project channel).
#[tauri::command]
pub fn project_manifest(
    id: String,
    state: State<'_, AppState>,
) -> Result<Vec<ManifestEntry>, String> {
    let Some(p) = state.db.get_project(&id).map_err(|e| e.to_string())? else {
        return Ok(vec![]);
    };
    let channel = format!("project:{}", p.project_path);
    state
        .db
        .get_manifest_for_channel(&channel)
        .map_err(|e| e.to_string())
}

/// Shared, unit-testable save logic (bypasses the Tauri State wrapper).
pub(crate) fn run_save_logic(
    state: &AppState,
    id: Option<String>,
    app: &str,
    entered_path: &str,
    name: Option<String>,
    mut spec: ProjectSpec,
    seed_from_profile_id: Option<String>,
) -> Result<Project, crate::error::AppError> {
    let app_type = AppType::from_str(app)?;
    // CRITICAL #1: validate + canonicalize at save time.
    let base = ProjectBase::resolve(entered_path, &app_type)?;
    let canon = base.root().to_string_lossy().to_string();

    // One-time seed-from-profile snapshot (decision #3): only when the project
    // has no own content yet AND a profile id is given. COPY spec.content.
    if let Some(pid) = seed_from_profile_id {
        let empty = spec.content.skills.is_empty()
            && spec.content.commands.is_empty()
            && spec.content.agents.is_empty()
            && spec.content.mcp.is_empty();
        if empty {
            if let Some(prof) = state.db.get_profile(&pid)? {
                spec.content = prof.spec.content.clone(); // snapshot, not a live link
            }
        }
    }

    let now = chrono::Utc::now().timestamp();
    let (id, created_at) = match id.as_deref().and_then(|i| {
        state
            .db
            .get_project(i)
            .ok()
            .flatten()
            .map(|p| (i.to_string(), p.created_at))
    }) {
        Some((existing_id, created)) => (existing_id, created),
        None => (
            id.unwrap_or_else(|| format!("proj:{}", uuid::Uuid::new_v4())),
            now,
        ),
    };

    let project = Project {
        id,
        project_path: canon,
        entered_path: entered_path.to_string(),
        app_type: app.to_string(),
        name,
        spec,
        enabled: true,
        created_at,
        updated_at: now,
    };
    state.db.save_project(&project)?;
    Ok(project)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_config::{ProfileContent, ProfileSpec};
    use crate::database::Database;
    use serial_test::serial;
    use std::env;
    use std::sync::Arc;
    use tempfile::TempDir;

    struct TempHome {
        dir: TempDir,
        oh: Option<String>,
        ou: Option<String>,
        ot: Option<String>,
    }
    impl TempHome {
        fn new() -> Self {
            let dir = TempDir::new().expect("tmp");
            let (oh, ou, ot) = (
                env::var("HOME").ok(),
                env::var("USERPROFILE").ok(),
                env::var("CC_SWITCH_TEST_HOME").ok(),
            );
            env::set_var("HOME", dir.path());
            env::set_var("USERPROFILE", dir.path());
            env::set_var("CC_SWITCH_TEST_HOME", dir.path());
            Self { dir, oh, ou, ot }
        }
    }
    impl Drop for TempHome {
        fn drop(&mut self) {
            match &self.oh {
                Some(v) => env::set_var("HOME", v),
                None => env::remove_var("HOME"),
            }
            match &self.ou {
                Some(v) => env::set_var("USERPROFILE", v),
                None => env::remove_var("USERPROFILE"),
            }
            match &self.ot {
                Some(v) => env::set_var("CC_SWITCH_TEST_HOME", v),
                None => env::remove_var("CC_SWITCH_TEST_HOME"),
            }
        }
    }

    #[test]
    #[serial]
    fn save_seed_from_profile_is_a_snapshot_not_a_live_link() {
        let home = TempHome::new();
        crate::settings::reload_settings().ok();
        let db = Arc::new(Database::memory().expect("db"));
        let state = AppState::new(db.clone());

        // a profile whose content we will SEED from
        let prof = crate::app_config::Profile {
            id: "local:claude:Src".into(),
            app_type: "claude".into(),
            name: "Src".into(),
            description: None,
            is_active: false,
            current_provider_id: None,
            spec: ProfileSpec {
                content: ProfileContent {
                    skills: vec![],
                    commands: vec!["seeded-cmd".into()],
                    agents: vec![],
                    mcp: vec![],
                },
                vars: serde_json::Map::new(),
            },
            sort_index: 0,
            created_at: 0,
        };
        db.save_profile(&prof).expect("save profile");

        let root = home.dir.path().join("seedproj");
        std::fs::create_dir_all(&root).expect("mkdir");

        // create the project, seeding from the profile (one-time COPY)
        let created = run_save_logic(
            &state,
            None,
            "claude",
            root.to_str().unwrap(),
            Some("Seeded".into()),
            ProjectSpec::default(),
            Some("local:claude:Src".into()),
        )
        .expect("save");
        assert_eq!(
            created.spec.content.commands,
            vec!["seeded-cmd"],
            "seed copied profile content"
        );

        // EDIT the profile afterwards — must NOT change the already-bound project
        let mut prof2 = db.get_profile("local:claude:Src").unwrap().unwrap();
        prof2.spec.content.commands = vec!["CHANGED".into()];
        db.save_profile(&prof2).expect("update profile");

        let reloaded = db.get_project(&created.id).unwrap().unwrap();
        assert_eq!(
            reloaded.spec.content.commands,
            vec!["seeded-cmd"],
            "project is a snapshot; profile edit must not leak in"
        );
    }

    #[test]
    #[serial]
    fn save_refuses_home_collapse_path() {
        let home = TempHome::new();
        crate::settings::reload_settings().ok();
        let db = Arc::new(Database::memory().expect("db"));
        let state = AppState::new(db.clone());
        // binding $HOME must be refused by the gate inside project_save
        let err = run_save_logic(
            &state,
            None,
            "claude",
            home.dir.path().to_str().unwrap(),
            None,
            ProjectSpec::default(),
            None,
        );
        assert!(err.is_err(), "binding HOME must be refused at save time");
    }
}
