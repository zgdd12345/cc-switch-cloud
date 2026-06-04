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

/// Single-source-of-truth names for the two MERGE manifest kinds (the
/// kind→enum-DEFER mitigation, 4b-3). These NAME the existing DB-stored string
/// literals; they do NOT change the stored values. Used by both the writer
/// (Self::row) and the reader (teardown_manifest_row) so a typo can't desync
/// the two sites.
const KIND_SETTINGS_MERGE: &str = "settings_merge";
const KIND_MCP_MERGE: &str = "mcp_merge";

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
        // For a settings_merge row, teardown_manifest_row runs reverse_merge HERE —
        // contract (d) pre-reapply ordering: it undoes our prior merge BEFORE the
        // materialize block (after the CLAUDE.md block, below) re-reads disk and
        // re-merges, so our prior write never becomes the new "user baseline".
        for r in &ours {
            Self::teardown_manifest_row(r, &app, &mut result.warnings)?;
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
                Some(&hash),
                None,
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
                Some(&hash),
                None,
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
                    Some(&hash),
                    None,
                ))?;
            }
        }

        // ---- project memory (CLAUDE.md, literal, whole-file; 4b-1) ----
        // Whole-file kind. Teardown is handled by the EXISTING else-arm above
        // (pre-delete) and in detach (remove_whole_file_if_owned) — NO new arm.
        // The prior project_memory file (if any) was already owned-deleted by the
        // pre-delete sweep, so a non-empty write here is a clean (re)materialize and
        // an empty claude_md correctly leaves nothing behind.
        let claude_md = &project.spec.dotfiles.claude_md;
        if !claude_md.is_empty() {
            let target = base.memory_file(&app);
            // prior_owned_hash is None: the pre-delete sweep already removed our
            // previously-owned file, so the only reason `target` still exists is a
            // user-created/edited file we must NOT clobber (write helper skips+warns).
            if let Some(hash) = write_project_whole_file(&target, claude_md, None)? {
                state.db.record_manifest_entry(&Self::row(
                    &channel,
                    &project.id,
                    &target,
                    "project_memory",
                    Some(&hash),
                    None,
                ))?;
            } else {
                result.warnings.push(format!(
                    "skipped project CLAUDE.md (user-edited/unmanaged file present): {}",
                    target.display()
                ));
            }
        }

        // ---- project settings.json (deep MERGE, ${VAR}; 4b-2, Claude only) ----
        // The pre-delete sweep already ran reverse_merge for our prior row
        // (contract d), so the on-disk settings.json is the USER baseline here.
        let settings_frag = &project.spec.dotfiles.settings;
        if !settings_frag.is_empty() {
            let target = base.settings_file();
            let var_map = Self::build_project_var_map(&state.db, &app, &project)?;
            let mut warns = Vec::new();
            let rendered = crate::services::profile_vars::substitute_vars(
                settings_frag,
                &var_map,
                /* json_escape = */ true,
                &mut warns,
            );
            for w in warns {
                result
                    .warnings
                    .push(format!("project settings.json render: {w}"));
            }
            match serde_json::from_str::<serde_json::Value>(&rendered) {
                Ok(frag) if !frag.is_object() => {
                    // settings.json must be a JSON object. A non-object root
                    // (array/string/number/bool/null) is an authoring error:
                    // refuse it (warn, no write, no row) instead of letting
                    // merge_with_snapshot's root-overwrite arm clobber the
                    // user's entire settings.json.
                    let ty = match frag {
                        serde_json::Value::Null => "null",
                        serde_json::Value::Bool(_) => "boolean",
                        serde_json::Value::Number(_) => "number",
                        serde_json::Value::String(_) => "string",
                        serde_json::Value::Array(_) => "array",
                        serde_json::Value::Object(_) => "object",
                    };
                    result.warnings.push(format!(
                        "project settings.json fragment must be a JSON object, got {ty}; skipping"
                    ));
                }
                Ok(frag) => {
                    // Load current disk as Option<Value> (None == skip-the-merge
                    // sentinel; NOT Value::Null — a valid on-disk `null` must not be
                    // misclassified, adversarial fix #3). absent file → Some({});
                    // present+valid → Some(v); present+invalid → None + warn.
                    let user_opt: Option<serde_json::Value> = if target.exists() {
                        match std::fs::read(&target)
                            .ok()
                            .and_then(|b| serde_json::from_slice(&b).ok())
                        {
                            Some(v) => Some(v),
                            None => {
                                result.warnings.push(format!(
                                    "project settings.json on disk is not a JSON value; skipping merge: {}",
                                    target.display()
                                ));
                                None
                            }
                        }
                    } else {
                        Some(serde_json::Value::Object(serde_json::Map::new()))
                    };
                    if let Some(mut user) = user_opt {
                        let mut owned = Vec::new();
                        let mut path = Vec::new();
                        crate::services::settings_merge::merge_with_snapshot(
                            &mut user, &frag, &mut path, &mut owned,
                        );
                        let bytes =
                            serde_json::to_vec_pretty(&crate::config::sort_json_keys(&user))
                                .map_err(|e| {
                                    AppError::Message(format!("serialize settings.json: {e}"))
                                })?;
                        atomic_write(&target, &bytes)?;
                        let env = crate::services::settings_merge::OwnedKeysEnvelope {
                            v: crate::services::settings_merge::OWNED_KEYS_VERSION,
                            keys: owned,
                        };
                        let owned_json = serde_json::to_string(&env)
                            .map_err(|e| AppError::Message(format!("serialize owned_keys: {e}")))?;
                        state.db.record_manifest_entry(&Self::row(
                            &channel,
                            &project.id,
                            &target,
                            KIND_SETTINGS_MERGE,
                            None,
                            Some(owned_json),
                        ))?;
                    }
                }
                Err(e) => result.warnings.push(format!(
                    "project settings.json fragment invalid JSON after render, skipping: {e}"
                )),
            }
        }

        // ---- project .mcp.json (mcpServers subtree, deep MERGE; 4b-3, Claude only) ----
        // REUSES the 4b-2 settings_merge engine verbatim. The pre-delete sweep already
        // reverse_merged our prior mcp_merge row, so the on-disk .mcp.json is the USER
        // baseline here. Merge ONLY the mcpServers subtree at the project ROOT
        // (base.mcp_file()), NOT the home-global ~/.claude.json. NO ${VAR}, NO cmd/c.
        {
            let servers = state.db.get_all_mcp_servers()?;
            let items: Vec<(String, Vec<String>)> = servers
                .values()
                .map(|s| (s.id.clone(), s.tags.clone()))
                .collect();
            let (want, w) =
                ProfileService::resolve_selectors("mcp", &project.spec.content.mcp, &items);
            result.warnings.extend(w);

            let mut mcp_map = serde_json::Map::new();
            for server in servers.values() {
                if !want.contains(&server.id) {
                    continue;
                }
                let mut spec = server.server.clone();
                if !spec.is_object() {
                    result.warnings.push(format!(
                        "project .mcp.json: server '{}' spec is not a JSON object; skipping",
                        server.id
                    ));
                    continue;
                }
                Self::strip_mcp_ui_fields(&mut spec);
                mcp_map.insert(server.id.clone(), spec);
            }

            // EMPTY-COLLAPSE: no servers resolve → record NO row, write NOTHING (the
            // pre-delete sweep already reversed our prior row → clean user-baseline file;
            // never an empty {"mcpServers":{}} stub).
            if !mcp_map.is_empty() {
                let target = base.mcp_file();
                let frag = serde_json::json!({ "mcpServers": serde_json::Value::Object(mcp_map) });
                // Option<Value> sentinel (None == skip), NOT Value::Null — mirror settings_merge.
                let user_opt: Option<serde_json::Value> = if target.exists() {
                    match std::fs::read(&target)
                        .ok()
                        .and_then(|b| serde_json::from_slice(&b).ok())
                    {
                        Some(v) => Some(v),
                        None => {
                            result.warnings.push(format!(
                                "project .mcp.json on disk is not a JSON value; skipping merge: {}",
                                target.display()
                            ));
                            None
                        }
                    }
                } else {
                    Some(serde_json::Value::Object(serde_json::Map::new()))
                };
                if let Some(mut user) = user_opt {
                    let mut owned = Vec::new();
                    let mut path = Vec::new();
                    crate::services::settings_merge::merge_with_snapshot(
                        &mut user, &frag, &mut path, &mut owned,
                    );
                    let bytes = serde_json::to_vec_pretty(&crate::config::sort_json_keys(&user))
                        .map_err(|e| AppError::Message(format!("serialize .mcp.json: {e}")))?;
                    atomic_write(&target, &bytes)?;
                    let env = crate::services::settings_merge::OwnedKeysEnvelope {
                        v: crate::services::settings_merge::OWNED_KEYS_VERSION,
                        keys: owned,
                    };
                    let owned_json = serde_json::to_string(&env)
                        .map_err(|e| AppError::Message(format!("serialize owned_keys: {e}")))?;
                    state.db.record_manifest_entry(&Self::row(
                        &channel,
                        &project.id,
                        &target,
                        KIND_MCP_MERGE,
                        None,
                        Some(owned_json),
                    ))?;
                }
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
            Self::teardown_manifest_row(r, &app, &mut result.warnings)?;
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

    /// Tear down ONE owned project-channel manifest row. Shared by apply()'s
    /// pre-delete sweep and detach() so the dispatch lives in exactly one place.
    /// Dispatch by kind:
    /// - `skill` → dir-aware, content-hash-safe removal (never `remove_dir_all` a real user dir).
    /// - `settings_merge` / `mcp_merge` → per-leaf `reverse_merge` (restores/removes only OUR leaves).
    /// - else (`command`/`agent`/`project_memory`/`whole_file`) → hash-gated `remove_whole_file_if_owned`.
    ///
    /// CRITICAL (contract c): the `settings_merge`/`mcp_merge` arm MUST stay before
    /// the `else`, otherwise a merge row would reach `remove_whole_file_if_owned` and
    /// whole-file delete the user's `settings.json`/`.mcp.json`. Keeping the dispatch
    /// in one fn single-sources that invariant: the `settings_merge` (4b-2) and
    /// `mcp_merge` (4b-3) kinds SHARE one arm here, not two loops.
    /// The caller owns the foreign-`project_id` guard (apply partitions upstream;
    /// detach `continue`s before calling this).
    fn teardown_manifest_row(
        r: &ManifestEntry,
        app: &AppType,
        warnings: &mut Vec<String>,
    ) -> Result<(), AppError> {
        if r.kind == "skill" {
            if let Some(parent) = Path::new(&r.target_path).parent() {
                let dir_name = Path::new(&r.target_path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                crate::services::SkillService::remove_from_project_dir(&dir_name, parent, app)
                    .map_err(|e| AppError::Message(format!("project skill remove failed: {e}")))?;
            }
        } else if r.kind == KIND_SETTINGS_MERGE || r.kind == KIND_MCP_MERGE {
            // settings_merge (4b-2) + mcp_merge (4b-3): per-leaf reverse_merge
            // restores/removes ONLY the leaves we wrote, preserving user-authored
            // keys. Same fail-closed engine; only the kind discriminant differs.
            // CRITICAL: this MUST precede the catch-all else — a merge row in the
            // else would whole-file delete the user's settings.json/.mcp.json.
            crate::services::settings_merge::reverse_merge(
                std::path::Path::new(&r.target_path),
                r.owned_keys.as_deref(),
                warnings,
            )?;
        } else {
            crate::services::profile_render::remove_whole_file_if_owned(
                &r.target_path,
                r.content_hash.as_deref(),
            )?;
        }
        Ok(())
    }

    /// Strip UI/metadata fields from an MCP server spec before writing it to a project
    /// .mcp.json. Mirrors claude_mcp.rs::set_mcp_servers_map's inline 8-field strip,
    /// minus Windows cmd/c wrapping (omitted for cross-machine repo .mcp.json).
    fn strip_mcp_ui_fields(spec: &mut serde_json::Value) {
        if let Some(obj) = spec.as_object_mut() {
            if let Some(inner) = obj.remove("server") {
                if inner.is_object() {
                    *spec = inner;
                }
            }
        }
        if let Some(obj) = spec.as_object_mut() {
            for k in [
                "enabled",
                "source",
                "id",
                "name",
                "description",
                "tags",
                "homepage",
                "docs",
            ] {
                obj.remove(k);
            }
        }
    }

    fn row(
        channel: &str,
        project_id: &str,
        target: &Path,
        kind: &str,
        content_hash: Option<&str>,
        owned_keys: Option<String>,
    ) -> ManifestEntry {
        ManifestEntry {
            id: 0,
            channel: channel.to_string(),
            profile_id: None,
            project_id: Some(project_id.to_string()),
            app_type: AppType::Claude.as_str().to_string(),
            target_path: target.to_string_lossy().to_string(),
            kind: kind.to_string(),
            content_hash: content_hash.map(|h| h.to_string()),
            owned_keys,
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

    /// Build the layered `${VAR}` map for a PROJECT (contract g). Layers low→high:
    /// 1. allowlisted process env (profile_vars::ENV_ALLOWLIST_PREFIXES),
    /// 2. active provider settings_config.env (get_effective_current_provider),
    /// 3. project.spec.vars (TOP).
    ///
    /// Does NOT call profile_vars::build_var_map — that would inject the GLOBAL
    /// active profile's vars, which must NOT leak into a project render.
    /// reverse_merge does NOT re-render, so a value change between apply and
    /// detach cannot defeat teardown (teardown is a pure fn of the stored snapshot).
    pub fn build_project_var_map(
        db: &crate::database::Database,
        app_type: &AppType,
        project: &crate::app_config::Project,
    ) -> Result<crate::services::profile_vars::VarMap, AppError> {
        use indexmap::IndexMap;
        let mut map: IndexMap<String, String> = IndexMap::new();

        // Layer 1: allowlisted process env.
        for (k, v) in std::env::vars() {
            if crate::services::profile_vars::ENV_ALLOWLIST_PREFIXES
                .iter()
                .any(|prefix| k.starts_with(prefix))
            {
                map.insert(k, v);
            }
        }

        // Layer 2: active provider env (if any).
        if let Some(provider_id) = crate::settings::get_effective_current_provider(db, app_type)? {
            if let Some(provider) = db.get_provider_by_id(&provider_id, app_type.as_str())? {
                if let Some(env_obj) = provider
                    .settings_config
                    .get("env")
                    .and_then(|v| v.as_object())
                {
                    for (k, v) in env_obj {
                        if let Some(value) = crate::services::profile_vars::coerce_value(v) {
                            map.insert(k.clone(), value);
                        }
                    }
                }
            }
        }

        // Layer 3: project spec.vars (highest precedence).
        for (k, v) in &project.spec.vars {
            if let Some(value) = crate::services::profile_vars::coerce_value(v) {
                map.insert(k.clone(), value);
            }
        }

        Ok(crate::services::profile_vars::VarMap::from_index_map(map))
    }
}

/// Hash-gated whole-file write for a project dotfile (4b-1). Mirrors
/// `profile_render::render_whole_file`'s OWNERSHIP contract but is base-agnostic:
/// it does NOT use `validate_rel_path` / `~/.claude` and does NOT stamp a manifest
/// row (the caller records via `Self::row`). The abs path is already past
/// `ProjectBase::resolve`'s HOME/symlink gate and the filename is a compile-time
/// constant ("CLAUDE.md"), so there is no traversal risk.
///
/// - exists AND (prior_owned_hash is None OR disk_hash != prior_owned_hash):
///   skip + warn, return Ok(None) (never overwrite a user-edited/unmanaged file).
/// - absent, OR disk_hash == prior_owned_hash: atomic_write the content, return
///   Ok(Some(sha256(content))).
fn write_project_whole_file(
    abs_path: &Path,
    content: &str,
    prior_owned_hash: Option<&str>,
) -> Result<Option<String>, AppError> {
    if abs_path.exists() {
        let disk = std::fs::read(abs_path).map_err(|e| AppError::io(abs_path, e))?;
        let disk_hash = crate::services::profile_render::content_hash(&disk);
        let owned = matches!(prior_owned_hash, Some(h) if h == disk_hash);
        if !owned {
            log::warn!(
                "拒绝覆盖未托管/被用户编辑的项目文件: {}",
                abs_path.display()
            );
            return Ok(None);
        }
    }
    atomic_write(abs_path, content.as_bytes())?;
    Ok(Some(crate::services::profile_render::content_hash(
        content.as_bytes(),
    )))
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
    fn seed_mcp(db: &Database, id: &str, server: serde_json::Value, tags: Vec<String>) {
        let s = crate::app_config::McpServer {
            id: id.into(),
            name: id.into(),
            server,
            apps: crate::app_config::McpApps::default(),
            description: None,
            homepage: None,
            docs: None,
            tags,
        };
        db.save_mcp_server(&s).expect("save mcp server");
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
                dotfiles: Default::default(),
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
            owned_keys: None,
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
            owned_keys: None,
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
            owned_keys: None,
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

    #[test]
    #[serial]
    fn write_project_whole_file_writes_when_absent() {
        let _home = TempHome::new();
        let dir = TempDir::new().expect("tmp");
        let target = dir.path().join("CLAUDE.md");
        // absent → write, returns Some(hash) of the content.
        let h = super::write_project_whole_file(&target, "# memory", None).expect("write");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "# memory");
        assert_eq!(
            h,
            Some(crate::services::profile_render::content_hash(b"# memory"))
        );
    }

    #[test]
    #[serial]
    fn write_project_whole_file_skips_user_edited_file() {
        let _home = TempHome::new();
        let dir = TempDir::new().expect("tmp");
        let target = dir.path().join("CLAUDE.md");
        // file exists with content we do NOT own (prior hash None) → skip + Ok(None).
        std::fs::write(&target, "USER WROTE THIS").expect("seed");
        let r = super::write_project_whole_file(&target, "MANAGED", None).expect("skip");
        assert_eq!(r, None, "must skip an unmanaged file");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "USER WROTE THIS");

        // file exists, prior hash present but disk hash differs (user edited) → skip.
        let prior = crate::services::profile_render::content_hash(b"OLD MANAGED");
        let r2 = super::write_project_whole_file(&target, "MANAGED", Some(&prior)).expect("skip2");
        assert_eq!(r2, None, "disk_hash != prior_owned_hash must skip");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "USER WROTE THIS");
    }

    #[test]
    #[serial]
    fn write_project_whole_file_overwrites_when_owned() {
        let _home = TempHome::new();
        let dir = TempDir::new().expect("tmp");
        let target = dir.path().join("CLAUDE.md");
        std::fs::write(&target, "OLD MANAGED").expect("seed");
        // disk_hash == prior_owned_hash → we own it → overwrite, return new hash.
        let prior = crate::services::profile_render::content_hash(b"OLD MANAGED");
        let h =
            super::write_project_whole_file(&target, "NEW MANAGED", Some(&prior)).expect("write");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "NEW MANAGED");
        assert_eq!(
            h,
            Some(crate::services::profile_render::content_hash(
                b"NEW MANAGED"
            ))
        );
    }

    #[test]
    #[serial]
    fn apply_writes_claude_md_at_root_and_records_project_memory() {
        let home = TempHome::new();
        crate::settings::reload_settings().ok();
        let db = Arc::new(Database::memory().expect("db"));
        let state = AppState::new(db.clone());
        let (mut proj, canon) = project_at(home.home(), "memroot", ProfileContent::default());
        proj.spec.dotfiles.claude_md = "# Project memory\nbe terse\n".into();
        db.save_project(&proj).expect("save");

        let res = ProjectApplyService::apply(&state, &proj.id).expect("apply");
        assert!(res.warnings.is_empty(), "no warnings: {:?}", res.warnings);

        // CLAUDE.md materialized at the project ROOT — NOT under .claude/.
        let root_file = canon.join("CLAUDE.md");
        assert_eq!(
            std::fs::read_to_string(&root_file).unwrap(),
            "# Project memory\nbe terse\n"
        );
        assert!(
            !canon.join(".claude").join("CLAUDE.md").exists(),
            "must NOT be written under .claude/"
        );

        // one project_memory manifest row on this channel, hash = sha256(content).
        let chan = format!("project:{}", canon.to_string_lossy());
        let rows = db.get_manifest_for_channel(&chan).unwrap();
        let mem: Vec<_> = rows.iter().filter(|r| r.kind == "project_memory").collect();
        assert_eq!(mem.len(), 1, "exactly one project_memory row");
        assert_eq!(mem[0].project_id.as_deref(), Some(proj.id.as_str()));
        assert_eq!(mem[0].target_path, root_file.to_string_lossy());
        assert_eq!(
            mem[0].content_hash.as_deref(),
            Some(
                crate::services::profile_render::content_hash(b"# Project memory\nbe terse\n")
                    .as_str()
            )
        );
    }

    #[test]
    #[serial]
    fn apply_empty_claude_md_writes_no_file_and_no_row() {
        let home = TempHome::new();
        crate::settings::reload_settings().ok();
        let db = Arc::new(Database::memory().expect("db"));
        let state = AppState::new(db.clone());
        let (proj, canon) = project_at(home.home(), "memnone", ProfileContent::default());
        // dotfiles.claude_md left empty by Default
        db.save_project(&proj).expect("save");
        ProjectApplyService::apply(&state, &proj.id).expect("apply");

        assert!(
            !canon.join("CLAUDE.md").exists(),
            "empty claude_md → no file"
        );
        let chan = format!("project:{}", canon.to_string_lossy());
        let rows = db.get_manifest_for_channel(&chan).unwrap();
        assert!(
            !rows.iter().any(|r| r.kind == "project_memory"),
            "no project_memory row for empty claude_md"
        );
    }

    #[test]
    #[serial]
    fn reapply_is_idempotent_and_user_edited_claude_md_survives() {
        let home = TempHome::new();
        crate::settings::reload_settings().ok();
        let db = Arc::new(Database::memory().expect("db"));
        let state = AppState::new(db.clone());
        let (mut proj, canon) = project_at(home.home(), "memreapply", ProfileContent::default());
        proj.spec.dotfiles.claude_md = "# v1\n".into();
        db.save_project(&proj).expect("save");
        ProjectApplyService::apply(&state, &proj.id).expect("apply 1");

        let root_file = canon.join("CLAUDE.md");
        let chan = format!("project:{}", canon.to_string_lossy());

        // clean re-apply with NEW owned content: pre-delete removes the owned v1
        // (hash matches), then the new write lands → still exactly one row.
        proj.spec.dotfiles.claude_md = "# v2\n".into();
        db.save_project(&proj).expect("save v2");
        ProjectApplyService::apply(&state, &proj.id).expect("apply 2");
        assert_eq!(std::fs::read_to_string(&root_file).unwrap(), "# v2\n");
        let mem = db
            .get_manifest_for_channel(&chan)
            .unwrap()
            .into_iter()
            .filter(|r| r.kind == "project_memory")
            .count();
        assert_eq!(mem, 1, "re-apply stays idempotent (one row)");

        // USER edits CLAUDE.md → next apply must NOT clobber it: pre-delete skips
        // (hash mismatch leaves the file), and the new write also skips (file
        // exists with a non-matching prior hash) → user edit preserved + warn.
        std::fs::write(&root_file, "# USER OWNS THIS NOW\n").expect("user edit");
        ProjectApplyService::apply(&state, &proj.id).expect("apply 3");
        assert_eq!(
            std::fs::read_to_string(&root_file).unwrap(),
            "# USER OWNS THIS NOW\n",
            "user-edited CLAUDE.md must survive re-apply"
        );
    }

    #[test]
    #[serial]
    fn detach_removes_owned_claude_md_via_existing_else_arm() {
        let home = TempHome::new();
        crate::settings::reload_settings().ok();
        let db = Arc::new(Database::memory().expect("db"));
        let state = AppState::new(db.clone());
        let (mut proj, canon) = project_at(home.home(), "memdetach", ProfileContent::default());
        proj.spec.dotfiles.claude_md = "# owned memory\n".into();
        db.save_project(&proj).expect("save");
        ProjectApplyService::apply(&state, &proj.id).expect("apply");

        let root_file = canon.join("CLAUDE.md");
        assert!(root_file.is_file(), "applied first");

        // detach: the project_memory row falls into the existing ELSE arm
        // (remove_whole_file_if_owned) — owned (hash matches) → removed.
        ProjectApplyService::detach(&state, &proj.id).expect("detach");
        assert!(!root_file.exists(), "owned CLAUDE.md removed on detach");
        let chan = format!("project:{}", canon.to_string_lossy());
        assert_eq!(
            db.get_manifest_for_channel(&chan).unwrap().len(),
            0,
            "rows cleared"
        );
    }

    #[test]
    #[serial]
    fn detach_preserves_user_edited_claude_md() {
        let home = TempHome::new();
        crate::settings::reload_settings().ok();
        let db = Arc::new(Database::memory().expect("db"));
        let state = AppState::new(db.clone());
        let (mut proj, canon) = project_at(home.home(), "memdetach2", ProfileContent::default());
        proj.spec.dotfiles.claude_md = "# owned\n".into();
        db.save_project(&proj).expect("save");
        ProjectApplyService::apply(&state, &proj.id).expect("apply");

        // user edits the materialized CLAUDE.md → hash no longer matches the row →
        // remove_whole_file_if_owned must skip+warn → file survives detach.
        let root_file = canon.join("CLAUDE.md");
        std::fs::write(&root_file, "# USER EDITED\n").expect("edit");
        ProjectApplyService::detach(&state, &proj.id).expect("detach");
        assert_eq!(
            std::fs::read_to_string(&root_file).unwrap(),
            "# USER EDITED\n",
            "user-edited CLAUDE.md must NOT be deleted on detach"
        );
    }

    #[test]
    #[serial]
    fn build_project_var_map_precedence_spec_over_provider_over_process() {
        let home = TempHome::new();
        crate::settings::reload_settings().ok();
        std::env::set_var("ANTHROPIC_SHARED", "from_process");
        std::env::set_var("AGENTHUB_ONLY_PROCESS", "process_only");
        std::env::set_var("RANDOM_HOST_SECRET", "leak");

        let db = Arc::new(Database::memory().expect("db"));
        let app = AppType::Claude;

        let provider = crate::provider::Provider::with_id(
            "prov1".to_string(),
            "Prov 1".to_string(),
            serde_json::json!({
                "env": {
                    "ANTHROPIC_SHARED": "from_provider",
                    "ANTHROPIC_PROVIDER_KEY": "pk"
                }
            }),
            None,
        );
        db.save_provider(app.as_str(), &provider)
            .expect("save provider");
        db.set_current_provider(app.as_str(), "prov1")
            .expect("set current");

        let (mut proj, _canon) = project_at(home.home(), "varproj", ProfileContent::default());
        proj.spec.vars.insert(
            "ANTHROPIC_SHARED".to_string(),
            serde_json::Value::String("from_project".to_string()),
        );
        db.save_project(&proj).expect("save");

        let app_arc = AppType::Claude;
        let stored = db.get_project(&proj.id).unwrap().unwrap();
        let map =
            ProjectApplyService::build_project_var_map(&db, &app_arc, &stored).expect("build map");

        assert_eq!(
            map.get("ANTHROPIC_SHARED"),
            Some("from_project"),
            "project.spec.vars wins"
        );
        assert_eq!(
            map.get("ANTHROPIC_PROVIDER_KEY"),
            Some("pk"),
            "provider env contributes"
        );
        assert_eq!(
            map.get("AGENTHUB_ONLY_PROCESS"),
            Some("process_only"),
            "allowlisted process env"
        );
        assert_eq!(
            map.get("RANDOM_HOST_SECRET"),
            None,
            "non-allowlisted env filtered"
        );

        std::env::remove_var("ANTHROPIC_SHARED");
        std::env::remove_var("AGENTHUB_ONLY_PROCESS");
        std::env::remove_var("RANDOM_HOST_SECRET");
    }

    #[test]
    #[serial]
    fn build_project_var_map_does_not_leak_global_profile_vars() {
        // The global ACTIVE profile may carry spec.vars; build_project_var_map MUST
        // NOT include them (it never calls build_var_map). Only the PROJECT's own
        // spec.vars (+ provider env + allowlisted process env) feed the map.
        let home = TempHome::new();
        crate::settings::reload_settings().ok();
        let db = Arc::new(Database::memory().expect("db"));
        let app = AppType::Claude;

        // an active profile with a var that MUST NOT leak.
        let mut pvars = serde_json::Map::new();
        pvars.insert(
            "ANTHROPIC_PROFILE_ONLY".to_string(),
            serde_json::Value::String("LEAKED".to_string()),
        );
        let profile = crate::app_config::Profile {
            id: "local:claude:Active".into(),
            app_type: "claude".into(),
            name: "Active".into(),
            description: None,
            is_active: true,
            current_provider_id: None,
            spec: crate::app_config::ProfileSpec {
                content: Default::default(),
                vars: pvars,
            },
            sort_index: 0,
            created_at: 0,
        };
        db.save_profile(&profile).expect("save profile");

        let (proj, _canon) = project_at(home.home(), "noleakproj", ProfileContent::default());
        db.save_project(&proj).expect("save");
        let stored = db.get_project(&proj.id).unwrap().unwrap();
        let map = ProjectApplyService::build_project_var_map(&db, &app, &stored).expect("map");
        assert_eq!(
            map.get("ANTHROPIC_PROFILE_ONLY"),
            None,
            "global profile vars must NOT leak"
        );
    }

    fn merge_proj(home: &Path, sub: &str, frag: &str) -> (Project, std::path::PathBuf) {
        let (mut proj, canon) = project_at(home, sub, ProfileContent::default());
        proj.spec.dotfiles.settings = frag.into();
        (proj, canon)
    }

    #[test]
    #[serial]
    fn apply_merges_settings_and_preserves_unrelated_user_keys() {
        let home = TempHome::new();
        crate::settings::reload_settings().ok();
        let db = Arc::new(Database::memory().expect("db"));
        let state = AppState::new(db.clone());
        let (proj, canon) = merge_proj(home.home(), "merge-a", r#"{"model": "claude-x"}"#);
        db.save_project(&proj).expect("save");

        // user already has a settings.json with their OWN key.
        let target = canon.join(".claude").join("settings.json");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, r#"{"userKept": true}"#).unwrap();

        let res = ProjectApplyService::apply(&state, &proj.id).expect("apply");
        assert!(res.warnings.is_empty(), "no warnings: {:?}", res.warnings);

        let disk: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
        assert_eq!(disk["model"], serde_json::json!("claude-x"), "frag merged");
        assert_eq!(
            disk["userKept"],
            serde_json::json!(true),
            "user key survives"
        );

        // exactly one settings_merge row with a parseable v1 envelope, content_hash None.
        let chan = format!("project:{}", canon.to_string_lossy());
        let rows = db.get_manifest_for_channel(&chan).unwrap();
        let m: Vec<_> = rows.iter().filter(|r| r.kind == "settings_merge").collect();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].content_hash, None, "merge rows carry NO content_hash");
        let env: crate::services::settings_merge::OwnedKeysEnvelope =
            serde_json::from_str(m[0].owned_keys.as_deref().expect("owned_keys"))
                .expect("parse env");
        assert_eq!(env.v, crate::services::settings_merge::OWNED_KEYS_VERSION);
        assert!(env.keys.iter().any(|k| k.path == vec!["model".to_string()]));
    }

    #[test]
    #[serial]
    fn apply_bad_fragment_warns_and_writes_no_file_no_row() {
        let home = TempHome::new();
        crate::settings::reload_settings().ok();
        let db = Arc::new(Database::memory().expect("db"));
        let state = AppState::new(db.clone());
        // fragment is not valid JSON even after render → warn, no write, no row.
        let (proj, canon) = merge_proj(home.home(), "merge-bad", r#"{ not: json"#);
        db.save_project(&proj).expect("save");
        let res = ProjectApplyService::apply(&state, &proj.id).expect("apply");
        assert!(res
            .warnings
            .iter()
            .any(|w| w.contains("invalid JSON after render")));
        assert!(
            !canon.join(".claude").join("settings.json").exists(),
            "no file written"
        );
        let chan = format!("project:{}", canon.to_string_lossy());
        assert!(
            !db.get_manifest_for_channel(&chan)
                .unwrap()
                .iter()
                .any(|r| r.kind == "settings_merge"),
            "no settings_merge row for a bad fragment"
        );
    }

    #[test]
    #[serial]
    fn apply_malformed_disk_skips_merge_byte_identical_no_row() {
        // M11: disk settings.json is invalid JSON → skip the merge, leave bytes
        // identical, warn, NO row.
        let home = TempHome::new();
        crate::settings::reload_settings().ok();
        let db = Arc::new(Database::memory().expect("db"));
        let state = AppState::new(db.clone());
        let (proj, canon) = merge_proj(home.home(), "merge-mal", r#"{"model": "x"}"#);
        db.save_project(&proj).expect("save");
        let target = canon.join(".claude").join("settings.json");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, b"{ broken : json").unwrap();

        let res = ProjectApplyService::apply(&state, &proj.id).expect("apply");
        assert!(res.warnings.iter().any(|w| w.contains("not a JSON value")));
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"{ broken : json",
            "byte-identical"
        );
        let chan = format!("project:{}", canon.to_string_lossy());
        assert!(
            !db.get_manifest_for_channel(&chan)
                .unwrap()
                .iter()
                .any(|r| r.kind == "settings_merge"),
            "no row when disk skip"
        );
    }

    #[test]
    #[serial]
    fn apply_non_object_fragment_warns_and_writes_no_file_no_row() {
        // A fragment whose ROOT is a non-object (array, string, ...) must be
        // REFUSED: warn, do NOT clobber the user's settings.json, NO row.
        for frag in [r#"[1,2,3]"#, r#""x""#] {
            let home = TempHome::new();
            crate::settings::reload_settings().ok();
            let db = Arc::new(Database::memory().expect("db"));
            let state = AppState::new(db.clone());
            let (proj, canon) = merge_proj(home.home(), "merge-nonobj", frag);
            db.save_project(&proj).expect("save");

            // user already has a settings.json that must survive untouched.
            let target = canon.join(".claude").join("settings.json");
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            let original = br#"{"userKept": true}"#;
            std::fs::write(&target, original).unwrap();

            let res = ProjectApplyService::apply(&state, &proj.id).expect("apply");
            assert!(
                res.warnings
                    .iter()
                    .any(|w| w.contains("must be a JSON object")),
                "warns about non-object root for frag {frag:?}: {:?}",
                res.warnings
            );
            assert_eq!(
                std::fs::read(&target).unwrap(),
                original,
                "user settings.json is byte-identical (not clobbered) for frag {frag:?}"
            );
            let chan = format!("project:{}", canon.to_string_lossy());
            assert!(
                !db.get_manifest_for_channel(&chan)
                    .unwrap()
                    .iter()
                    .any(|r| r.kind == "settings_merge"),
                "no settings_merge row for a non-object fragment {frag:?}"
            );
        }
    }

    #[test]
    #[serial]
    fn apply_renders_vars_in_settings_fragment() {
        let home = TempHome::new();
        crate::settings::reload_settings().ok();
        let db = Arc::new(Database::memory().expect("db"));
        let state = AppState::new(db.clone());
        let (mut proj, canon) =
            merge_proj(home.home(), "merge-var", r#"{"model": "${MODEL_NAME}"}"#);
        proj.spec.vars.insert(
            "MODEL_NAME".to_string(),
            serde_json::Value::String("claude-from-var".to_string()),
        );
        db.save_project(&proj).expect("save");
        ProjectApplyService::apply(&state, &proj.id).expect("apply");
        let target = canon.join(".claude").join("settings.json");
        let disk: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
        assert_eq!(
            disk["model"],
            serde_json::json!("claude-from-var"),
            "${{VAR}} rendered"
        );
    }

    #[test]
    #[serial]
    fn reapply_settings_is_idempotent_and_prior_is_true_user_baseline() {
        let home = TempHome::new();
        crate::settings::reload_settings().ok();
        let db = Arc::new(Database::memory().expect("db"));
        let state = AppState::new(db.clone());
        // frag overwrites a key the USER already has → prior must be the user's value.
        let (proj, canon) = merge_proj(home.home(), "merge-reapply", r#"{"model": "ours"}"#);
        db.save_project(&proj).expect("save");
        let target = canon.join(".claude").join("settings.json");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, r#"{"model": "USER_ORIGINAL"}"#).unwrap();

        ProjectApplyService::apply(&state, &proj.id).expect("apply 1");
        ProjectApplyService::apply(&state, &proj.id).expect("apply 2");

        // exactly ONE settings_merge row (pre-delete reverse_merge ran first, then
        // re-merge recorded a fresh single row).
        let chan = format!("project:{}", canon.to_string_lossy());
        let rows = db.get_manifest_for_channel(&chan).unwrap();
        let m: Vec<_> = rows.iter().filter(|r| r.kind == "settings_merge").collect();
        assert_eq!(
            m.len(),
            1,
            "double-apply stays idempotent (one settings_merge row)"
        );

        // the recorded prior for [model] must be the TRUE user original — the
        // pre-delete reverse restored "USER_ORIGINAL" before the re-merge snapshotted.
        let env: crate::services::settings_merge::OwnedKeysEnvelope =
            serde_json::from_str(m[0].owned_keys.as_deref().unwrap()).unwrap();
        let model_leaf = env
            .keys
            .iter()
            .find(|k| k.path == vec!["model".to_string()])
            .expect("model leaf");
        assert!(model_leaf.prior.present);
        assert_eq!(
            model_leaf.prior.value,
            Some(serde_json::json!("USER_ORIGINAL")),
            "prior must be the TRUE user baseline, NOT our prior write (contract d)"
        );

        // disk reflects our value after re-apply.
        let disk: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
        assert_eq!(disk["model"], serde_json::json!("ours"));
    }

    #[test]
    #[serial]
    fn m10_settings_file_is_never_whole_deleted_on_reapply_or_detach() {
        // M10 (CRITICAL): a bound project with a settings fragment + CLAUDE.md.
        // After apply: a settings_merge row exists. (a) re-apply: settings.json
        // STILL EXISTS with the user key. (b) detach: settings.json STILL EXISTS
        // (user keys reversed) — the merge row must NOT fall through the else and
        // get whole-file deleted.
        let home = TempHome::new();
        crate::settings::reload_settings().ok();
        let db = Arc::new(Database::memory().expect("db"));
        let state = AppState::new(db.clone());
        let (mut proj, canon) = merge_proj(home.home(), "m10", r#"{"model": "ours"}"#);
        proj.spec.dotfiles.claude_md = "# mem\n".into();
        db.save_project(&proj).expect("save");

        let target = canon.join(".claude").join("settings.json");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, r#"{"userKept": true}"#).unwrap();

        ProjectApplyService::apply(&state, &proj.id).expect("apply");
        let chan = format!("project:{}", canon.to_string_lossy());
        assert!(
            db.get_manifest_for_channel(&chan)
                .unwrap()
                .iter()
                .any(|r| r.kind == "settings_merge"),
            "a settings_merge row must exist after apply"
        );

        // (a) re-apply: file STILL exists with the user key.
        ProjectApplyService::apply(&state, &proj.id).expect("re-apply");
        assert!(
            target.exists(),
            "settings.json must NOT be whole-deleted by re-apply"
        );
        let disk_a: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
        assert_eq!(
            disk_a["userKept"],
            serde_json::json!(true),
            "user key survives re-apply"
        );

        // (b) detach: file STILL exists; our [model] leaf reversed; user key intact.
        ProjectApplyService::detach(&state, &proj.id).expect("detach");
        assert!(
            target.exists(),
            "settings.json must STILL EXIST after detach (M10)"
        );
        let disk_b: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
        assert_eq!(
            disk_b["userKept"],
            serde_json::json!(true),
            "user key survives detach"
        );
        assert!(
            disk_b.get("model").is_none(),
            "our inserted leaf removed on detach (M7)"
        );
    }

    #[test]
    #[serial]
    fn detach_restores_overwritten_user_key_and_keeps_new_user_key() {
        // M7 + M9: frag overwrote user's existing [model]; user later added a brand
        // new key never in the frag. Detach restores [model] to the user original
        // and leaves the brand-new key untouched.
        let home = TempHome::new();
        crate::settings::reload_settings().ok();
        let db = Arc::new(Database::memory().expect("db"));
        let state = AppState::new(db.clone());
        let (proj, canon) = merge_proj(home.home(), "m7m9", r#"{"model": "ours"}"#);
        db.save_project(&proj).expect("save");
        let target = canon.join(".claude").join("settings.json");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, r#"{"model": "USER_ORIGINAL"}"#).unwrap();

        ProjectApplyService::apply(&state, &proj.id).expect("apply");

        // user adds a brand-new key that was NEVER in our fragment (M9).
        let mut cur: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
        cur["userBrandNew"] = serde_json::json!("added later");
        std::fs::write(&target, serde_json::to_vec_pretty(&cur).unwrap()).unwrap();

        ProjectApplyService::detach(&state, &proj.id).expect("detach");
        let disk: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
        assert_eq!(
            disk["model"],
            serde_json::json!("USER_ORIGINAL"),
            "overwritten user key restored (M7)"
        );
        assert_eq!(
            disk["userBrandNew"],
            serde_json::json!("added later"),
            "brand-new user key survives (M9)"
        );

        // rows cleared.
        let chan = format!("project:{}", canon.to_string_lossy());
        assert_eq!(
            db.get_manifest_for_channel(&chan).unwrap().len(),
            0,
            "rows cleared on detach"
        );
    }

    #[test]
    fn strip_mcp_ui_fields_drops_all_eight_and_keeps_connection() {
        let mut spec = serde_json::json!({
            "type": "stdio",
            "command": "node",
            "args": ["server.js"],
            "env": { "K": "v" },
            "enabled": true,
            "source": "registry",
            "id": "srv1",
            "name": "Server One",
            "description": "desc",
            "tags": ["a", "b"],
            "homepage": "https://h",
            "docs": "https://d"
        });
        ProjectApplyService::strip_mcp_ui_fields(&mut spec);
        assert_eq!(
            spec,
            serde_json::json!({
                "type": "stdio",
                "command": "node",
                "args": ["server.js"],
                "env": { "K": "v" }
            }),
            "only the connection fields survive"
        );
    }

    #[test]
    fn strip_mcp_ui_fields_noop_on_clean_stdio_spec() {
        let mut spec = serde_json::json!({
            "type": "stdio", "command": "uvx", "args": ["x"]
        });
        let before = spec.clone();
        ProjectApplyService::strip_mcp_ui_fields(&mut spec);
        assert_eq!(spec, before, "already-clean spec is unchanged");
    }

    #[test]
    fn strip_mcp_ui_fields_keeps_http_url() {
        let mut spec = serde_json::json!({
            "type": "http", "url": "https://mcp.example/api", "name": "X", "enabled": true
        });
        ProjectApplyService::strip_mcp_ui_fields(&mut spec);
        assert_eq!(
            spec,
            serde_json::json!({ "type": "http", "url": "https://mcp.example/api" }),
            "http/sse url survives, UI fields stripped"
        );
    }

    #[test]
    fn strip_mcp_ui_fields_unwraps_legacy_server_wrapper() {
        // legacy {"server":{..real..}, "name":..} → unwrap to the inner spec, then strip.
        let mut spec = serde_json::json!({
            "name": "wrapped",
            "enabled": true,
            "server": { "type": "stdio", "command": "go", "args": ["run"] }
        });
        ProjectApplyService::strip_mcp_ui_fields(&mut spec);
        assert_eq!(
            spec,
            serde_json::json!({ "type": "stdio", "command": "go", "args": ["run"] }),
            "legacy server wrapper unwrapped and stripped"
        );
    }

    #[test]
    fn strip_mcp_ui_fields_non_object_server_value_does_not_panic_or_lose_fields() {
        // a `server` value that is NOT an object is removed (dropped, never
        // reinserted, since we only reassign `*spec` when the unwrapped value
        // is an object); the connection fields survive and there is no panic.
        let mut spec = serde_json::json!({
            "type": "stdio", "command": "x", "server": "not-an-object", "enabled": true
        });
        ProjectApplyService::strip_mcp_ui_fields(&mut spec);
        assert_eq!(
            spec,
            serde_json::json!({ "type": "stdio", "command": "x" }),
            "non-object `server` is removed (dropped, never reinserted); connection fields survive, no panic"
        );
    }

    #[test]
    #[serial]
    fn apply_writes_mcp_json_at_root_with_stripped_servers_and_envelope() {
        // Matrix #1 (happy) + path guard: two servers selected → ROOT .mcp.json,
        // stripped, ONE mcp_merge row, owned_keys envelope v=1.
        let home = TempHome::new();
        crate::settings::reload_settings().ok();
        let db = Arc::new(Database::memory().expect("db"));
        let state = AppState::new(db.clone());
        seed_mcp(
            &db,
            "srv-a",
            serde_json::json!({"type":"stdio","command":"a","enabled":true,"name":"A"}),
            vec![],
        );
        seed_mcp(
            &db,
            "srv-b",
            serde_json::json!({"type":"http","url":"https://b","source":"reg"}),
            vec![],
        );
        let (proj, canon) = project_at(
            home.home(),
            "mcp-a",
            ProfileContent {
                skills: vec![],
                commands: vec![],
                agents: vec![],
                mcp: vec!["srv-a".into(), "srv-b".into()],
            },
        );
        db.save_project(&proj).expect("save");

        // Pre-seed an existing on-disk .mcp.json with an EMPTY mcpServers object so the
        // engine RECURSES into mcpServers (both user + frag are objects) and records a
        // per-server leaf for each inserted key. Without this, an absent file collapses to
        // user_opt=Some({}); the top-level `mcpServers` key is absent in user, so the engine
        // hits the `None` arm ONCE at path ["mcpServers"] (prior.present=false) and inserts
        // the WHOLE subtree withOUT recursing — yielding NO ["mcpServers","srv-a"] leaf. The
        // absent-file shape is pinned separately by apply_mcp_absent_file_created_with_only_our_subtree
        // (T4 #4); here we want the per-server-leaf "covers both servers" shape.
        let target = canon.join(".mcp.json");
        std::fs::write(&target, br#"{"mcpServers":{}}"#).unwrap();

        let res = ProjectApplyService::apply(&state, &proj.id).expect("apply");
        assert!(res.warnings.is_empty(), "no warnings: {:?}", res.warnings);

        // .mcp.json at ROOT, NOT under .claude/.
        assert!(target.is_file(), "root .mcp.json written");
        assert!(
            !canon.join(".claude").join(".mcp.json").exists(),
            "must NOT be written under .claude/"
        );
        let disk: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
        assert_eq!(
            disk["mcpServers"]["srv-a"],
            serde_json::json!({"type":"stdio","command":"a"}),
            "srv-a stripped (enabled/name gone)"
        );
        assert_eq!(
            disk["mcpServers"]["srv-b"],
            serde_json::json!({"type":"http","url":"https://b"}),
            "srv-b stripped (source gone)"
        );

        // exactly one mcp_merge row: content_hash None, owned_keys parses to v=1.
        let chan = format!("project:{}", canon.to_string_lossy());
        let rows = db.get_manifest_for_channel(&chan).unwrap();
        let m: Vec<_> = rows.iter().filter(|r| r.kind == "mcp_merge").collect();
        assert_eq!(m.len(), 1, "exactly one mcp_merge row");
        assert_eq!(m[0].content_hash, None, "merge rows carry NO content_hash");
        assert_eq!(m[0].target_path, target.to_string_lossy());
        let env: crate::services::settings_merge::OwnedKeysEnvelope =
            serde_json::from_str(m[0].owned_keys.as_deref().expect("owned_keys"))
                .expect("parse env");
        assert_eq!(env.v, crate::services::settings_merge::OWNED_KEYS_VERSION);
        // Pre-seeded empty mcpServers → engine recurses → ONE per-server leaf each
        // (both absent in the empty user map → prior.present=false). Covers BOTH servers.
        assert!(
            env.keys
                .iter()
                .any(|k| k.path == vec!["mcpServers".to_string(), "srv-a".to_string()]),
            "per-server leaf for srv-a recorded"
        );
        assert!(
            env.keys
                .iter()
                .any(|k| k.path == vec!["mcpServers".to_string(), "srv-b".to_string()]),
            "per-server leaf for srv-b recorded"
        );
    }

    #[test]
    #[serial]
    fn apply_empty_mcp_selection_writes_no_file_no_row() {
        // Matrix #6 empty-collapse: content.mcp=[] → no file, no row.
        let home = TempHome::new();
        crate::settings::reload_settings().ok();
        let db = Arc::new(Database::memory().expect("db"));
        let state = AppState::new(db.clone());
        seed_mcp(
            &db,
            "srv-a",
            serde_json::json!({"type":"stdio","command":"a"}),
            vec![],
        );
        let (proj, canon) = project_at(home.home(), "mcp-empty", ProfileContent::default());
        db.save_project(&proj).expect("save");
        ProjectApplyService::apply(&state, &proj.id).expect("apply");
        assert!(
            !canon.join(".mcp.json").exists(),
            "no file for empty selection"
        );
        let chan = format!("project:{}", canon.to_string_lossy());
        assert!(
            !db.get_manifest_for_channel(&chan)
                .unwrap()
                .iter()
                .any(|r| r.kind == "mcp_merge"),
            "no mcp_merge row for empty selection"
        );
    }

    #[test]
    #[serial]
    fn apply_non_object_server_spec_is_skipped_with_warning() {
        // Matrix #8: server C spec is a String → A written, C skipped + warn.
        let home = TempHome::new();
        crate::settings::reload_settings().ok();
        let db = Arc::new(Database::memory().expect("db"));
        let state = AppState::new(db.clone());
        seed_mcp(
            &db,
            "srv-a",
            serde_json::json!({"type":"stdio","command":"a"}),
            vec![],
        );
        seed_mcp(
            &db,
            "srv-c",
            serde_json::json!("i-am-not-an-object"),
            vec![],
        );
        let (proj, canon) = project_at(
            home.home(),
            "mcp-nonobj",
            ProfileContent {
                skills: vec![],
                commands: vec![],
                agents: vec![],
                mcp: vec!["srv-a".into(), "srv-c".into()],
            },
        );
        db.save_project(&proj).expect("save");
        let res = ProjectApplyService::apply(&state, &proj.id).expect("apply");
        assert!(
            res.warnings
                .iter()
                .any(|w| w.contains("srv-c") && w.contains("not a JSON object")),
            "warns about non-object srv-c: {:?}",
            res.warnings
        );
        let disk: serde_json::Value =
            serde_json::from_slice(&std::fs::read(canon.join(".mcp.json")).unwrap()).unwrap();
        assert!(disk["mcpServers"].get("srv-a").is_some(), "srv-a written");
        assert!(disk["mcpServers"].get("srv-c").is_none(), "srv-c skipped");
    }

    #[test]
    #[serial]
    fn apply_mcp_malformed_disk_skips_byte_identical_no_row() {
        // Matrix #7: pre-existing .mcp.json is invalid JSON → warn, byte-identical, NO row.
        let home = TempHome::new();
        crate::settings::reload_settings().ok();
        let db = Arc::new(Database::memory().expect("db"));
        let state = AppState::new(db.clone());
        seed_mcp(
            &db,
            "srv-a",
            serde_json::json!({"type":"stdio","command":"a"}),
            vec![],
        );
        let (proj, canon) = project_at(
            home.home(),
            "mcp-mal",
            ProfileContent {
                skills: vec![],
                commands: vec![],
                agents: vec![],
                mcp: vec!["srv-a".into()],
            },
        );
        db.save_project(&proj).expect("save");
        let target = canon.join(".mcp.json");
        std::fs::write(&target, b"not json {{").unwrap();
        let res = ProjectApplyService::apply(&state, &proj.id).expect("apply");
        assert!(
            res.warnings.iter().any(|w| w.contains("not a JSON value")),
            "warns on malformed disk: {:?}",
            res.warnings
        );
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"not json {{",
            "byte-identical"
        );
        let chan = format!("project:{}", canon.to_string_lossy());
        assert!(
            !db.get_manifest_for_channel(&chan)
                .unwrap()
                .iter()
                .any(|r| r.kind == "mcp_merge"),
            "no row when disk skipped"
        );
    }

    #[test]
    #[serial]
    fn apply_mcp_absent_file_created_with_only_our_subtree() {
        // Matrix #4: merge into absent file → created with mcpServers.A, prior absent.
        let home = TempHome::new();
        crate::settings::reload_settings().ok();
        let db = Arc::new(Database::memory().expect("db"));
        let state = AppState::new(db.clone());
        seed_mcp(
            &db,
            "srv-a",
            serde_json::json!({"type":"stdio","command":"a"}),
            vec![],
        );
        let (proj, canon) = project_at(
            home.home(),
            "mcp-absent",
            ProfileContent {
                skills: vec![],
                commands: vec![],
                agents: vec![],
                mcp: vec!["srv-a".into()],
            },
        );
        db.save_project(&proj).expect("save");
        ProjectApplyService::apply(&state, &proj.id).expect("apply");
        let disk: serde_json::Value =
            serde_json::from_slice(&std::fs::read(canon.join(".mcp.json")).unwrap()).unwrap();
        assert_eq!(
            disk,
            serde_json::json!({"mcpServers":{"srv-a":{"type":"stdio","command":"a"}}})
        );
        let chan = format!("project:{}", canon.to_string_lossy());
        let m: Vec<_> = db
            .get_manifest_for_channel(&chan)
            .unwrap()
            .into_iter()
            .filter(|r| r.kind == "mcp_merge")
            .collect();
        let env: crate::services::settings_merge::OwnedKeysEnvelope =
            serde_json::from_str(m[0].owned_keys.as_deref().unwrap()).unwrap();
        let leaf = env
            .keys
            .iter()
            .find(|k| k.path == vec!["mcpServers".to_string()])
            .expect("mcpServers subtree leaf (absent-file → ONE whole-subtree leaf)");
        assert!(
            !leaf.prior.present,
            "prior.present=false for absent-file merge"
        );
    }

    #[test]
    #[serial]
    fn apply_mcp_is_idempotent_one_row_identical_bytes() {
        // Matrix #9: apply [A] twice → identical bytes, ONE row.
        let home = TempHome::new();
        crate::settings::reload_settings().ok();
        let db = Arc::new(Database::memory().expect("db"));
        let state = AppState::new(db.clone());
        seed_mcp(
            &db,
            "srv-a",
            serde_json::json!({"type":"stdio","command":"a"}),
            vec![],
        );
        let (proj, canon) = project_at(
            home.home(),
            "mcp-idem",
            ProfileContent {
                skills: vec![],
                commands: vec![],
                agents: vec![],
                mcp: vec!["srv-a".into()],
            },
        );
        db.save_project(&proj).expect("save");
        ProjectApplyService::apply(&state, &proj.id).expect("apply 1");
        let bytes1 = std::fs::read(canon.join(".mcp.json")).unwrap();
        ProjectApplyService::apply(&state, &proj.id).expect("apply 2");
        let bytes2 = std::fs::read(canon.join(".mcp.json")).unwrap();
        assert_eq!(bytes1, bytes2, "re-apply produces identical bytes");
        let chan = format!("project:{}", canon.to_string_lossy());
        let count = db
            .get_manifest_for_channel(&chan)
            .unwrap()
            .iter()
            .filter(|r| r.kind == "mcp_merge")
            .count();
        assert_eq!(count, 1, "re-apply stays idempotent (one mcp_merge row)");
    }

    #[test]
    #[serial]
    fn detach_removes_our_servers_preserves_user_servers_and_other_keys() {
        // Matrix #2 + #5 (whole-file-delete regression): user server + other top
        // key pre-exist; apply [A]; detach → user-x + otherTop survive, A gone,
        // FILE NOT whole-file-deleted (mcp_merge routes to reverse_merge).
        let home = TempHome::new();
        crate::settings::reload_settings().ok();
        let db = Arc::new(Database::memory().expect("db"));
        let state = AppState::new(db.clone());
        seed_mcp(
            &db,
            "srv-a",
            serde_json::json!({"type":"stdio","command":"a"}),
            vec![],
        );
        let (proj, canon) = project_at(
            home.home(),
            "mcp-detach",
            ProfileContent {
                skills: vec![],
                commands: vec![],
                agents: vec![],
                mcp: vec!["srv-a".into()],
            },
        );
        db.save_project(&proj).expect("save");

        // user already has a .mcp.json with their OWN server + an unrelated top key.
        let target = canon.join(".mcp.json");
        std::fs::write(
            &target,
            serde_json::to_vec_pretty(&serde_json::json!({
                "mcpServers": { "user-x": { "type": "stdio", "command": "u" } },
                "otherTop": 1
            }))
            .unwrap(),
        )
        .unwrap();

        ProjectApplyService::apply(&state, &proj.id).expect("apply");
        // after apply: both servers present, otherTop intact.
        let merged: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
        assert!(merged["mcpServers"].get("srv-a").is_some(), "ours added");
        assert!(
            merged["mcpServers"].get("user-x").is_some(),
            "user server kept on apply"
        );

        ProjectApplyService::detach(&state, &proj.id).expect("detach");
        assert!(
            target.exists(),
            "file survives detach (NOT whole-file-deleted)"
        );
        let after: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
        assert!(
            after["mcpServers"].get("srv-a").is_none(),
            "our server removed on detach"
        );
        assert_eq!(
            after["mcpServers"]["user-x"],
            serde_json::json!({"type":"stdio","command":"u"}),
            "user server survives detach"
        );
        assert_eq!(
            after["otherTop"],
            serde_json::json!(1),
            "other top key survives"
        );

        let chan = format!("project:{}", canon.to_string_lossy());
        assert_eq!(
            db.get_manifest_for_channel(&chan).unwrap().len(),
            0,
            "rows cleared"
        );
    }

    #[test]
    #[serial]
    fn mcp_merge_row_routes_to_reverse_merge_not_whole_file_delete() {
        // Matrix #5 (explicit regression): a synthetic content_hash=None mcp_merge
        // row whose owned_keys removes nothing → teardown must call reverse_merge,
        // never remove_whole_file_if_owned → file SURVIVES.
        let home = TempHome::new();
        crate::settings::reload_settings().ok();
        let db = Arc::new(Database::memory().expect("db"));
        let state = AppState::new(db.clone());
        let (proj, canon) = project_at(home.home(), "mcp-regress", ProfileContent::default());
        db.save_project(&proj).expect("save");

        let target = canon.join(".mcp.json");
        std::fs::write(&target, br#"{"mcpServers":{"user-x":{"command":"u"}}}"#).unwrap();
        let chan = format!("project:{}", canon.to_string_lossy());
        // an owned_keys envelope that owns NOTHING currently on disk (wrote a leaf
        // the user later changed) → reverse_merge leaves the file intact.
        let env = serde_json::to_string(&crate::services::settings_merge::OwnedKeysEnvelope {
            v: crate::services::settings_merge::OWNED_KEYS_VERSION,
            keys: vec![crate::services::settings_merge::OwnedKey {
                path: vec!["mcpServers".into(), "ghost".into()],
                prior: crate::services::settings_merge::PriorLeaf {
                    present: false,
                    value: None,
                },
                wrote: serde_json::json!({"command":"never-on-disk"}),
            }],
        })
        .unwrap();
        db.record_manifest_entry(&ManifestEntry {
            id: 0,
            channel: chan.clone(),
            profile_id: None,
            project_id: Some(proj.id.clone()),
            app_type: "claude".into(),
            target_path: target.to_string_lossy().to_string(),
            kind: "mcp_merge".into(),
            content_hash: None,
            owned_keys: Some(env),
            created_at: 0,
        })
        .unwrap();

        ProjectApplyService::detach(&state, &proj.id).expect("detach");
        assert!(
            target.exists(),
            "mcp_merge row must NOT whole-file-delete the file"
        );
        let after: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
        assert_eq!(
            after["mcpServers"]["user-x"],
            serde_json::json!({"command":"u"}),
            "user server fully intact (reverse_merge, not whole-file delete)"
        );
    }

    #[test]
    #[serial]
    fn detach_user_field_inside_our_server_survives() {
        // Matrix #10 (per-field): pre-seed mcpServers.A={command,userField}; apply
        // frag A={command,args}; detach → pin to the engine's exact per-leaf result.
        // merge_with_snapshot recurses A (both objects): writes [A,args] (absent)
        // and overwrites [A,command] (prior "u"→"a"); userField is NOT in our frag
        // so it is untouched. reverse_merge removes [A,args], restores [A,command]
        // to "u"; userField survives → A = {"command":"u","userField":true}.
        let home = TempHome::new();
        crate::settings::reload_settings().ok();
        let db = Arc::new(Database::memory().expect("db"));
        let state = AppState::new(db.clone());
        seed_mcp(
            &db,
            "srv-a",
            serde_json::json!({"type":"stdio","command":"a","args":["x"]}),
            vec![],
        );
        let (proj, canon) = project_at(
            home.home(),
            "mcp-perfield",
            ProfileContent {
                skills: vec![],
                commands: vec![],
                agents: vec![],
                mcp: vec!["srv-a".into()],
            },
        );
        db.save_project(&proj).expect("save");

        let target = canon.join(".mcp.json");
        std::fs::write(
            &target,
            br#"{"mcpServers":{"srv-a":{"command":"u","userField":true}}}"#,
        )
        .unwrap();

        ProjectApplyService::apply(&state, &proj.id).expect("apply");
        let merged: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
        // our frag overwrote command + added args + type; userField untouched.
        assert_eq!(
            merged["mcpServers"]["srv-a"]["command"],
            serde_json::json!("a")
        );
        assert_eq!(
            merged["mcpServers"]["srv-a"]["args"],
            serde_json::json!(["x"])
        );
        assert_eq!(
            merged["mcpServers"]["srv-a"]["userField"],
            serde_json::json!(true)
        );

        ProjectApplyService::detach(&state, &proj.id).expect("detach");
        let after: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
        assert_eq!(
            after["mcpServers"]["srv-a"],
            serde_json::json!({"command":"u","userField":true}),
            "per-leaf reverse: command restored, args/type removed, userField survives"
        );
    }
}
