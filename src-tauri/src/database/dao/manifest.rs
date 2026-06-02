//! ManifestEntry 数据访问对象
//!
//! 提供 apply_manifest 表的读写操作。
//!
//! apply_manifest 表结构（schema v13/v14）：
//! - id           INTEGER PRIMARY KEY AUTOINCREMENT
//! - channel      TEXT NOT NULL DEFAULT 'global'
//! - profile_id   TEXT  (nullable, FK → profiles.id ON DELETE CASCADE)
//! - app_type     TEXT NOT NULL
//! - target_path  TEXT NOT NULL
//! - kind         TEXT NOT NULL
//! - created_at   INTEGER NOT NULL DEFAULT 0
//! - content_hash TEXT  (nullable, added in v14)

use crate::app_config::ManifestEntry;
use crate::database::{lock_conn, Database};
use crate::error::AppError;
use rusqlite::params;

impl Database {
    // ========== ManifestEntry operations ==========

    /// 写入一条 manifest 记录；忽略 entry.id，返回自增主键
    pub fn record_manifest_entry(&self, e: &ManifestEntry) -> Result<i64, AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "INSERT INTO apply_manifest
             (channel, profile_id, app_type, target_path, kind, content_hash, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                e.channel,
                e.profile_id,
                e.app_type,
                e.target_path,
                e.kind,
                e.content_hash,
                e.created_at,
            ],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(conn.last_insert_rowid())
    }

    /// 获取某个 Profile + app_type 的全部 manifest 记录
    pub fn get_manifest_for_profile(
        &self,
        profile_id: &str,
        app_type: &str,
    ) -> Result<Vec<ManifestEntry>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT id, channel, profile_id, app_type, target_path, kind, content_hash, created_at
                 FROM apply_manifest
                 WHERE profile_id = ?1 AND app_type = ?2
                 ORDER BY id ASC",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

        let rows = stmt
            .query_map([profile_id, app_type], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            })
            .map_err(|e| AppError::Database(e.to_string()))?;

        let mut entries = Vec::new();
        for row_res in rows {
            let (id, channel, p_id, a_type, target_path, kind, content_hash, created_at) =
                row_res.map_err(|e| AppError::Database(e.to_string()))?;
            entries.push(ManifestEntry {
                id,
                channel,
                profile_id: p_id,
                app_type: a_type,
                target_path,
                kind,
                content_hash,
                created_at,
            });
        }
        Ok(entries)
    }

    /// 按 id 列表批量删除 manifest 记录；空切片时为 no-op
    pub fn delete_manifest_entries(&self, ids: &[i64]) -> Result<(), AppError> {
        if ids.is_empty() {
            return Ok(());
        }
        let placeholders: Vec<String> = ids.iter().map(|_| "?".to_string()).collect();
        let sql = format!(
            "DELETE FROM apply_manifest WHERE id IN ({})",
            placeholders.join(", ")
        );
        let conn = lock_conn!(self.conn);
        let params_vec: Vec<&dyn rusqlite::types::ToSql> = ids
            .iter()
            .map(|id| id as &dyn rusqlite::types::ToSql)
            .collect();
        conn.execute(&sql, params_vec.as_slice())
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 清除某个 Profile + app_type 的全部 manifest 记录
    pub fn clear_manifest_for_profile(
        &self,
        profile_id: &str,
        app_type: &str,
    ) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "DELETE FROM apply_manifest WHERE profile_id = ?1 AND app_type = ?2",
            params![profile_id, app_type],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn make_entry(profile_id: &str, app_type: &str, target_path: &str) -> ManifestEntry {
        ManifestEntry {
            id: 0, // ignored on insert
            channel: "global".into(),
            profile_id: Some(profile_id.into()),
            app_type: app_type.into(),
            target_path: target_path.into(),
            kind: "whole_file".into(),
            content_hash: Some("abc123".into()),
            created_at: 0,
        }
    }

    #[test]
    fn manifest_crud_roundtrip() -> Result<(), AppError> {
        let db = Database::memory()?;
        let p = "prof:manifest-test";
        seed_profile(&db, p)?;

        // record_manifest_entry returns an id
        let e1 = make_entry(p, "claude", "/home/user/.claude/settings.json");
        let id1 = db.record_manifest_entry(&e1)?;
        assert!(id1 > 0);

        // get_manifest_for_profile returns the entry
        let rows = db.get_manifest_for_profile(p, "claude")?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, id1);
        assert_eq!(rows[0].target_path, "/home/user/.claude/settings.json");
        assert_eq!(rows[0].content_hash, Some("abc123".into()));

        // record a second entry
        let e2 = make_entry(p, "claude", "/home/user/.claude/CLAUDE.md");
        let id2 = db.record_manifest_entry(&e2)?;
        assert!(id2 > id1);
        assert_eq!(db.get_manifest_for_profile(p, "claude")?.len(), 2);

        // delete_manifest_entries(&[id1]) leaves one
        db.delete_manifest_entries(&[id1])?;
        let remaining = db.get_manifest_for_profile(p, "claude")?;
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, id2);

        // clear_manifest_for_profile removes the rest
        db.clear_manifest_for_profile(p, "claude")?;
        assert_eq!(db.get_manifest_for_profile(p, "claude")?.len(), 0);

        Ok(())
    }
}
