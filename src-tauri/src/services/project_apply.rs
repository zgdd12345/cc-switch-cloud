//! ProjectApplyService (increment 4a, Claude only).
//!
//! Materializes a project's own content set (skills COPY / commands / agents)
//! into <project>/.claude/{skills,commands,agents}/ and records every written
//! file as a project-channel apply_manifest row (channel="project:<canonpath>",
//! project_id, kind, content_hash). Detach owned-deletes exactly those rows.
//!
//! SAFETY: every path goes through ProjectBase::resolve (CRITICAL #1). Reconcile
//! is keyed on project_id: snapshot rows whose project_id differs from the
//! currently-bound project are FOREIGN and are skipped+warned, never deleted
//! (CRITICAL #1 corollary / path-reuse). write-then-record ordering mirrors the
//! global path so a crash leaves at most a recorded-and-written file.

use std::path::Path;

use crate::app_config::{AppType, ManifestEntry};
use crate::config::atomic_write;
use crate::error::AppError;
use crate::services::profile::ProfileService;
use crate::services::project_paths::{content_file_path, ProjectBase};
use crate::store::AppState;

/// Result of a project apply/detach: non-fatal warnings (mirrors ActivateResult).
///
/// `allow(dead_code)`: the apply/detach service surface is introduced here ahead
/// of its Tauri command callers, which are wired in by later tasks; the inline
/// test module exercises the full surface today (mirrors project_paths.rs).
#[allow(dead_code)]
#[derive(Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectApplyResult {
    pub warnings: Vec<String>,
}

#[allow(dead_code)] // surface consumed by Tauri command wiring in later tasks
pub struct ProjectApplyService;

