//! Profile 命令层
//!
//! 为前端暴露 Profile 的 CRUD 及激活/取消激活操作。
//! 镜像 `commands/agent.rs` 和 `commands/provider.rs` 的状态获取模式：
//! - 简单查询通过 `AppState` 直达 DB
//! - activate/deactivate 委托给无状态 `ProfileService`（镜像 switch_provider 模式）

use crate::app_config::{AppType, ManifestEntry, Profile, ProfileDotfile, ProfileSpec};
use crate::services::profile::{ActivateResult, ProfileService};
use crate::store::AppState;
use std::str::FromStr;
use tauri::State;

// ========== Profile 管理 ==========

/// 获取所有 Profile（直接查 DB）
#[tauri::command]
pub fn get_profiles(state: State<'_, AppState>) -> Result<Vec<Profile>, String> {
    state.db.get_all_profiles().map_err(|e| e.to_string())
}

/// 获取指定 app 的所有 Profile
#[tauri::command]
pub fn get_profiles_for_app(
    app: String,
    state: State<'_, AppState>,
) -> Result<Vec<Profile>, String> {
    state
        .db
        .get_profiles_for_app(&app)
        .map_err(|e| e.to_string())
}

/// 创建 Profile
#[tauri::command]
pub fn create_profile(
    app: String,
    name: String,
    description: Option<String>,
    current_provider_id: Option<String>,
    spec: ProfileSpec,
    state: State<'_, AppState>,
) -> Result<Profile, String> {
    // 校验 app_type
    AppType::from_str(&app).map_err(|e| e.to_string())?;

    // 生成唯一 ID（镜像 agent.rs 的 local: 前缀 + 名称）
    let id = format!("local:{app}:{name}");

    // 计算 sort_index = max(existing for app) + 1
    let existing = state
        .db
        .get_profiles_for_app(&app)
        .map_err(|e| e.to_string())?;
    let sort_index = existing.iter().map(|p| p.sort_index).max().unwrap_or(-1) + 1;

    let created_at = chrono::Utc::now().timestamp();

    let profile = Profile {
        id,
        app_type: app,
        name,
        description,
        is_active: false,
        current_provider_id,
        spec,
        sort_index,
        created_at,
    };

    state.db.save_profile(&profile).map_err(|e| {
        // UNIQUE (app_type, name) 违约 → 友好错误提示
        let msg = e.to_string();
        if msg.contains("UNIQUE") || msg.contains("unique") {
            format!(
                "profile '{}' already exists for app '{}'",
                profile.name, profile.app_type
            )
        } else {
            msg
        }
    })?;

    Ok(profile)
}

/// 更新 Profile（仅更新元数据字段；不应用到磁盘）
#[tauri::command]
pub fn update_profile(
    id: String,
    name: Option<String>,
    description: Option<String>,
    current_provider_id: Option<String>,
    spec: ProfileSpec,
    state: State<'_, AppState>,
) -> Result<Profile, String> {
    let mut profile = state
        .db
        .get_profile(&id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("profile not found: {id}"))?;

    if let Some(n) = name {
        profile.name = n;
    }
    // description 字段：始终采用传入值（None 表示清除）
    profile.description = description;
    // current_provider_id：始终采用传入值（None 表示清除）
    profile.current_provider_id = current_provider_id;
    profile.spec = spec;

    state.db.save_profile(&profile).map_err(|e| {
        let msg = e.to_string();
        if msg.contains("UNIQUE") || msg.contains("unique") {
            format!(
                "profile '{}' already exists for app '{}'",
                profile.name, profile.app_type
            )
        } else {
            msg
        }
    })?;

    Ok(profile)
}

