use indexmap::IndexMap;

use crate::app_config::AppType;
use crate::config::write_text_file;
use crate::error::AppError;
use crate::prompt::Prompt;
use crate::prompt_files::prompt_file_path;
use crate::store::AppState;

/// 安全地获取当前 Unix 时间戳
fn get_unix_timestamp() -> Result<i64, AppError> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .map_err(|e| AppError::Message(format!("Failed to get system time: {e}")))
}

pub struct PromptService;

// 隐藏的 `__profile__:<profile_id>` 行（profile 派生的 CLAUDE.md 模板）只能通过
// `enable_prompt` 启用——它会执行 single-enabled sweep 把其它行（含隐藏行）置为
// disabled。绝不能通过 `upsert_prompt(enabled=true)` 启用隐藏行：`upsert_prompt`
// 不做 sweep，会破坏「最多一个启用项」的不变量。

impl PromptService {
    pub fn get_prompts(
        state: &AppState,
        app: AppType,
    ) -> Result<IndexMap<String, Prompt>, AppError> {
        state.db.get_prompts(app.as_str())
    }

    pub fn upsert_prompt(
        state: &AppState,
        app: AppType,
        _id: &str,
        prompt: Prompt,
    ) -> Result<(), AppError> {
        // 防御：保留命名空间 `__profile__:` 仅供 ProfileService 内部路径写入
        // （它们走 save_prompt 直连 DAO + enable_prompt 来启用，绝不经此处 enabled=true）。
        // 唯一会经过本函数的合法 __profile__ 调用都是 enabled=false 的禁用路径，故仅当
        // 试图「启用」一条保留行时拒绝——这正是会破坏单启用不变量 / 影子覆盖隐藏行的危险动作。
        if prompt.enabled
            && (_id.starts_with("__profile__:") || prompt.id.starts_with("__profile__:"))
        {
            return Err(AppError::InvalidInput(
                "不能通过 upsert 启用保留命名空间 __profile__: 的提示词".to_string(),
            ));
        }

        // 检查是否为已启用的提示词
        let is_enabled = prompt.enabled;

        state.db.save_prompt(app.as_str(), &prompt)?;

        if is_enabled {
            // 启用提示词：写入内容到文件
            let target_path = prompt_file_path(&app)?;
            write_text_file(&target_path, &prompt.content)?;
        } else {
            // 禁用提示词：检查是否还有其他已启用的提示词
            // 使用 _with_hidden：save_prompt 已先执行（上面），故刚被禁用的隐藏行
            // 会被正确计为 disabled——禁用唯一启用的隐藏 CLAUDE.md 行时 any_enabled=false
            // -> 清空文件；若仍有可见提示词启用则保留文件。
            let prompts = state.db.get_prompts_with_hidden(app.as_str())?;
            let any_enabled = prompts.values().any(|p| p.enabled);

            if !any_enabled {
                // 所有提示词都已禁用，清空文件
                let target_path = prompt_file_path(&app)?;
                if target_path.exists() {
                    write_text_file(&target_path, "")?;
                }
            }
        }

        Ok(())
    }

    pub fn delete_prompt(state: &AppState, app: AppType, id: &str) -> Result<(), AppError> {
        // _with_hidden：守卫必须能看到隐藏的 __profile__ 行，若其已启用则拒绝删除。
        let prompts = state.db.get_prompts_with_hidden(app.as_str())?;

        if let Some(prompt) = prompts.get(id) {
            if prompt.enabled {
                return Err(AppError::InvalidInput("无法删除已启用的提示词".to_string()));
            }
        }

        state.db.delete_prompt(app.as_str(), id)?;
        Ok(())
    }

