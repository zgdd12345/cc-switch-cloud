//! ProfileDotfile 数据访问对象
//!
//! 提供 profile_dotfiles 表的 CRUD 操作。
//!
//! profile_dotfiles 表结构（schema v13）：
//! - profile_id TEXT NOT NULL  (FK → profiles.id ON DELETE CASCADE)
//! - rel_path   TEXT NOT NULL
//! - content    TEXT NOT NULL DEFAULT ''
//! - PRIMARY KEY (profile_id, rel_path)

use crate::app_config::ProfileDotfile;
use crate::database::{lock_conn, Database};
use crate::error::AppError;
use rusqlite::params;

impl Database {
    // ========== ProfileDotfile CRUD ==========

    /// 获取某个 Profile 的所有 dotfiles
    pub fn get_profile_dotfiles(&self, profile_id: &str) -> Result<Vec<ProfileDotfile>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT profile_id, rel_path, content
                 FROM profile_dotfiles WHERE profile_id = ?1
                 ORDER BY rel_path ASC",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

        let rows = stmt
            .query_map([profile_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| AppError::Database(e.to_string()))?;

        let mut dotfiles = Vec::new();
        for row_res in rows {
            let (profile_id, rel_path, content) =
                row_res.map_err(|e| AppError::Database(e.to_string()))?;
            dotfiles.push(ProfileDotfile {
                profile_id,
                rel_path,
                content,
            });
        }
        Ok(dotfiles)
    }

    /// 获取单个 dotfile（按 profile_id + rel_path）
    pub fn get_profile_dotfile(
        &self,
        profile_id: &str,
        rel_path: &str,
    ) -> Result<Option<ProfileDotfile>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT profile_id, rel_path, content
                 FROM profile_dotfiles WHERE profile_id = ?1 AND rel_path = ?2",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

        let result = stmt.query_row([profile_id, rel_path], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        });

        match result {
            Ok((profile_id, rel_path, content)) => Ok(Some(ProfileDotfile {
                profile_id,
                rel_path,
                content,
            })),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AppError::Database(e.to_string())),
        }
    }

    /// 保存 dotfile（INSERT OR REPLACE 语义，PK = profile_id + rel_path）
    pub fn set_profile_dotfile(
        &self,
        profile_id: &str,
        rel_path: &str,
        content: &str,
    ) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "INSERT OR REPLACE INTO profile_dotfiles(profile_id, rel_path, content) VALUES(?1, ?2, ?3)",
            params![profile_id, rel_path, content],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 删除单个 dotfile；返回是否找到该行
    pub fn delete_profile_dotfile(
        &self,
        profile_id: &str,
        rel_path: &str,
    ) -> Result<bool, AppError> {
        let conn = lock_conn!(self.conn);
        let affected = conn
            .execute(
                "DELETE FROM profile_dotfiles WHERE profile_id = ?1 AND rel_path = ?2",
                params![profile_id, rel_path],
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(affected > 0)
    }

    /// 删除某个 Profile 的所有 dotfiles
    pub fn delete_all_profile_dotfiles(&self, profile_id: &str) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "DELETE FROM profile_dotfiles WHERE profile_id = ?1",
            params![profile_id],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::app_config::ProfileSpec;
    use crate::database::Database;
    use crate::error::AppError;

    /// Insert a minimal profile row so FK constraints are satisfied.
    fn seed_profile(db: &Database, id: &str) -> Result<(), AppError> {
        let spec = serde_json::to_string(&ProfileSpec::default())
            .map_err(|e| AppError::Config(e.to_string()))?;
        let conn = crate::database::lock_conn!(db.conn);
        conn.execute(
            "INSERT OR IGNORE INTO profiles(id, app_type, name, is_active, spec, sort_index, created_at)
             VALUES(?1, 'claude', ?1, 0, ?2, 0, 0)",
            rusqlite::params![id, spec],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    #[test]
    fn profile_dotfiles_crud_roundtrip() -> Result<(), AppError> {
        let db = Database::memory()?;
        let p = "prof:dotfile-test";
        seed_profile(&db, p)?;

        // Insert two dotfiles
        db.set_profile_dotfile(p, "settings.json", "{}")?;
        db.set_profile_dotfile(p, "statusline.sh", "echo hi")?;

        // get_profile_dotfiles returns both
        let all = db.get_profile_dotfiles(p)?;
        assert_eq!(all.len(), 2);

        // get_profile_dotfile returns correct content
        let df = db
            .get_profile_dotfile(p, "settings.json")?
            .expect("settings.json should exist");
        assert_eq!(df.content, "{}");

        // set same rel_path again updates (INSERT OR REPLACE)
        db.set_profile_dotfile(p, "settings.json", r#"{"model":"claude-opus-4-5"}"#)?;
        let updated = db
            .get_profile_dotfile(p, "settings.json")?
            .expect("settings.json should still exist");
        assert_eq!(updated.content, r#"{"model":"claude-opus-4-5"}"#);
        // count should still be 2
        assert_eq!(db.get_profile_dotfiles(p)?.len(), 2);

        // delete_profile_dotfile returns true, then None
        assert!(db.delete_profile_dotfile(p, "settings.json")?);
        assert!(db.get_profile_dotfile(p, "settings.json")?.is_none());

        // delete_all_profile_dotfiles clears remaining
        db.delete_all_profile_dotfiles(p)?;
        assert_eq!(db.get_profile_dotfiles(p)?.len(), 0);

        Ok(())
    }
}