/// 删除 Profile；返回是否找到该行
///
/// 3b-3: 若为 Claude profile，同步删除其派生的隐藏 `__profile__:<id>` prompt 行。
/// 若该 profile 当前为 active 且其隐藏行已启用，则先通过 upsert_prompt(enabled=false)
/// 将其禁用（触发 any_enabled 检测，若无其他启用项则清空 CLAUDE.md），
/// 再直接通过 DAO 删除该隐藏行。此处**不调用** ProfileService::deactivate，
/// 以避免 manifest teardown + settings.json 重建的副作用，以及潜在的重入风险。
#[tauri::command]
pub fn delete_profile(id: String, state: State<'_, AppState>) -> Result<bool, String> {
    // None-safe: 若 profile 不存在，直接尝试删除并返回
    let Some(profile) = state.db.get_profile(&id).map_err(|e| e.to_string())? else {
        return state.db.delete_profile(&id).map_err(|e| e.to_string());
    };

    if profile.app_type == AppType::Claude.as_str() {
        let hidden_id = format!("__profile__:{}", id);

        // 若该 profile 当前为 active 且其隐藏行已启用，先禁用隐藏行（清空 CLAUDE.md）
        let is_active = state
            .db
            .get_active_profile("claude")
            .map_err(|e| e.to_string())?
            .map(|p| p.id)
            == Some(id.clone());
        if is_active {
            if let Some(mut row) = state
                .db
                .get_prompt_with_hidden("claude", &hidden_id)
                .map_err(|e| e.to_string())?
            {
                if row.enabled {
                    row.enabled = false;
                    crate::services::prompt::PromptService::upsert_prompt(
                        &state,
                        AppType::Claude,
                        &hidden_id,
                        row,
                    )
                    .map_err(|e| e.to_string())?;
                }
            }
        }

        // 直接通过 DAO 删除隐藏 prompt 行（无论是否存在，删除不存在的行无副作用）
        state
            .db
            .delete_prompt("claude", &hidden_id)
            .map_err(|e| e.to_string())?;
    }

    // 删除 profile（CASCADE: profile_dotfiles + manifest）
    state.db.delete_profile(&id).map_err(|e| e.to_string())
}

/// 激活指定 Profile（委托 ProfileService::activate）
#[tauri::command]
pub fn activate_profile(
    app: String,
    id: String,
    state: State<'_, AppState>,
) -> Result<ActivateResult, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    ProfileService::activate(&state, app_type, &id).map_err(|e| e.to_string())
}

/// 取消激活指定 app 的当前 Profile（委托 ProfileService::deactivate）
///
/// 3b：deactivate 现执行确定性 teardown（拆除 whole-file dotfiles + 重建 settings.json），
/// 返回非致命警告（与 activate 一致）。
#[tauri::command]
pub fn deactivate_profile(
    app: String,
    state: State<'_, AppState>,
) -> Result<ActivateResult, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    ProfileService::deactivate(&state, app_type).map_err(|e| e.to_string())
}

/// 获取指定 app 的当前激活 Profile
#[tauri::command]
pub fn get_active_profile(
    app: String,
    state: State<'_, AppState>,
) -> Result<Option<Profile>, String> {
    state.db.get_active_profile(&app).map_err(|e| e.to_string())
}

// ========== Profile Dotfile 管理 ==========

/// 保存/更新 Profile dotfile（先校验路径防止路径穿越）
#[tauri::command]
pub fn set_profile_dotfile(
    id: String,
    rel_path: String,
    content: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    crate::services::profile_render::validate_rel_path(&rel_path).map_err(|e| e.to_string())?;
    state
        .db
        .set_profile_dotfile(&id, &rel_path, &content)
        .map_err(|e| e.to_string())
}

/// 获取某个 Profile 的所有 dotfiles
#[tauri::command]
pub fn get_profile_dotfiles(
    id: String,
    state: State<'_, AppState>,
) -> Result<Vec<ProfileDotfile>, String> {
    state
        .db
        .get_profile_dotfiles(&id)
        .map_err(|e| e.to_string())
}

/// 删除某个 Profile 的单个 dotfile；返回是否找到该行
#[tauri::command]
pub fn delete_profile_dotfile(
    id: String,
    rel_path: String,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    state
        .db
        .delete_profile_dotfile(&id, &rel_path)
        .map_err(|e| e.to_string())
}

/// 获取某个 Profile + app 的全部 manifest 记录
#[tauri::command]
pub fn get_profile_manifest(
    id: String,
    app: String,
    state: State<'_, AppState>,
) -> Result<Vec<ManifestEntry>, String> {
    state
        .db
        .get_manifest_for_profile(&id, &app)
        .map_err(|e| e.to_string())
}

// ========== T5 tests: delete_profile cleans up hidden CLAUDE.md prompt row ==========

#[cfg(test)]
mod tests {
    use crate::app_config::{AppType, Profile, ProfileContent, ProfileSpec};
    use crate::database::Database;
    use crate::services::profile::ProfileService;
    use crate::store::AppState;
    use serial_test::serial;
    use std::env;
    use std::sync::Arc;
    use tempfile::TempDir;

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

