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
             (channel, profile_id, project_id, app_type, target_path, kind, content_hash, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                e.channel,
                e.profile_id,
                e.project_id,
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
                "SELECT id, channel, profile_id, project_id, app_type, target_path, kind, content_hash, created_at
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
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, i64>(8)?,
                ))
            })
            .map_err(|e| AppError::Database(e.to_string()))?;

        let mut entries = Vec::new();
        for row_res in rows {
            let (id, channel, p_id, proj_id, a_type, target_path, kind, content_hash, created_at) =
                row_res.map_err(|e| AppError::Database(e.to_string()))?;
            entries.push(ManifestEntry {
                id,
                channel,
                profile_id: p_id,
                project_id: proj_id,
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

    /// 获取某 channel 的全部 manifest 记录（项目通道 = "project:<canonpath>"）。
    /// 与 get_manifest_for_profile 不同：仅按 channel 过滤，确保全局与项目互不串扰。
    pub fn get_manifest_for_channel(&self, channel: &str) -> Result<Vec<ManifestEntry>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT id, channel, profile_id, project_id, app_type, target_path, kind, content_hash, created_at
                 FROM apply_manifest
                 WHERE channel = ?1
                 ORDER BY id ASC",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([channel], |row| {
                Ok(ManifestEntry {
                    id: row.get(0)?,
                    channel: row.get(1)?,
                    profile_id: row.get(2)?,
                    project_id: row.get(3)?,
                    app_type: row.get(4)?,
                    target_path: row.get(5)?,
                    kind: row.get(6)?,
                    content_hash: row.get(7)?,
                    created_at: row.get(8)?,
                })
            })
            .map_err(|e| AppError::Database(e.to_string()))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| AppError::Database(e.to_string()))?);
        }
        Ok(out)
    }

    /// 清除某 channel 的全部 manifest 记录（仅该 channel；全局 / 其它项目不受影响）。
    pub fn clear_manifest_for_channel(&self, channel: &str) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "DELETE FROM apply_manifest WHERE channel = ?1",
            params![channel],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 列出所有项目通道（channel LIKE 'project:%'）的去重列表，供路径失效清理使用。
    pub fn get_all_project_channels(&self) -> Result<Vec<String>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare("SELECT DISTINCT channel FROM apply_manifest WHERE channel LIKE 'project:%'")
            .map_err(|e| AppError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| AppError::Database(e.to_string()))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| AppError::Database(e.to_string()))?);
        }
        Ok(out)
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
            project_id: None,
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

    #[test]
    fn manifest_channel_scoped_queries() -> Result<(), AppError> {
        let db = Database::memory()?;
        // project_id has NO FK (Task 1 design decision) — no projects row needed.
        let chan = "project:/abs/repo";

        // one global row + two project rows on the same channel
        let g = ManifestEntry {
            id: 0,
            channel: "global".into(),
            profile_id: None,
            project_id: None,
            app_type: "claude".into(),
            target_path: "/g".into(),
            kind: "command".into(),
            content_hash: None,
            created_at: 0,
        };
        db.record_manifest_entry(&g)?;
        for tp in [
            "/abs/repo/.claude/commands/a.md",
            "/abs/repo/.claude/agents/b.md",
        ] {
            db.record_manifest_entry(&ManifestEntry {
                id: 0,
                channel: chan.into(),
                profile_id: None,
                project_id: Some("proj:c".into()),
                app_type: "claude".into(),
                target_path: tp.into(),
                kind: "command".into(),
                content_hash: Some("h".into()),
                created_at: 0,
            })?;
        }

        let chan_rows = db.get_manifest_for_channel(chan)?;
        assert_eq!(chan_rows.len(), 2, "only the 2 project-channel rows");
        assert!(chan_rows.iter().all(|r| r.channel == chan));
        assert!(chan_rows
            .iter()
            .all(|r| r.project_id.as_deref() == Some("proj:c")));

        let channels = db.get_all_project_channels()?;
        assert!(channels.contains(&chan.to_string()));
        assert!(
            !channels.contains(&"global".to_string()),
            "global is not a project channel"
        );

        db.clear_manifest_for_channel(chan)?;
        assert_eq!(db.get_manifest_for_channel(chan)?.len(), 0);
        // global row untouched
        let conn = crate::database::lock_conn!(db.conn);
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM apply_manifest WHERE channel='global'",
                [],
                |r| r.get(0),
            )
            .expect("count global");
        assert_eq!(n, 1, "global rows must survive channel clear");
        Ok(())
    }

    #[test]
    fn manifest_entry_carries_project_id() -> Result<(), AppError> {
        let db = Database::memory()?;
        // NOTE: project_id has NO FK (design decision, Task 1), so a manifest row
        // may carry any project_id string without a matching projects row.
        let e = ManifestEntry {
            id: 0,
            channel: "project:/abs/repo".into(),
            profile_id: None,
            project_id: Some("proj:m".into()),
            app_type: "claude".into(),
            target_path: "/abs/repo/.claude/commands/foo.md".into(),
            kind: "command".into(),
            content_hash: Some("h".into()),
            created_at: 0,
        };
        let id = db.record_manifest_entry(&e)?;
        assert!(id > 0);
        // read back via the project-channel getter (added in Task 4b) — for now read raw:
        let conn = crate::database::lock_conn!(db.conn);
        let pid: Option<String> = conn
            .query_row(
                "SELECT project_id FROM apply_manifest WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .expect("query project_id");
        assert_eq!(pid.as_deref(), Some("proj:m"));
        Ok(())
    }
}