#[allow(dead_code)] // surface consumed by Tauri command wiring in later tasks
impl ProjectApplyService {
    /// Apply a project's own content set into <project>/.claude (Claude only, 4a).
    pub fn apply(state: &AppState, project_id: &str) -> Result<ProjectApplyResult, AppError> {
        let mut result = ProjectApplyResult::default();

        let project = state
            .db
            .get_project(project_id)?
            .ok_or_else(|| AppError::Message(format!("project not found: {project_id}")))?;
        if project.app_type != AppType::Claude.as_str() {
            return Err(AppError::Message(format!(
                "4a applies Claude only; project app_type is {}",
                project.app_type
            )));
        }
        if !project.enabled {
            result
                .warnings
                .push("project is paused (enabled=false); apply skipped".into());
            return Ok(result);
        }
        let app = AppType::Claude;

        // CRITICAL #1: re-run the path-safety gate on EVERY apply.
        let base = ProjectBase::resolve(&project.entered_path, &app)?;
        let channel = base.channel();

        // Brief §3.2 apply-time cleanup: best-effort sweep of stale project channels
        // whose underlying dir is gone (the SOLE production path that runs the prune,
        // since project_id carries NO FK CASCADE — design decision, Task 1). Never
        // fail the apply for a hygiene sweep; log and continue.
        if let Err(e) = Self::prune_orphan_project_channels(&state.db) {
            log::warn!("prune_orphan_project_channels (apply-time cleanup) failed: {e}");
        }

        // CRITICAL #1 corollary (path-reuse): inspect the previous channel snapshot.
        // Any row whose project_id differs from THIS project's id is foreign — skip+warn,
        // never reconcile/delete. Then owned-delete + clear only OUR rows before re-apply.
        let prev = state.db.get_manifest_for_channel(&channel)?;
        let (ours, foreign): (Vec<_>, Vec<_>) = prev
            .into_iter()
            .partition(|r| r.project_id.as_deref() == Some(project.id.as_str()));
        if !foreign.is_empty() {
            result.warnings.push(format!(
                "channel {channel} has {} row(s) bound to a different project; skipping them (path reused?)",
                foreign.len()
            ));
        }
        // owned-delete OUR previous rows' targets, then clear OUR rows (idempotent re-apply).
        for r in &ours {
            if r.kind == "skill" {
                // dir-aware, content-hash-safe removal so re-apply cleans old skill copies.
                if let Some(parent) = Path::new(&r.target_path).parent() {
                    let dir_name = Path::new(&r.target_path)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    crate::services::SkillService::remove_from_project_dir(&dir_name, parent, &app)
                        .map_err(|e| {
                            AppError::Message(format!("project skill remove failed: {e}"))
                        })?;
                }
            } else {
                crate::services::profile_render::remove_whole_file_if_owned(
                    &r.target_path,
                    r.content_hash.as_deref(),
                )?;
            }
        }
        let our_ids: Vec<i64> = ours.iter().map(|r| r.id).collect();
        state.db.delete_manifest_entries(&our_ids)?;

        let content = &project.spec.content;

        // ---- commands (key = command.name) ----
        let cmds = state.db.get_all_installed_commands()?;
        let items: Vec<(String, Vec<String>)> = cmds
            .iter()
            .map(|c| (c.name.clone(), c.tags.clone()))
            .collect();
        let (want, w) = ProfileService::resolve_selectors("command", &content.commands, &items);
        result.warnings.extend(w);
        let cmd_dir = base.dotdir().join("commands");
        for c in &cmds {
            if !want.contains(&c.name) {
                continue;
            }
            let target = content_file_path(&cmd_dir, &c.name)?;
            // write-then-record: write the file FIRST, then record the manifest row.
            atomic_write(&target, c.content.as_bytes())?;
            let hash = crate::services::profile_render::content_hash(c.content.as_bytes());
            state.db.record_manifest_entry(&Self::row(
                &channel,
                &project.id,
                &target,
                "command",
                &hash,
            ))?;
        }

        // ---- agents (key = agent.name) ----
        let agents = state.db.get_all_installed_agents()?;
        let items: Vec<(String, Vec<String>)> = agents
            .iter()
            .map(|a| (a.name.clone(), a.tags.clone()))
            .collect();
        let (want, w) = ProfileService::resolve_selectors("agent", &content.agents, &items);
        result.warnings.extend(w);
        let agent_dir = base.dotdir().join("agents");
        for a in &agents {
            if !want.contains(&a.name) {
                continue;
            }
            let target = content_file_path(&agent_dir, &a.name)?;
            atomic_write(&target, a.content.as_bytes())?;
            let hash = crate::services::profile_render::content_hash(a.content.as_bytes());
            state.db.record_manifest_entry(&Self::row(
                &channel,
                &project.id,
                &target,
                "agent",
                &hash,
            ))?;
        }

        // ---- skills (key = skill.directory; COPY, real dir) ----
        let skills = state.db.get_all_installed_skills()?;
        let items: Vec<(String, Vec<String>)> = skills
            .values()
            .map(|s| (s.directory.clone(), s.tags.clone()))
            .collect();
        let (want, w) = ProfileService::resolve_selectors("skill", &content.skills, &items);
        result.warnings.extend(w);
        let skills_base = base.dotdir().join("skills");
        for s in skills.values() {
            if !want.contains(&s.directory) {
                continue;
            }
            // write-then-record: copy the skill dir FIRST, then record the manifest row
            // (content_hash = dir hash, so detach can verify ownership of the copied dir).
            crate::services::SkillService::sync_to_project_dir(&s.directory, &skills_base, &app)
                .map_err(|e| AppError::Message(format!("project skill copy failed: {e}")))?;
            let dest = skills_base.join(&s.directory);
            if let Ok(hash) = crate::services::SkillService::compute_dir_hash(&dest) {
                state.db.record_manifest_entry(&Self::row(
                    &channel,
                    &project.id,
                    &dest,
                    "skill",
                    &hash,
                ))?;
            }
        }

        Ok(result)
    }