        fn claude_md(&self) -> std::path::PathBuf {
            self.dir.path().join(".claude").join("CLAUDE.md")
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

    fn make_claude_profile(id: &str) -> Profile {
        Profile {
            id: id.into(),
            app_type: "claude".into(),
            name: id.into(),
            description: None,
            is_active: false,
            current_provider_id: None,
            spec: ProfileSpec {
                content: ProfileContent {
                    skills: vec![],
                    commands: vec![],
                    agents: vec![],
                    mcp: vec![],
                },
                vars: serde_json::Map::new(),
            },
            sort_index: 0,
            created_at: 1,
        }
    }

    /// Exercises the same logic as the `delete_profile` command, bypassing the Tauri
    /// `State<AppState>` wrapper (which cannot be constructed in unit tests without a
    /// live Tauri runtime).  Tests T5-delete Step 2 assertion:
    ///
    ///   delete_active_claude_profile_blanks_and_removes_hidden_row:
    ///     create + activate a claude profile A with CLAUDE.md="A" (file=="A", hidden
    ///     row enabled); invoke the delete logic; assert:
    ///       ~/.claude/CLAUDE.md == ""
    ///       get_prompt_with_hidden("claude","__profile__:A") == None
    ///       get_profile(A) == None
    fn run_delete_profile_logic(state: &AppState, id: &str) -> Result<bool, String> {
        let Some(profile) = state.db.get_profile(id).map_err(|e| e.to_string())? else {
            return state.db.delete_profile(id).map_err(|e| e.to_string());
        };

        if profile.app_type == AppType::Claude.as_str() {
            let hidden_id = format!("__profile__:{}", id);

            let is_active = state
                .db
                .get_active_profile("claude")
                .map_err(|e| e.to_string())?
                .map(|p| p.id)
                == Some(id.to_string());

            if is_active {
                if let Some(mut row) = state
                    .db
                    .get_prompt_with_hidden("claude", &hidden_id)
                    .map_err(|e| e.to_string())?
                {
                    if row.enabled {
                        row.enabled = false;
                        crate::services::prompt::PromptService::upsert_prompt(
                            state,
                            AppType::Claude,
                            &hidden_id,
                            row,
                        )
                        .map_err(|e| e.to_string())?;
                    }
                }
            }

            state
                .db
                .delete_prompt("claude", &hidden_id)
                .map_err(|e| e.to_string())?;
        }

        state.db.delete_profile(id).map_err(|e| e.to_string())
    }

    #[test]
    #[serial]
    fn delete_active_claude_profile_blanks_and_removes_hidden_row() {
        let home = TempHome::new();
        crate::settings::reload_settings().expect("reload settings");
        let db = Arc::new(Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());

        // Create + activate profile A with CLAUDE.md="A"
        let p = make_claude_profile("A");
        db.save_profile(&p).expect("save profile A");
        db.set_profile_dotfile("A", "CLAUDE.md", "A")
            .expect("set CLAUDE.md dotfile");

        ProfileService::activate(&state, AppType::Claude, "A").expect("activate A");

        // Pre-condition: profile is active, hidden row is enabled, file == "A"
        let active = db
            .get_active_profile("claude")
            .expect("get_active_profile")
            .expect("should have active profile");
        assert_eq!(active.id, "A");

        let hidden = db
            .get_prompt_with_hidden("claude", "__profile__:A")
            .expect("get hidden row")
            .expect("hidden row should exist after activate");
        assert!(
            hidden.enabled,
            "hidden row should be enabled after activate"
        );

        let live = std::fs::read_to_string(home.claude_md()).unwrap_or_default();
        assert_eq!(live, "A", "CLAUDE.md should contain 'A' after activate");

        // Delete profile A via the command logic
        let found = run_delete_profile_logic(&state, "A").expect("delete should not error");
        assert!(
            found,
            "delete_profile should return true for an existing profile"
        );

        // Post-conditions:
        // 1. CLAUDE.md is blank
        let live_after = std::fs::read_to_string(home.claude_md()).unwrap_or_default();
        assert_eq!(
            live_after, "",
            "CLAUDE.md should be blank after deleting the active profile"
        );

        // 2. hidden prompt row is gone
        let hidden_after = db
            .get_prompt_with_hidden("claude", "__profile__:A")
            .expect("get_prompt_with_hidden");
        assert!(
            hidden_after.is_none(),
            "hidden prompt row must not exist after profile deletion"
        );

        // 3. profile itself is gone
        let profile_after = db.get_profile("A").expect("get profile after delete");
        assert!(
            profile_after.is_none(),
            "profile A must not exist after deletion"
        );
    }

    /// delete_nonexistent_profile_is_ok:
    /// 删除不存在的 profile 不应 panic；None-safe 路径应返回 Ok(false)。
    #[test]
    #[serial]
    fn delete_nonexistent_profile_is_ok() {
        let _home = TempHome::new();
        crate::settings::reload_settings().expect("reload settings");
        let db = Arc::new(Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());

        let result = run_delete_profile_logic(&state, "does-not-exist")
            .expect("delete of nonexistent profile should return Ok, not Err");
        assert!(
            !result,
            "deleting a nonexistent profile should return false (0 rows affected)"
        );
    }
}
