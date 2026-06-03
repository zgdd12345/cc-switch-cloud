//! 提示词数据访问对象
//!
//! 提供提示词（Prompt）的 CRUD 操作。

use crate::database::{lock_conn, Database};
use crate::error::AppError;
use crate::prompt::Prompt;
use indexmap::IndexMap;
use rusqlite::params;

/// 公共列清单：所有 SELECT 与行映射保持一致
const PROMPT_COLUMNS: &str =
    "id, name, content, description, enabled, created_at, updated_at, hidden";

/// 行 -> Prompt 映射器（列顺序须与 PROMPT_COLUMNS 一致）
fn row_to_prompt(row: &rusqlite::Row<'_>) -> rusqlite::Result<Prompt> {
    Ok(Prompt {
        id: row.get(0)?,
        name: row.get(1)?,
        content: row.get(2)?,
        description: row.get(3)?,
        enabled: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
        hidden: row.get(7)?,
    })
}

impl Database {
    /// 获取指定应用类型的所有可见提示词（hidden = 0）
    ///
    /// UI 安全：由 profile 派生的 `__profile__` 等隐藏行不会出现在
    /// Prompts 列表中，也不影响首次启动导入的幂等性。
    pub fn get_prompts(&self, app_type: &str) -> Result<IndexMap<String, Prompt>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {PROMPT_COLUMNS}
             FROM prompts WHERE app_type = ?1 AND hidden = 0
             ORDER BY created_at ASC, id ASC"
            ))
            .map_err(|e| AppError::Database(e.to_string()))?;

        let prompt_iter = stmt
            .query_map(params![app_type], row_to_prompt)
            .map_err(|e| AppError::Database(e.to_string()))?;

        let mut prompts = IndexMap::new();
        for prompt_res in prompt_iter {
            let prompt = prompt_res.map_err(|e| AppError::Database(e.to_string()))?;
            prompts.insert(prompt.id.clone(), prompt);
        }
        Ok(prompts)
    }

    /// 获取指定应用类型的所有提示词（包含 hidden = 1 的隐藏行）
    pub fn get_prompts_with_hidden(
        &self,
        app_type: &str,
    ) -> Result<IndexMap<String, Prompt>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {PROMPT_COLUMNS}
             FROM prompts WHERE app_type = ?1
             ORDER BY created_at ASC, id ASC"
            ))
            .map_err(|e| AppError::Database(e.to_string()))?;

        let prompt_iter = stmt
            .query_map(params![app_type], row_to_prompt)
            .map_err(|e| AppError::Database(e.to_string()))?;

        let mut prompts = IndexMap::new();
        for prompt_res in prompt_iter {
            let prompt = prompt_res.map_err(|e| AppError::Database(e.to_string()))?;
            prompts.insert(prompt.id.clone(), prompt);
        }
        Ok(prompts)
    }

    /// 获取单条提示词（包含 hidden = 1 的隐藏行）
    pub fn get_prompt_with_hidden(
        &self,
        app_type: &str,
        id: &str,
    ) -> Result<Option<Prompt>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {PROMPT_COLUMNS}
             FROM prompts WHERE app_type = ?1 AND id = ?2"
            ))
            .map_err(|e| AppError::Database(e.to_string()))?;

        let result = stmt.query_row(params![app_type, id], row_to_prompt);
        match result {
            Ok(prompt) => Ok(Some(prompt)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AppError::Database(e.to_string())),
        }
    }

    /// 保存提示词
    pub fn save_prompt(&self, app_type: &str, prompt: &Prompt) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "INSERT OR REPLACE INTO prompts (
                id, app_type, name, content, description, enabled, created_at, updated_at, hidden
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                prompt.id,
                app_type,
                prompt.name,
                prompt.content,
                prompt.description,
                prompt.enabled,
                prompt.created_at,
                prompt.updated_at,
                prompt.hidden,
            ],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 删除提示词
    pub fn delete_prompt(&self, app_type: &str, id: &str) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "DELETE FROM prompts WHERE id = ?1 AND app_type = ?2",
            params![id, app_type],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }
}
