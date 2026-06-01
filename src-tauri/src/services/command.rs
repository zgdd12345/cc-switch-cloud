//! Commands 服务层
//!
//! 在磁盘上物化 Claude 斜杠命令（slash-commands）。
//!
//! 与 Skills 不同，Commands 是**直接写入的单文件**（无 SSOT / 无 symlink）：
//! - 启用的命令写入 `~/.claude/commands/<name>.md`
//! - 数据库存储安装记录和启用状态
//!
//! ## 安全性（SAFETY-CRITICAL）
//!
//! 本服务会在 `~/.claude/commands/` 中写入/删除文件，必须严格防止：
//! 1. **路径穿越**：所有文件操作的 name 必须先经过 `validate_name`
//!    （正则 `^[A-Za-z0-9._-]+$`，且拒绝 `.` / `..` / 任何路径分隔符）。
//! 2. **误删用户文件**：`reconcile()` 只对数据库中存在的命令名进行操作，
//!    **绝不枚举目录再删除未知文件**。用户自己放进去的 `.md` 因此天然不受影响。
//!
//! 注：服务方法目前由后续 Tauri 命令层消费，暂时整体 `allow(dead_code)`。
#![allow(dead_code)]

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::app_config::InstalledCommand;
use crate::config;
use crate::database::Database;
use crate::error::AppError;

/// 在 commands 目录中发现、但未被 AgentHub 管理的命令（只读，永不删除）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnmanagedCommand {
    /// 命令名（文件名去掉 `.md` 后缀）
    pub name: String,
    /// 完整路径
    pub path: String,
}

/// Command 服务：负责在磁盘上物化 Claude 斜杠命令
pub struct CommandService {
    db: Arc<Database>,
}

impl CommandService {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    // ========== 路径与校验（安全门） ==========

    /// 解析并确保 commands 目录存在
    fn commands_dir() -> Result<PathBuf, AppError> {
        let dir = config::get_commands_dir();
        fs::create_dir_all(&dir).map_err(|e| AppError::io(&dir, e))?;
        Ok(dir)
    }