    /// Detach: owned-delete every recorded file for this project's channel, then clear rows.
    /// Leaves empty directories in place (brief §2). Never removes user-edited/unowned files.
    pub fn detach(state: &AppState, project_id: &str) -> Result<ProjectApplyResult, AppError> {
        let mut result = ProjectApplyResult::default();
        let project = state
            .db
            .get_project(project_id)?
            .ok_or_else(|| AppError::Message(format!("project not found: {project_id}")))?;
        let app = AppType::Claude;
        let base = ProjectBase::resolve(&project.entered_path, &app)?;
        let channel = base.channel();

        let rows = state.db.get_manifest_for_channel(&channel)?;
        for r in &rows {
            // CRITICAL #1 corollary: only touch OUR rows.
            if r.project_id.as_deref() != Some(project.id.as_str()) {
                result.warnings.push(format!(
                    "skipping foreign manifest row (project_id={:?}) on detach",
                    r.project_id
                ));
                continue;
            }
            // owned-delete: only if on-disk hash == recorded hash.
            if r.kind == "skill" {
                // dir-aware, content-hash-safe removal (never remove_dir_all a real user dir)
                if let Some(parent) = Path::new(&r.target_path).parent() {
                    let dir_name = Path::new(&r.target_path)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    crate::services::SkillService::remove_from_project_dir(&dir_name, parent, &app)
                        .map_err(|e| {
                            AppError::Message(format!("project skill remove failed: {e}"))
                        })?;
                }
            } else {
                crate::services::profile_render::remove_whole_file_if_owned(
                    &r.target_path,
                    r.content_hash.as_deref(),
                )?;
            }
        }
        // clear only OUR rows; leave foreign rows intact.
        let our_ids: Vec<i64> = rows
            .iter()
            .filter(|r| r.project_id.as_deref() == Some(project.id.as_str()))
            .map(|r| r.id)
            .collect();
        state.db.delete_manifest_entries(&our_ids)?;
        Ok(result)
    }

    fn row(
        channel: &str,
        project_id: &str,
        target: &Path,
        kind: &str,
        content_hash: &str,
    ) -> ManifestEntry {
        ManifestEntry {
            id: 0,
            channel: channel.to_string(),
            profile_id: None,
            project_id: Some(project_id.to_string()),
            app_type: AppType::Claude.as_str().to_string(),
            target_path: target.to_string_lossy().to_string(),
            kind: kind.to_string(),
            content_hash: Some(content_hash.to_string()),
            created_at: chrono::Utc::now().timestamp(),
        }
    }

