//! Agents 数据访问对象
//!
//! 提供 InstalledAgent 的 CRUD 操作。
//!
//! v2.5+ agents 表结构：
//! - id TEXT PRIMARY KEY
//! - name TEXT NOT NULL
//! - content TEXT NOT NULL DEFAULT ''
//! - description TEXT
//! - tags TEXT  (JSON array, e.g. '["core","fix"]')
//! - enabled_claude BOOLEAN NOT NULL DEFAULT 0
//! - installed_at INTEGER NOT NULL DEFAULT 0

use crate::app_config::InstalledAgent;
use crate::database::{lock_conn, Database};
use crate::error::AppError;
use rusqlite::params;

impl Database {
    // ========== InstalledAgent CRUD ==========

    /// 获取所有已安装的 Agents
    pub fn get_all_installed_agents(&self) -> Result<Vec<InstalledAgent>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT id, name, content, description, tags, enabled_claude, installed_at
                 FROM agents ORDER BY name ASC",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

        let agent_iter = stmt
            .query_map([], |row| {
                let tags_raw: Option<String> = row.get(4)?;
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    tags_raw,
                    row.get::<_, bool>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            })
            .map_err(|e| AppError::Database(e.to_string()))?;

        let mut agents = Vec::new();
        for row_res in agent_iter {
            let (id, name, content, description, tags_raw, enabled_claude, installed_at) =
                row_res.map_err(|e| AppError::Database(e.to_string()))?;
            let tags = parse_tags(tags_raw.as_deref());
            agents.push(InstalledAgent {
                id,
                name,
                content,
                description,
                tags,
                enabled_claude,
                installed_at,
            });
        }
        Ok(agents)
    }

    /// 获取单个已安装的 Agent
    pub fn get_installed_agent(&self, id: &str) -> Result<Option<InstalledAgent>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT id, name, content, description, tags, enabled_claude, installed_at
                 FROM agents WHERE id = ?1",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

        let result = stmt.query_row([id], |row| {
            let tags_raw: Option<String> = row.get(4)?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                tags_raw,
                row.get::<_, bool>(5)?,
                row.get::<_, i64>(6)?,
            ))
        });

        match result {
            Ok((id, name, content, description, tags_raw, enabled_claude, installed_at)) => {
                let tags = parse_tags(tags_raw.as_deref());
                Ok(Some(InstalledAgent {
                    id,
                    name,
                    content,
                    description,
                    tags,
                    enabled_claude,
                    installed_at,
                }))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AppError::Database(e.to_string())),
        }
    }

    /// 保存 Agent（添加或更新，INSERT OR REPLACE 语义）
    pub fn save_agent(&self, a: &InstalledAgent) -> Result<(), AppError> {
        let tags_json = serde_json::to_string(&a.tags)
            .map_err(|e| AppError::Config(format!("tags JSON serialization failed: {e}")))?;
        let conn = lock_conn!(self.conn);
        conn.execute(
            "INSERT OR REPLACE INTO agents
             (id, name, content, description, tags, enabled_claude, installed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                a.id,
                a.name,
                a.content,
                a.description,
                tags_json,
                a.enabled_claude,
                a.installed_at,
            ],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 更新 Agent 的 Claude 启用状态；返回是否找到该行
    pub fn set_agent_enabled(&self, id: &str, enabled: bool) -> Result<bool, AppError> {
        let conn = lock_conn!(self.conn);
        let affected = conn
            .execute(
                "UPDATE agents SET enabled_claude = ?1 WHERE id = ?2",
                params![enabled, id],
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(affected > 0)
    }

    /// 删除 Agent；返回是否找到该行
    pub fn delete_agent(&self, id: &str) -> Result<bool, AppError> {
        let conn = lock_conn!(self.conn);
        let affected = conn
            .execute("DELETE FROM agents WHERE id = ?1", params![id])
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(affected > 0)
    }
}

/// 解析 tags 列：JSON 数组文本 → Vec<String>；NULL 或空字符串 → vec![]
fn parse_tags(raw: Option<&str>) -> Vec<String> {
    match raw {
        None | Some("") => vec![],
        Some(s) => serde_json::from_str(s).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;
    use crate::error::AppError;

    #[test]
    fn agent_crud_roundtrip() -> Result<(), AppError> {
        let db = Database::memory()?;
        let a = InstalledAgent {
            id: "local:my-agent".into(),
            name: "my-agent".into(),
            content: "# my-agent\n".into(),
            description: Some("d".into()),
            tags: vec!["core".into()],
            enabled_claude: false,
            installed_at: 1,
        };
        db.save_agent(&a)?;
        let all = db.get_all_installed_agents()?;
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].tags, vec!["core".to_string()]);
        db.set_agent_enabled("local:my-agent", true)?;
        assert!(
            db.get_installed_agent("local:my-agent")?
                .unwrap()
                .enabled_claude
        );
        // upsert: save same id again with new content
        let mut a2 = a.clone();
        a2.content = "# my-agent2\n".into();
        db.save_agent(&a2)?;
        assert_eq!(db.get_all_installed_agents()?.len(), 1);
        assert_eq!(
            db.get_installed_agent("local:my-agent")?.unwrap().content,
            "# my-agent2\n"
        );
        assert!(db.delete_agent("local:my-agent")?);
        assert_eq!(db.get_all_installed_agents()?.len(), 0);
        Ok(())
    }
}