    /// 校验命令名：必须非空、匹配 `^[A-Za-z0-9._-]+$`，且不是 `.` / `..`，
    /// 不含任何路径分隔符。**这是安全门**——所有文件操作都要先过这一关。
    fn validate_name(name: &str) -> Result<(), AppError> {
        if name.is_empty() {
            return Err(AppError::InvalidInput("命令名不能为空".to_string()));
        }
        if name == "." || name == ".." {
            return Err(AppError::InvalidInput(format!("非法命令名: {name}")));
        }
        // 显式拒绝路径分隔符（即便正则已覆盖，也作为纵深防御）
        if name.contains('/') || name.contains('\\') {
            return Err(AppError::InvalidInput(format!(
                "命令名不能包含路径分隔符: {name}"
            )));
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        {
            return Err(AppError::InvalidInput(format!(
                "命令名只能包含字母、数字、'.'、'_'、'-': {name}"
            )));
        }
        Ok(())
    }

    /// 根据命令名解析其在磁盘上的文件路径（先校验名称）。
    fn file_path(name: &str) -> Result<PathBuf, AppError> {
        Self::validate_name(name)?;
        let dir = Self::commands_dir()?;
        let path = dir.join(format!("{name}.md"));

        // 纵深防御：确保解析出的文件父目录就是 commands 目录本身，
        // 防止任何意外的路径逃逸。
        match path.parent() {
            Some(parent) if parent == dir => Ok(path),
            _ => Err(AppError::InvalidInput(format!(
                "命令路径逃逸出 commands 目录: {name}"
            ))),
        }
    }

    // ========== 文件物化 ==========

    /// 原子写入命令文件（临时文件 + fsync + rename）。
    fn write_command_file(c: &InstalledCommand) -> Result<(), AppError> {
        let path = Self::file_path(&c.name)?;
        config::atomic_write(&path, c.content.as_bytes())
    }

    /// 删除命令文件（若存在）。
    fn remove_command_file(name: &str) -> Result<(), AppError> {
        let path = Self::file_path(name)?;
        if path.exists() {
            fs::remove_file(&path).map_err(|e| AppError::io(&path, e))?;
        }
        Ok(())
    }

    /// 根据启用状态物化：启用则写文件，禁用则删文件。
    fn apply_enabled(c: &InstalledCommand) -> Result<(), AppError> {
        if c.enabled_claude {
            Self::write_command_file(c)
        } else {
            Self::remove_command_file(&c.name)
        }
    }

    // ========== 公共 API ==========

    /// 创建一个本地命令（默认不启用，因此暂不写文件）。
    pub fn create(
        &self,
        name: &str,
        content: &str,
        description: Option<String>,
        tags: Vec<String>,
    ) -> Result<InstalledCommand, AppError> {
        Self::validate_name(name)?;

        let id = format!("local:{name}");
        if self.db.get_installed_command(&id)?.is_some() {
            return Err(AppError::InvalidInput(format!("命令已存在: {name}")));
        }

        let command = InstalledCommand {
            id,
            name: name.to_string(),
            content: content.to_string(),
            description,
            tags,
            enabled_claude: false,
            installed_at: chrono::Utc::now().timestamp(),
        };

        self.db.save_command(&command)?;
        // enabled 默认 false，因此此处不写文件。
        Ok(command)
    }

    /// 更新命令内容/描述/标签；若已启用则重写文件。
    pub fn update(
        &self,
        id: &str,
        content: &str,
        description: Option<String>,
        tags: Vec<String>,
    ) -> Result<InstalledCommand, AppError> {
        let mut command = self
            .db
            .get_installed_command(id)?
            .ok_or_else(|| AppError::InvalidInput(format!("命令不存在: {id}")))?;

        command.content = content.to_string();
        command.description = description;
        command.tags = tags;

        self.db.save_command(&command)?;

        if command.enabled_claude {
            Self::write_command_file(&command)?;
        }
        Ok(command)
    }

    /// 切换命令的启用状态，并相应地物化/移除文件。
    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<bool, AppError> {
        let found = self.db.set_command_enabled(id, enabled)?;
        if !found {
            return Ok(false);
        }
        if let Some(command) = self.db.get_installed_command(id)? {
            Self::apply_enabled(&command)?;
        }
        Ok(true)
    }

    /// 删除命令：先删文件，再删数据库记录。
    pub fn delete(&self, id: &str) -> Result<bool, AppError> {
        if let Some(command) = self.db.get_installed_command(id)? {
            Self::remove_command_file(&command.name)?;
        }
        self.db.delete_command(id)
    }

    /// 安全地把数据库状态同步到磁盘。
    ///
    /// **SAFE 契约**：只遍历数据库中的命令——启用则写文件、禁用则删其文件。
    /// **绝不**枚举目录后删除未知文件，因此用户自己放进 commands 目录的 `.md`
    /// 文件天然不受影响。
    pub fn reconcile(&self) -> Result<(), AppError> {
        let commands = self.db.get_all_installed_commands()?;
        for command in &commands {
            // 跳过名称非法的脏数据，避免在安全门外触碰磁盘。
            if Self::validate_name(&command.name).is_err() {
                log::warn!("reconcile 跳过非法命令名: {}", command.name);
                continue;
            }
            Self::apply_enabled(command)?;
        }
        Ok(())
    }

    /// 扫描 commands 目录中未被管理的 `*.md` 文件（只读，永不删除）。
    pub fn scan_unmanaged(&self) -> Result<Vec<UnmanagedCommand>, AppError> {
        let managed: std::collections::HashSet<String> = self
            .db
            .get_all_installed_commands()?
            .into_iter()
            .map(|c| c.name)
            .collect();

        let dir = Self::commands_dir()?;
        let mut result = Vec::new();

        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => return Ok(result),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let stem = match path.file_stem().and_then(|s| s.to_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };
            if managed.contains(&stem) {
                continue;
            }
            result.push(UnmanagedCommand {
                name: stem,
                path: path.display().to_string(),
            });
        }
        Ok(result)
    }

