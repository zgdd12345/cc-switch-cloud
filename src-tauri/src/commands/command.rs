//! Commands 命令层
//!
//! 为前端暴露斜杠命令（slash-commands）的 CRUD 及扫描/导入操作。
//! 镜像 `skill.rs` 的状态获取模式：
//! - `CommandServiceState` 包装 `Arc<CommandService>`，在 lib.rs `setup` 时 `.manage()` 注册
//! - 简单列表查询通过 `AppState` 直达 DB（与 `get_installed_skills` 相同模式）
//! - 其余变更操作通过 `CommandServiceState` 调用服务方法

use crate::app_config::InstalledCommand;
use crate::services::command::{CommandService, UnmanagedCommand};
use crate::store::AppState;
use std::sync::Arc;
use tauri::State;

/// CommandService 状态包装（镜像 SkillServiceState）
pub struct CommandServiceState(pub Arc<CommandService>);

// ========== 命令管理 ==========

/// 获取所有已安装的命令（直接查 DB，与 get_installed_skills 相同模式）
#[tauri::command]
pub fn get_installed_commands(
    app_state: State<'_, AppState>,
) -> Result<Vec<InstalledCommand>, String> {
    app_state
        .db
        .get_all_installed_commands()
        .map_err(|e| e.to_string())
}

/// 创建本地命令
#[tauri::command]
pub fn create_command(
    name: String,
    content: String,
    description: Option<String>,
    tags: Vec<String>,
    service: State<'_, CommandServiceState>,
) -> Result<InstalledCommand, String> {
    service
        .0
        .create(&name, &content, description, tags)
        .map_err(|e| e.to_string())
}

/// 更新命令内容/描述/标签
#[tauri::command]
pub fn update_command(
    id: String,
    content: String,
    description: Option<String>,
    tags: Vec<String>,
    service: State<'_, CommandServiceState>,
) -> Result<InstalledCommand, String> {
    service
        .0
        .update(&id, &content, description, tags)
        .map_err(|e| e.to_string())
}

/// 删除命令
#[tauri::command]
pub fn delete_command(id: String, service: State<'_, CommandServiceState>) -> Result<bool, String> {
    service.0.delete(&id).map_err(|e| e.to_string())
}

/// 切换命令启用状态
#[tauri::command]
pub fn set_command_enabled(
    id: String,
    enabled: bool,
    service: State<'_, CommandServiceState>,
) -> Result<bool, String> {
    service
        .0
        .set_enabled(&id, enabled)
        .map_err(|e| e.to_string())
}

/// 扫描未被管理的命令文件
#[tauri::command]
pub fn scan_unmanaged_commands(
    service: State<'_, CommandServiceState>,
) -> Result<Vec<UnmanagedCommand>, String> {
    service.0.scan_unmanaged().map_err(|e| e.to_string())
}

/// 从磁盘路径导入命令（导入即启用）
#[tauri::command]
pub fn import_commands_from_disk(
    paths: Vec<String>,
    service: State<'_, CommandServiceState>,
) -> Result<Vec<InstalledCommand>, String> {
    service.0.import_from_disk(paths).map_err(|e| e.to_string())
}