    pub fn enable_prompt(state: &AppState, app: AppType, id: &str) -> Result<(), AppError> {
        // 回填当前 live 文件内容到已启用的提示词，或创建备份
        let target_path = prompt_file_path(&app)?;
        if target_path.exists() {
            if let Ok(live_content) = std::fs::read_to_string(&target_path) {
                if !live_content.trim().is_empty() {
                    // _with_hidden：当前已启用项可能是隐藏的 __profile__ 行。
                    let mut prompts = state.db.get_prompts_with_hidden(app.as_str())?;

                    // 尝试回填到当前已启用的提示词
                    if let Some((enabled_id, enabled_prompt)) = prompts
                        .iter_mut()
                        .find(|(_, p)| p.enabled)
                        .map(|(id, p)| (id.clone(), p))
                    {
                        if enabled_id.starts_with("__profile__:") {
                            // 跳过回填：profile 模板（profile_dotfiles）才是权威，
                            // 绝不把用户手改的 live 内容写回 profile 拥有的隐藏行。
                            log::info!("当前已启用项为 profile 隐藏行，跳过回填: {enabled_id}");
                        } else {
                            let timestamp = get_unix_timestamp()?;
                            enabled_prompt.content = live_content.clone();
                            enabled_prompt.updated_at = Some(timestamp);
                            log::info!("回填 live 提示词内容到已启用项: {enabled_id}");
                            state.db.save_prompt(app.as_str(), enabled_prompt)?;
                        }
                    } else {
                        // 没有已启用的提示词，则创建一次备份（避免重复备份）
                        let content_exists = prompts
                            .values()
                            .any(|p| p.content.trim() == live_content.trim());
                        if !content_exists {
                            let timestamp = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs() as i64;
                            let backup_id = format!("backup-{timestamp}");
                            let backup_prompt = Prompt {
                                id: backup_id.clone(),
                                name: format!(
                                    "原始提示词 {}",
                                    chrono::Local::now().format("%Y-%m-%d %H:%M")
                                ),
                                content: live_content,
                                description: Some("自动备份的原始提示词".to_string()),
                                enabled: false,
                                hidden: false,
                                created_at: Some(timestamp),
                                updated_at: Some(timestamp),
                            };
                            log::info!("回填 live 提示词内容，创建备份: {backup_id}");
                            state.db.save_prompt(app.as_str(), &backup_prompt)?;
                        }
                    }
                }
            }
        }

        // 启用目标提示词并写入文件
        // _with_hidden：sweep 必须覆盖所有行（含隐藏的 __profile__ 行），保证
        // 「最多一个启用项」——启用普通提示词会关闭隐藏行；启用隐藏行会关闭普通行。
        let mut prompts = state.db.get_prompts_with_hidden(app.as_str())?;

        for prompt in prompts.values_mut() {
            prompt.enabled = false;
        }

        if let Some(prompt) = prompts.get_mut(id) {
            prompt.enabled = true;
            write_text_file(&target_path, &prompt.content)?; // 原子写入
            state.db.save_prompt(app.as_str(), prompt)?;
        } else {
            return Err(AppError::InvalidInput(format!("提示词 {id} 不存在")));
        }

        // Save all prompts to disable others
        for (_, prompt) in prompts.iter() {
            state.db.save_prompt(app.as_str(), prompt)?;
        }

        Ok(())
    }

    pub fn import_from_file(state: &AppState, app: AppType) -> Result<String, AppError> {
        let file_path = prompt_file_path(&app)?;

        if !file_path.exists() {
            return Err(AppError::Message("提示词文件不存在".to_string()));
        }

        let content =
            std::fs::read_to_string(&file_path).map_err(|e| AppError::io(&file_path, e))?;
        let timestamp = get_unix_timestamp()?;

        let id = format!("imported-{timestamp}");
        let prompt = Prompt {
            id: id.clone(),
            name: format!(
                "导入的提示词 {}",
                chrono::Local::now().format("%Y-%m-%d %H:%M")
            ),
            content,
            description: Some("从现有配置文件导入".to_string()),
            enabled: false,
            hidden: false,
            created_at: Some(timestamp),
            updated_at: Some(timestamp),
        };

        Self::upsert_prompt(state, app, &id, prompt)?;
        Ok(id)
    }