    /// 从磁盘文件导入命令（默认启用并写回文件）。
    pub fn import_from_disk(&self, paths: Vec<String>) -> Result<Vec<InstalledCommand>, AppError> {
        let mut imported = Vec::new();
        for path_str in paths {
            let path = PathBuf::from(&path_str);
            let name = match path.file_stem().and_then(|s| s.to_str()) {
                Some(s) => s.to_string(),
                None => {
                    log::warn!("import 跳过无法解析文件名的路径: {path_str}");
                    continue;
                }
            };
            Self::validate_name(&name)?;

            let content = fs::read_to_string(&path).map_err(|e| AppError::io(&path, e))?;

            let command = self.create(&name, &content, None, Vec::new())?;
            // 导入即启用：写回文件。
            self.set_enabled(&command.id, true)?;

            let enabled = self
                .db
                .get_installed_command(&command.id)?
                .unwrap_or(command);
            imported.push(enabled);
        }
        Ok(imported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::env;
    use tempfile::TempDir;

    /// 测试用临时 HOME 守卫，模仿 app_config.rs 的 TempHome。
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

        fn commands_dir(&self) -> PathBuf {
            self.dir.path().join(".claude").join("commands")
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            match &self.original_home {
                Some(value) => env::set_var("HOME", value),
                None => env::remove_var("HOME"),
            }
            match &self.original_userprofile {
                Some(value) => env::set_var("USERPROFILE", value),
                None => env::remove_var("USERPROFILE"),
            }
            match &self.original_test_home {
                Some(value) => env::set_var("CC_SWITCH_TEST_HOME", value),
                None => env::remove_var("CC_SWITCH_TEST_HOME"),
            }
        }
    }

    fn service() -> CommandService {
        let db = Arc::new(Database::memory().expect("memory db"));
        CommandService::new(db)
    }

    #[test]
    #[serial]
    fn create_then_enable_writes_file_with_content() {
        let home = TempHome::new();
        let svc = service();

        let cmd = svc
            .create("fix", "# Fix\nbody", Some("d".into()), vec!["core".into()])
            .expect("create");
        // 默认不启用，无文件
        let path = home.commands_dir().join("fix.md");
        assert!(!path.exists(), "disabled command must not write a file");

        svc.set_enabled(&cmd.id, true).expect("enable");
        assert!(path.exists(), "enabled command file must exist");
        assert_eq!(fs::read_to_string(&path).unwrap(), "# Fix\nbody");
    }

    #[test]
    #[serial]
    fn disable_removes_file_but_keeps_db_row() {
        let home = TempHome::new();
        let svc = service();

        let cmd = svc
            .create("hello", "content", None, vec![])
            .expect("create");
        svc.set_enabled(&cmd.id, true).expect("enable");
        let path = home.commands_dir().join("hello.md");
        assert!(path.exists());

        svc.set_enabled(&cmd.id, false).expect("disable");
        assert!(!path.exists(), "disabling must remove the file");

        let row = svc
            .db
            .get_installed_command(&cmd.id)
            .expect("query")
            .expect("row still present");
        assert!(!row.enabled_claude, "db row remains, enabled_claude=false");
    }

    #[test]
    #[serial]
    fn delete_removes_file_and_db_row() {
        let home = TempHome::new();
        let svc = service();

        let cmd = svc.create("gone", "x", None, vec![]).expect("create");
        svc.set_enabled(&cmd.id, true).expect("enable");
        let path = home.commands_dir().join("gone.md");
        assert!(path.exists());

        assert!(svc.delete(&cmd.id).expect("delete"));
        assert!(!path.exists(), "delete removes the file");
        assert!(
            svc.db
                .get_installed_command(&cmd.id)
                .expect("query")
                .is_none(),
            "delete removes the db row"
        );
    }

    #[test]
    #[serial]
    fn reconcile_does_not_delete_unrelated_user_file() {
        let home = TempHome::new();
        let svc = service();

        // 用户自己放进去的、无数据库记录的文件
        let dir = home.commands_dir();
        fs::create_dir_all(&dir).expect("mkdir");
        let user_file = dir.join("usercmd.md");
        fs::write(&user_file, "user content").expect("write user file");

        // 一个启用的命令 + 一个禁用的命令
        let enabled = svc.create("enabled", "E", None, vec![]).expect("create");
        svc.db
            .set_command_enabled(&enabled.id, true)
            .expect("flag enabled");
        let _disabled = svc.create("disabled", "D", None, vec![]).expect("create");

        svc.reconcile().expect("reconcile");

        // SAFETY: 未知用户文件必须保留
        assert!(
            user_file.exists(),
            "reconcile must NOT delete an unrelated user file"
        );
        assert_eq!(fs::read_to_string(&user_file).unwrap(), "user content");

        // 启用的命令写出文件，禁用的命令无文件
        assert!(dir.join("enabled.md").exists());
        assert!(!dir.join("disabled.md").exists());
    }

    #[test]
    #[serial]
    fn name_validation_rejects_traversal_and_separators() {
        let _home = TempHome::new();
        let svc = service();

        for bad in ["../evil", "a/b", "..", ".", "", "a\\b", "foo/../bar"] {
            let err = svc.create(bad, "x", None, vec![]);
            assert!(
                err.is_err(),
                "name {bad:?} should be rejected by validation"
            );
        }
        // 确认没有任何文件被写到 commands 目录之外或之内
        let dir = config::get_commands_dir();
        if dir.exists() {
            let count = fs::read_dir(&dir).map(|e| e.count()).unwrap_or(0);
            assert_eq!(count, 0, "no files should be written for invalid names");
        }
    }

    #[test]
    #[serial]
    fn import_from_disk_creates_and_enables() {
        let home = TempHome::new();
        let svc = service();

        let dir = home.commands_dir();
        fs::create_dir_all(&dir).expect("mkdir");
        let src = dir.join("imported.md");
        fs::write(&src, "imported body").expect("write src");

        let imported = svc
            .import_from_disk(vec![src.display().to_string()])
            .expect("import");
        assert_eq!(imported.len(), 1);
        assert!(imported[0].enabled_claude);
        assert_eq!(imported[0].name, "imported");

        // 文件仍在（被写回），数据库有记录
        assert!(src.exists());
        assert!(svc
            .db
            .get_installed_command("local:imported")
            .expect("query")
            .is_some());
    }

    #[test]
    #[serial]
    fn scan_unmanaged_lists_only_unmanaged_md_files() {
        let home = TempHome::new();
        let svc = service();

        let dir = home.commands_dir();
        fs::create_dir_all(&dir).expect("mkdir");
        fs::write(dir.join("usercmd.md"), "u").expect("write");
        fs::write(dir.join("notes.txt"), "x").expect("write");

        let managed = svc.create("managed", "m", None, vec![]).expect("create");
        svc.set_enabled(&managed.id, true).expect("enable");

        let unmanaged = svc.scan_unmanaged().expect("scan");
        let names: Vec<String> = unmanaged.into_iter().map(|u| u.name).collect();
        assert!(names.contains(&"usercmd".to_string()));
        assert!(!names.contains(&"managed".to_string()));
        assert!(!names.contains(&"notes".to_string()));
    }

    #[test]
    #[serial]
    fn update_rewrites_file_when_enabled() {
        let home = TempHome::new();
        let svc = service();

        let cmd = svc.create("upd", "v1", None, vec![]).expect("create");
        svc.set_enabled(&cmd.id, true).expect("enable");
        let path = home.commands_dir().join("upd.md");
        assert_eq!(fs::read_to_string(&path).unwrap(), "v1");

        svc.update(&cmd.id, "v2", Some("new".into()), vec!["t".into()])
            .expect("update");
        assert_eq!(fs::read_to_string(&path).unwrap(), "v2");
    }
}