    /// Apply-time / doctor-style prune: drop manifest rows for any project
    /// channel whose underlying directory no longer exists (the path-gone case).
    /// INVOKED in production from `apply` (apply-time cleanup, step 5b) so this
    /// hygiene actually runs — not just from its unit test. Because project_id
    /// has NO FK CASCADE (design decision, Task 1), the project-row-deleted case
    /// is also covered here: a deleted project leaves its `project:<canonpath>`
    /// rows behind, and if its dir is gone this prune removes them; if the dir
    /// still exists, detach (run before delete) or a later re-bind reconcile
    /// clears them. Returns the number of channels pruned.
    pub fn prune_orphan_project_channels(
        db: &crate::database::Database,
    ) -> Result<usize, AppError> {
        let mut pruned = 0usize;
        for channel in db.get_all_project_channels()? {
            let path = channel.strip_prefix("project:").unwrap_or(&channel);
            if !Path::new(path).exists() {
                db.clear_manifest_for_channel(&channel)?;
                pruned += 1;
            }
        }
        Ok(pruned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_config::{ProfileContent, Project, ProjectSpec};
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
            let oh = env::var("HOME").ok();
            let ou = env::var("USERPROFILE").ok();
            let ot = env::var("CC_SWITCH_TEST_HOME").ok();
            env::set_var("HOME", dir.path());
            env::set_var("USERPROFILE", dir.path());
            env::set_var("CC_SWITCH_TEST_HOME", dir.path());
            Self { dir, oh, ou, ot }
        }
        fn home(&self) -> &Path {
            self.dir.path()
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

    fn seed_command(db: &Database, name: &str, content: &str) {
        let c = crate::app_config::InstalledCommand {
            id: format!("local:{name}"),
            name: name.into(),
            content: content.into(),
            description: None,
            tags: vec![],
            enabled_claude: false,
            installed_at: 0,
        };
        db.save_command(&c).expect("save command");
    }
    fn seed_agent(db: &Database, name: &str, content: &str) {
        let a = crate::app_config::InstalledAgent {
            id: format!("local:{name}"),
            name: name.into(),
            content: content.into(),
            description: None,
            tags: vec![],
            enabled_claude: false,
            installed_at: 0,
        };
        db.save_agent(&a).expect("save agent");
    }

    fn project_at(
        home: &Path,
        sub: &str,
        content: ProfileContent,
    ) -> (Project, std::path::PathBuf) {
        let root = home.join(sub);
        std::fs::create_dir_all(&root).expect("mkdir proj");
        let canon = root.canonicalize().expect("canon");
        let p = Project {
            id: format!("proj:{sub}"),
            project_path: canon.to_string_lossy().to_string(),
            entered_path: root.to_string_lossy().to_string(),
            app_type: "claude".into(),
            name: Some(sub.into()),
            spec: ProjectSpec {
                content,
                vars: serde_json::Map::new(),
            },
            enabled: true,
            created_at: 1,
            updated_at: 1,
        };
        (p, canon)
    }

    #[test]
    #[serial]
    fn apply_writes_commands_and_agents_then_records_manifest() {
        let home = TempHome::new();
        crate::settings::reload_settings().ok();
        let db = Arc::new(Database::memory().expect("db"));
        let state = AppState::new(db.clone());
        seed_command(&db, "cmd-foo", "FOO");
        seed_agent(&db, "agent-bar", "BAR");

        let (proj, canon) = project_at(
            home.home(),
            "work-a",
            ProfileContent {
                skills: vec![],
                commands: vec!["cmd-foo".into()],
                agents: vec!["agent-bar".into()],
                mcp: vec![],
            },
        );
        db.save_project(&proj).expect("save project");

        let res = ProjectApplyService::apply(&state, &proj.id).expect("apply");
        assert!(res.warnings.is_empty(), "no warnings: {:?}", res.warnings);

        let cmd_file = canon.join(".claude").join("commands").join("cmd-foo.md");
        let agent_file = canon.join(".claude").join("agents").join("agent-bar.md");
        assert_eq!(std::fs::read_to_string(&cmd_file).unwrap(), "FOO");
        assert_eq!(std::fs::read_to_string(&agent_file).unwrap(), "BAR");

        // write-then-record: every written file has a manifest row on the project channel,
        // tied to project_id, with content_hash. (crash-ordering: no unrecorded written file.)
        let chan = format!("project:{}", canon.to_string_lossy());
        let rows = db.get_manifest_for_channel(&chan).expect("rows");
        assert_eq!(rows.len(), 2, "two rows: one command, one agent");
        for r in &rows {
            assert_eq!(r.project_id.as_deref(), Some(proj.id.as_str()));
            assert_eq!(r.channel, chan);
            assert!(
                r.content_hash.is_some(),
                "every recorded file must carry a content_hash"
            );
            assert!(
                Path::new(&r.target_path).is_file(),
                "recorded target must exist on disk"
            );
        }
    }

    #[test]
    #[serial]
    fn detach_owned_deletes_recorded_files_and_clears_rows() {
        let home = TempHome::new();
        crate::settings::reload_settings().ok();
        let db = Arc::new(Database::memory().expect("db"));
        let state = AppState::new(db.clone());
        seed_command(&db, "cmd-foo", "FOO");
        let (proj, canon) = project_at(
            home.home(),
            "work-b",
            ProfileContent {
                skills: vec![],
                commands: vec!["cmd-foo".into()],
                agents: vec![],
                mcp: vec![],
            },
        );
        db.save_project(&proj).expect("save");
        ProjectApplyService::apply(&state, &proj.id).expect("apply");

        let cmd_file = canon.join(".claude").join("commands").join("cmd-foo.md");
        assert!(cmd_file.is_file());

        // user writes their OWN file into the same dir — must survive detach
        let user_file = canon.join(".claude").join("commands").join("user-own.md");
        std::fs::write(&user_file, "USER").expect("write user file");

        ProjectApplyService::detach(&state, &proj.id).expect("detach");

        assert!(
            !cmd_file.exists(),
            "owned command file must be removed on detach"
        );
        assert!(
            user_file.is_file(),
            "user file must NOT be touched on detach"
        );
        let chan = format!("project:{}", canon.to_string_lossy());
        assert_eq!(
            db.get_manifest_for_channel(&chan).unwrap().len(),
            0,
            "rows cleared"
        );
        // dir itself is left in place (brief §2: no directory removal on detach)
        assert!(
            cmd_file.parent().unwrap().is_dir(),
            ".claude/commands dir stays"
        );
    }

    #[test]
    #[serial]
    fn detach_skips_user_edited_owned_file() {
        let home = TempHome::new();
        crate::settings::reload_settings().ok();
        let db = Arc::new(Database::memory().expect("db"));
        let state = AppState::new(db.clone());
        seed_command(&db, "cmd-foo", "FOO");
        let (proj, canon) = project_at(
            home.home(),
            "work-c",
            ProfileContent {
                skills: vec![],
                commands: vec!["cmd-foo".into()],
                agents: vec![],
                mcp: vec![],
            },
        );
        db.save_project(&proj).expect("save");
        ProjectApplyService::apply(&state, &proj.id).expect("apply");

        // user EDITS the materialized file → hash no longer matches → detach must skip+warn
        let cmd_file = canon.join(".claude").join("commands").join("cmd-foo.md");
        std::fs::write(&cmd_file, "USER EDITED").expect("edit");
        ProjectApplyService::detach(&state, &proj.id).expect("detach");
        assert_eq!(
            std::fs::read_to_string(&cmd_file).unwrap(),
            "USER EDITED",
            "user edit preserved"
        );
    }

    #[test]
    #[serial]
    fn apply_materializes_skills_as_copy_and_detach_removes() {
        let home = TempHome::new();
        crate::settings::reload_settings().ok();
        let db = Arc::new(Database::memory().expect("db"));
        let state = AppState::new(db.clone());

        // SSOT skill + DB installed skill row keyed by directory
        let ssot = crate::services::SkillService::get_ssot_dir().expect("ssot");
        let sdir = ssot.join("proj-skill");
        std::fs::create_dir_all(&sdir).expect("mkdir");
        std::fs::write(sdir.join("SKILL.md"), "---\nname: proj-skill\n---\n# x\n").expect("write");
        // InstalledSkill does NOT derive Default (app_config.rs:167 derives only
        // Debug/Clone/Serialize/Deserialize), so spell out EVERY field — no `..Default::default()`.
        // SkillApps DOES derive Default, so SkillApps::default() is fine.
        let skill = crate::app_config::InstalledSkill {
            id: "local:proj-skill".into(),
            name: "proj-skill".into(),
            description: None,
            directory: "proj-skill".into(),
            repo_owner: None,
            repo_name: None,
            repo_branch: None,
            readme_url: None,
            apps: crate::app_config::SkillApps::default(),
            installed_at: 0,
            content_hash: None,
            updated_at: 0,
            tags: vec![],
        };
        db.save_skill(&skill).expect("save skill");

        let (proj, canon) = project_at(
            home.home(),
            "work-s",
            ProfileContent {
                skills: vec!["proj-skill".into()],
                commands: vec![],
                agents: vec![],
                mcp: vec![],
            },
        );
        db.save_project(&proj).expect("save");
        ProjectApplyService::apply(&state, &proj.id).expect("apply");

        let dest = canon.join(".claude").join("skills").join("proj-skill");
        assert!(dest.join("SKILL.md").is_file(), "skill copied");
        assert!(
            !std::fs::symlink_metadata(&dest)
                .unwrap()
                .file_type()
                .is_symlink(),
            "must be COPY"
        );

        let chan = format!("project:{}", canon.to_string_lossy());
        let rows = db.get_manifest_for_channel(&chan).unwrap();
        assert!(rows
            .iter()
            .any(|r| r.kind == "skill" && r.target_path == dest.to_string_lossy()));

        ProjectApplyService::detach(&state, &proj.id).expect("detach");
        assert!(!dest.exists(), "copied skill removed on detach");
    }

    #[test]
    #[serial]
    fn apply_does_not_reconcile_foreign_project_rows() {
        let home = TempHome::new();
        crate::settings::reload_settings().ok();
        let db = Arc::new(Database::memory().expect("db"));
        let state = AppState::new(db.clone());
        seed_command(&db, "cmd-foo", "FOO");

        let (proj, canon) = project_at(
            home.home(),
            "reused-path",
            ProfileContent {
                skills: vec![],
                commands: vec!["cmd-foo".into()],
                agents: vec![],
                mcp: vec![],
            },
        );
        db.save_project(&proj).expect("save");
        let chan = format!("project:{}", canon.to_string_lossy());

        // Simulate a STALE row from a DIFFERENT project at the same canonical path
        // (old repo deleted + re-cloned; project_id differs). Its target is a real user file.
        // This row's project_id ("proj:OLD") has NO matching projects row — that is only
        // insertable because project_id carries NO FK (design decision, Task 1); with the
        // FK + PRAGMA foreign_keys=ON this record_manifest_entry would fail and the test
        // could never exercise the §3.2 path-reuse protection.
        let foreign_target = canon.join(".claude").join("commands").join("foreign.md");
        std::fs::create_dir_all(foreign_target.parent().unwrap()).unwrap();
        std::fs::write(&foreign_target, "FOREIGN USER DATA").unwrap();
        db.record_manifest_entry(&ManifestEntry {
            id: 0,
            channel: chan.clone(),
            profile_id: None,
            project_id: Some("proj:OLD".into()),
            app_type: "claude".into(),
            target_path: foreign_target.to_string_lossy().to_string(),
            kind: "command".into(),
            content_hash: Some(crate::services::profile_render::content_hash(
                b"FOREIGN USER DATA",
            )),
            created_at: 0,
        })
        .unwrap();

        // Apply the NEW project — must NOT delete the foreign row's file (skip+warn), only manage its own.
        let res = ProjectApplyService::apply(&state, &proj.id).expect("apply");
        assert!(
            res.warnings.iter().any(|w| w.contains("different project")),
            "warn about foreign rows"
        );
        assert!(
            foreign_target.is_file(),
            "foreign project's file MUST survive (path-reuse protection)"
        );
        assert_eq!(
            std::fs::read_to_string(&foreign_target).unwrap(),
            "FOREIGN USER DATA"
        );
    }

    #[test]
    #[serial]
    fn prune_removes_channels_whose_path_is_gone() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().expect("db"));
        // a project-channel row pointing at a non-existent path, whose project_id
        // ("proj:gone") has no projects row — insertable only because project_id
        // carries NO FK (design decision, Task 1).
        db.record_manifest_entry(&ManifestEntry {
            id: 0,
            channel: "project:/no/such/path/xyz".into(),
            profile_id: None,
            project_id: Some("proj:gone".into()),
            app_type: "claude".into(),
            target_path: "/no/such/path/xyz/.claude/commands/a.md".into(),
            kind: "command".into(),
            content_hash: Some("h".into()),
            created_at: 0,
        })
        .unwrap();
        let pruned = ProjectApplyService::prune_orphan_project_channels(&db).expect("prune");
        assert!(pruned >= 1);
        assert_eq!(
            db.get_manifest_for_channel("project:/no/such/path/xyz")
                .unwrap()
                .len(),
            0
        );
    }