    pub fn get_current_file_content(app: AppType) -> Result<Option<String>, AppError> {
        let file_path = prompt_file_path(&app)?;
        if !file_path.exists() {
            return Ok(None);
        }
        let content =
            std::fs::read_to_string(&file_path).map_err(|e| AppError::io(&file_path, e))?;
        Ok(Some(content))
    }

    /// 首次启动时从现有提示词文件自动导入（如果存在）
    /// 返回导入的数量
    pub fn import_from_file_on_first_launch(
        state: &AppState,
        app: AppType,
    ) -> Result<usize, AppError> {
        // 幂等性保护：该应用已有提示词则跳过
        let existing = state.db.get_prompts(app.as_str())?;
        if !existing.is_empty() {
            return Ok(0);
        }

        let file_path = prompt_file_path(&app)?;

        // 检查文件是否存在
        if !file_path.exists() {
            return Ok(0);
        }

        // 读取文件内容
        let content = match std::fs::read_to_string(&file_path) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("读取提示词文件失败: {file_path:?}, 错误: {e}");
                return Ok(0);
            }
        };

        // 检查内容是否为空
        if content.trim().is_empty() {
            return Ok(0);
        }

        log::info!("发现提示词文件，自动导入: {file_path:?}");

        // 创建提示词对象
        let timestamp = get_unix_timestamp()?;
        let id = format!("auto-imported-{timestamp}");
        let prompt = Prompt {
            id: id.clone(),
            name: format!(
                "Auto-imported Prompt {}",
                chrono::Local::now().format("%Y-%m-%d %H:%M")
            ),
            content,
            description: Some("Automatically imported on first launch".to_string()),
            enabled: true, // 首次导入时自动启用
            hidden: false,
            created_at: Some(timestamp),
            updated_at: Some(timestamp),
        };

        // 保存到数据库
        state.db.save_prompt(app.as_str(), &prompt)?;

        log::info!("自动导入完成: {}", app.as_str());
        Ok(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;
    use crate::prompt::Prompt;
    use crate::store::AppState;
    use serial_test::serial;
    use std::env;
    use std::sync::Arc;
    use tempfile::TempDir;

    /// 测试用临时 HOME 守卫（镜像 services/profile.rs / database/backup.rs 的模式）。
    /// 任何触碰 ~/.claude/CLAUDE.md 的测试都必须配合 #[serial]。
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

    fn mk_prompt(id: &str, content: &str, enabled: bool, hidden: bool) -> Prompt {
        Prompt {
            id: id.into(),
            name: id.into(),
            content: content.into(),
            description: None,
            enabled,
            hidden,
            created_at: Some(1),
            updated_at: Some(1),
        }
    }

    /// 启用普通提示词必须把隐藏的 __profile__ 行也置为 disabled（single-enabled sweep）。
    #[test]
    #[serial]
    fn enable_normal_prompt_disables_hidden_profile_row() {
        let home = TempHome::new();
        let db = Arc::new(Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());

        // seed a hidden profile-owned row and enable it live first
        db.save_prompt("claude", &mk_prompt("__profile__:p", "P", false, true))
            .expect("save hidden");
        PromptService::enable_prompt(&state, AppType::Claude, "__profile__:p")
            .expect("enable hidden");
        assert_eq!(std::fs::read_to_string(home.claude_md()).unwrap(), "P");

        // seed a visible prompt and enable it
        db.save_prompt("claude", &mk_prompt("n", "N", false, false))
            .expect("save visible");
        PromptService::enable_prompt(&state, AppType::Claude, "n").expect("enable visible");

        let all = db
            .get_prompts_with_hidden("claude")
            .expect("get_prompts_with_hidden");
        assert!(
            !all.get("__profile__:p").unwrap().enabled,
            "hidden profile row must be disabled when a normal prompt is enabled"
        );
        assert!(
            all.get("n").unwrap().enabled,
            "normal prompt must be enabled"
        );
        assert_eq!(std::fs::read_to_string(home.claude_md()).unwrap(), "N");
        assert_eq!(
            all.values().filter(|p| p.enabled).count(),
            1,
            "exactly one row must be enabled"
        );
    }

    /// 再次启用隐藏行时必须跳过回填：profile 模板才是权威，绝不把用户手改的 live 内容写回隐藏行。
    #[test]
    #[serial]
    fn enable_hidden_skips_backfill_no_template_capture() {
        let home = TempHome::new();
        let db = Arc::new(Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());

        db.save_prompt("claude", &mk_prompt("__profile__:p", "P", false, true))
            .expect("save hidden");
        PromptService::enable_prompt(&state, AppType::Claude, "__profile__:p")
            .expect("enable hidden");
        assert_eq!(std::fs::read_to_string(home.claude_md()).unwrap(), "P");

        // user hand-edits the live file
        write_text_file(&home.claude_md(), "USER EDIT").expect("hand edit");

        // re-enable the hidden row -> backfill must be skipped
        PromptService::enable_prompt(&state, AppType::Claude, "__profile__:p")
            .expect("re-enable hidden");

        let row = db
            .get_prompt_with_hidden("claude", "__profile__:p")
            .expect("get hidden")
            .expect("hidden exists");
        assert_eq!(
            row.content, "P",
            "hidden profile row content must NOT capture the user's live edit"
        );
        assert_eq!(
            std::fs::read_to_string(home.claude_md()).unwrap(),
            "P",
            "live file must be restored from the authoritative template"
        );
    }

    /// 禁用唯一启用的隐藏行：没有其他启用项 -> 文件清空。
    #[test]
    #[serial]
    fn disable_lone_hidden_blanks_file() {
        let home = TempHome::new();
        let db = Arc::new(Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());

        db.save_prompt("claude", &mk_prompt("__profile__:p", "P", false, true))
            .expect("save hidden");
        PromptService::enable_prompt(&state, AppType::Claude, "__profile__:p")
            .expect("enable hidden");
        assert_eq!(std::fs::read_to_string(home.claude_md()).unwrap(), "P");

        // disable the lone hidden row via upsert_prompt(enabled=false)
        PromptService::upsert_prompt(
            &state,
            AppType::Claude,
            "__profile__:p",
            mk_prompt("__profile__:p", "P", false, true),
        )
        .expect("disable hidden");

        assert_eq!(
            std::fs::read_to_string(home.claude_md()).unwrap(),
            "",
            "disabling the lone enabled (hidden) row must blank the live file"
        );
    }

    /// 禁用一个已禁用的隐藏行时，若仍有可见提示词启用，则文件保持不变。
    #[test]
    #[serial]
    fn disable_hidden_keeps_file_when_visible_enabled() {
        let home = TempHome::new();
        let db = Arc::new(Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());

        // a disabled hidden row exists
        db.save_prompt("claude", &mk_prompt("__profile__:p", "P", false, true))
            .expect("save hidden");
        // enable a visible prompt
        db.save_prompt("claude", &mk_prompt("n", "N", false, false))
            .expect("save visible");
        PromptService::enable_prompt(&state, AppType::Claude, "n").expect("enable visible");
        assert_eq!(std::fs::read_to_string(home.claude_md()).unwrap(), "N");

        // upsert the (already disabled) hidden row as disabled
        PromptService::upsert_prompt(
            &state,
            AppType::Claude,
            "__profile__:p",
            mk_prompt("__profile__:p", "P", false, true),
        )
        .expect("disable already-disabled hidden");

        assert_eq!(
            std::fs::read_to_string(home.claude_md()).unwrap(),
            "N",
            "file must be kept because the visible prompt is still enabled"
        );
    }
}
