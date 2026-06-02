//! Profile 命令层
//!
//! 为前端暴露 Profile 的 CRUD 及激活/取消激活操作。
//! 镜像 `commands/agent.rs` 和 `commands/provider.rs` 的状态获取模式：
//! - 简单查询通过 `AppState` 直达 DB
//! - activate/deactivate 委托给无状态 `ProfileService`（镜像 switch_provider 模式）

use crate::app_config::{AppType, Profile, ProfileSpec};
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
#[tauri::command]
pub fn delete_profile(id: String, state: State<'_, AppState>) -> Result<bool, String> {
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
#[tauri::command]
pub fn deactivate_profile(app: String, state: State<'_, AppState>) -> Result<(), String> {
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