    #[test]
    #[serial]
    fn apply_prunes_path_gone_channel_before_reconcile() {
        // Brief §3.2: the path-gone prune MUST be reachable in production. apply()
        // runs prune_orphan_project_channels at its start, so a pre-seeded stale
        // channel (its dir deleted) is swept away by a real apply — proving the
        // prune is wired to a production entry point, not just its own unit test.
        let home = TempHome::new();
        crate::settings::reload_settings().ok();
        let db = Arc::new(Database::memory().expect("db"));
        let state = AppState::new(db.clone());
        seed_command(&db, "cmd-foo", "FOO");

        // a stale project-channel row whose path no longer exists (old repo deleted).
        let gone_chan = "project:/no/such/gone/repo";
        db.record_manifest_entry(&ManifestEntry {
            id: 0,
            channel: gone_chan.into(),
            profile_id: None,
            project_id: Some("proj:GONE".into()),
            app_type: "claude".into(),
            target_path: "/no/such/gone/repo/.claude/commands/old.md".into(),
            kind: "command".into(),
            content_hash: Some("h".into()),
            created_at: 0,
        })
        .unwrap();
        assert_eq!(db.get_manifest_for_channel(gone_chan).unwrap().len(), 1);

        // a real, live project at an existing path — apply it normally.
        let (proj, _canon) = project_at(
            home.home(),
            "live-repo",
            ProfileContent {
                skills: vec![],
                commands: vec!["cmd-foo".into()],
                agents: vec![],
                mcp: vec![],
            },
        );
        db.save_project(&proj).expect("save");
        ProjectApplyService::apply(&state, &proj.id).expect("apply");

        // apply-time cleanup pruned the path-gone channel (production entry point).
        assert_eq!(
            db.get_manifest_for_channel(gone_chan).unwrap().len(),
            0,
            "apply() must prune the stale path-gone channel via apply-time cleanup"
        );
    }
}
