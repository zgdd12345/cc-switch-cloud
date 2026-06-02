//! Agents 命令层
//!
//! 为前端暴露子代理的 CRUD 及扫描/导入操作。
//! 镜像 `command.rs` 的状态获取模式：
//! - `AgentServiceState` 包装 `Arc<AgentService>`，在 lib.rs `setup` 时 `.manage()` 注册
//! - 简单列表查询通过 `AppState` 直达 DB（与 `get_installed_commands` 相同模式）
//! - 其余变更操作通过 `AgentServiceState` 调用服务方法

use crate::app_config::InstalledAgent;
use crate::services::agent::{AgentService, UnmanagedAgent};
use crate::store::AppState;
use std::sync::Arc;
use tauri::State;

/// AgentService 状态包装（镜像 CommandServiceState）
pub struct AgentServiceState(pub Arc<AgentService>);

// ========== 代理管理 ==========

/// 获取所有已安装的代理（直接查 DB，与 get_installed_commands 相同模式）
#[tauri::command]
pub fn get_installed_agents(app_state: State<'_, AppState>) -> Result<Vec<InstalledAgent>, String> {
    app_state
        .db
        .get_all_installed_agents()
        .map_err(|e| e.to_string())
}

/// 创建本地代理
#[tauri::command]
pub fn create_agent(
    name: String,
    content: String,
    description: Option<String>,
    tags: Vec<String>,
    service: State<'_, AgentServiceState>,
) -> Result<InstalledAgent, String> {
    service
        .0
        .create(&name, &content, description, tags)
        .map_err(|e| e.to_string())
}

/// 更新代理内容/描述/标签
#[tauri::command]
pub fn update_agent(
    id: String,
    content: String,
    description: Option<String>,
    tags: Vec<String>,
    service: State<'_, AgentServiceState>,
) -> Result<InstalledAgent, String> {
    service
        .0
        .update(&id, &content, description, tags)
        .map_err(|e| e.to_string())
}

/// 删除代理
#[tauri::command]
pub fn delete_agent(id: String, service: State<'_, AgentServiceState>) -> Result<bool, String> {
    service.0.delete(&id).map_err(|e| e.to_string())
}

/// 切换代理启用状态
#[tauri::command]
pub fn set_agent_enabled(
    id: String,
    enabled: bool,
    service: State<'_, AgentServiceState>,
) -> Result<bool, String> {
    service
        .0
        .set_enabled(&id, enabled)
        .map_err(|e| e.to_string())
}

/// 扫描未被管理的代理文件
#[tauri::command]
pub fn scan_unmanaged_agents(
    service: State<'_, AgentServiceState>,
) -> Result<Vec<UnmanagedAgent>, String> {
    service.0.scan_unmanaged().map_err(|e| e.to_string())
}

/// 从磁盘路径导入代理（导入即启用）
#[tauri::command]
pub fn import_agents_from_disk(
    paths: Vec<String>,
    service: State<'_, AgentServiceState>,
) -> Result<Vec<InstalledAgent>, String> {
    service.0.import_from_disk(paths).map_err(|e| e.to_string())
}
