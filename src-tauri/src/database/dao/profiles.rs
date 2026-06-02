//! Profiles 数据访问对象
//!
//! 提供 Profile 的 CRUD 操作及 active profile 管理。
//!
//! profiles 表结构（schema v13）：
//! - id                  TEXT PRIMARY KEY
//! - app_type            TEXT NOT NULL
//! - name                TEXT NOT NULL
//! - description         TEXT
//! - is_active           BOOLEAN NOT NULL DEFAULT 0
//! - current_provider_id TEXT
//! - spec                TEXT NOT NULL DEFAULT '{}'
//! - sort_index          INTEGER NOT NULL DEFAULT 0
//! - created_at          INTEGER NOT NULL DEFAULT 0

use crate::app_config::{Profile, ProfileSpec};
use crate::database::{lock_conn, Database};
use crate::error::AppError;
use rusqlite::params;

impl Database {
    // ========== Profile CRUD ==========

    /// 获取所有 Profiles
    pub fn get_all_profiles(&self) -> Result<Vec<Profile>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT id, app_type, name, description, is_active, current_provider_id,
                        spec, sort_index, created_at
                 FROM profiles ORDER BY sort_index ASC, created_at ASC",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, bool>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                ))
            })
            .map_err(|e| AppError::Database(e.to_string()))?;

        let mut profiles = Vec::new();
        for row_res in rows {
            let (
                id,
                app_type,
                name,
                description,
                is_active,
                current_provider_id,
                spec_raw,
                sort_index,
                created_at,
            ) = row_res.map_err(|e| AppError::Database(e.to_string()))?;
            let spec: ProfileSpec = serde_json::from_str(&spec_raw).unwrap_or_default();
            profiles.push(Profile {
                id,
                app_type,
                name,
                description,
                is_active,
                current_provider_id,
                spec,
                sort_index,
                created_at,
            });
        }
        Ok(profiles)
    }

    /// 获取特定 app_type 的所有 Profiles
    pub fn get_profiles_for_app(&self, app_type: &str) -> Result<Vec<Profile>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT id, app_type, name, description, is_active, current_provider_id,
                        spec, sort_index, created_at
                 FROM profiles WHERE app_type = ?1
                 ORDER BY sort_index ASC, created_at ASC",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

        let rows = stmt
            .query_map([app_type], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, bool>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                ))
            })
            .map_err(|e| AppError::Database(e.to_string()))?;

        let mut profiles = Vec::new();
        for row_res in rows {
            let (
                id,
                app_type,
                name,
                description,
                is_active,
                current_provider_id,
                spec_raw,
                sort_index,
                created_at,
            ) = row_res.map_err(|e| AppError::Database(e.to_string()))?;
            let spec: ProfileSpec = serde_json::from_str(&spec_raw).unwrap_or_default();
            profiles.push(Profile {
                id,
                app_type,
                name,
                description,
                is_active,
                current_provider_id,
                spec,
                sort_index,
                created_at,
            });
        }
        Ok(profiles)
    }

    /// 获取单个 Profile（按 id）
    pub fn get_profile(&self, id: &str) -> Result<Option<Profile>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT id, app_type, name, description, is_active, current_provider_id,
                        spec, sort_index, created_at
                 FROM profiles WHERE id = ?1",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

        let result = stmt.query_row([id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, bool>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, i64>(8)?,
            ))
        });

        match result {
            Ok((
                id,
                app_type,
                name,
                description,
                is_active,
                current_provider_id,
                spec_raw,
                sort_index,
                created_at,
            )) => {
                let spec: ProfileSpec = serde_json::from_str(&spec_raw).unwrap_or_default();
                Ok(Some(Profile {
                    id,
                    app_type,
                    name,
                    description,
                    is_active,
                    current_provider_id,
                    spec,
                    sort_index,
                    created_at,
                }))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AppError::Database(e.to_string())),
        }
    }

    /// 获取特定 app_type 的当前激活 Profile
    pub fn get_active_profile(&self, app_type: &str) -> Result<Option<Profile>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT id, app_type, name, description, is_active, current_provider_id,
                        spec, sort_index, created_at
                 FROM profiles WHERE app_type = ?1 AND is_active = 1 LIMIT 1",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

        let result = stmt.query_row([app_type], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, bool>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, i64>(8)?,
            ))
        });

        match result {
            Ok((
                id,
                app_type,
                name,
                description,
                is_active,
                current_provider_id,
                spec_raw,
                sort_index,
                created_at,
            )) => {
                let spec: ProfileSpec = serde_json::from_str(&spec_raw).unwrap_or_default();
                Ok(Some(Profile {
                    id,
                    app_type,
                    name,
                    description,
                    is_active,
                    current_provider_id,
                    spec,
                    sort_index,
                    created_at,
                }))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AppError::Database(e.to_string())),
        }
    }

    /// 保存 Profile（INSERT OR REPLACE 语义）
    pub fn save_profile(&self, p: &Profile) -> Result<(), AppError> {
        let spec_json = serde_json::to_string(&p.spec)
            .map_err(|e| AppError::Config(format!("spec JSON serialization failed: {e}")))?;
        let conn = lock_conn!(self.conn);
        conn.execute(
            "INSERT OR REPLACE INTO profiles
             (id, app_type, name, description, is_active, current_provider_id,
              spec, sort_index, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                p.id,
                p.app_type,
                p.name,
                p.description,
                p.is_active,
                p.current_provider_id,
                spec_json,
                p.sort_index,
                p.created_at,
            ],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 删除 Profile；返回是否找到该行
    pub fn delete_profile(&self, id: &str) -> Result<bool, AppError> {
        let conn = lock_conn!(self.conn);
        let affected = conn
            .execute("DELETE FROM profiles WHERE id = ?1", params![id])
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(affected > 0)
    }

    /// 设置指定 app_type 的激活 Profile（单行不变量事务）
    /// 先将该 app_type 所有行的 is_active 清零，再将目标行置 1。
    pub fn set_active_profile(&self, app_type: &str, id: &str) -> Result<(), AppError> {
        let mut conn = lock_conn!(self.conn);
        let tx = conn
            .transaction()
            .map_err(|e| AppError::Database(e.to_string()))?;

        tx.execute(
            "UPDATE profiles SET is_active = 0 WHERE app_type = ?1",
            params![app_type],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;

        tx.execute(
            "UPDATE profiles SET is_active = 1 WHERE id = ?1 AND app_type = ?2",
            params![id, app_type],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;

        tx.commit().map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 清除指定 app_type 的激活 Profile（所有行 is_active 置 0）
    pub fn clear_active_profile(&self, app_type: &str) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "UPDATE profiles SET is_active = 0 WHERE app_type = ?1",
            params![app_type],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_config::{ProfileContent, ProfileSpec};
    use crate::database::Database;
    use crate::error::AppError;

    fn make_profile(id: &str, app_type: &str, name: &str) -> Profile {
        Profile {
            id: id.into(),
            app_type: app_type.into(),
            name: name.into(),
            description: Some(format!("desc for {name}")),
            is_active: false,
            current_provider_id: None,
            spec: ProfileSpec {
                content: ProfileContent {
                    skills: vec!["skill-a".into(), "skill-b".into()],
                    commands: vec!["cmd-foo".into()],
                    agents: vec!["agent-x".into()],
                    mcp: vec!["mcp-server-1".into()],
                },
                vars: {
                    let mut m = serde_json::Map::new();
                    m.insert("MY_VAR".into(), serde_json::Value::String("hello".into()));
                    m
                },
            },
            sort_index: 0,
            created_at: 1_000_000,
        }
    }

    #[test]
    fn profile_dao_crud_roundtrip() -> Result<(), AppError> {
        let db = Database::memory()?;
        let p = make_profile("prof:1", "claude", "Work");

        // save then get
        db.save_profile(&p)?;
        let fetched = db.get_profile("prof:1")?.expect("should exist");
        assert_eq!(fetched.id, "prof:1");
        assert_eq!(fetched.app_type, "claude");
        assert_eq!(fetched.name, "Work");
        assert_eq!(fetched.spec.content.skills, vec!["skill-a", "skill-b"]);
        assert_eq!(fetched.spec.content.commands, vec!["cmd-foo"]);
        assert_eq!(fetched.spec.content.agents, vec!["agent-x"]);
        assert_eq!(fetched.spec.content.mcp, vec!["mcp-server-1"]);
        assert_eq!(
            fetched.spec.vars.get("MY_VAR").and_then(|v| v.as_str()),
            Some("hello")
        );

        // get_all_profiles should have exactly 1
        let all = db.get_all_profiles()?;
        assert_eq!(all.len(), 1);

        // delete returns true
        assert!(db.delete_profile("prof:1")?);

        // after delete, get returns None
        assert!(db.get_profile("prof:1")?.is_none());

        Ok(())
    }

    #[test]
    fn set_active_profile_single_row_invariant_per_app() -> Result<(), AppError> {
        let db = Database::memory()?;

        // Insert 3 claude profiles
        let pa = make_profile("claude:A", "claude", "Profile A");
        let pb = make_profile("claude:B", "claude", "Profile B");
        let pc = make_profile("claude:C", "claude", "Profile C");
        db.save_profile(&pa)?;
        db.save_profile(&pb)?;
        db.save_profile(&pc)?;

        // Set A active, then B active — only B should remain active
        db.set_active_profile("claude", "claude:A")?;
        db.set_active_profile("claude", "claude:B")?;

        let claude_profiles = db.get_profiles_for_app("claude")?;
        let active_count = claude_profiles.iter().filter(|p| p.is_active).count();
        assert_eq!(
            active_count, 1,
            "exactly one claude profile should be active"
        );
        let active = db
            .get_active_profile("claude")?
            .expect("should have active claude profile");
        assert_eq!(active.id, "claude:B");

        // Insert a codex profile and set it active
        let codex_p = make_profile("codex:X", "codex", "Codex Profile X");
        db.save_profile(&codex_p)?;
        db.set_active_profile("codex", "codex:X")?;

        // Claude active row should be unaffected (per-app scoping)
        let claude_active = db
            .get_active_profile("claude")?
            .expect("claude active should still be B");
        assert_eq!(claude_active.id, "claude:B");

        // Codex active should be X
        let codex_active = db
            .get_active_profile("codex")?
            .expect("codex:X should be active");
        assert_eq!(codex_active.id, "codex:X");

        Ok(())
    }
}
