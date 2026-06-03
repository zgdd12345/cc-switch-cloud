//! Projects 数据访问对象（schema v17，设备本地）。

use crate::app_config::Project;
use crate::database::{lock_conn, Database};
use crate::error::AppError;
use rusqlite::params;

impl Database {
    /// 获取所有 Projects（设备本地）
    pub fn get_all_projects(&self) -> Result<Vec<Project>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT id, project_path, entered_path, app_type, name, spec, enabled,
                        created_at, updated_at
                 FROM projects ORDER BY created_at ASC, id ASC",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], Self::map_project_row)
            .map_err(|e| AppError::Database(e.to_string()))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| AppError::Database(e.to_string()))?);
        }
        Ok(out)
    }

    /// 获取某 app_type 的所有 Projects
    pub fn get_projects_for_app(&self, app_type: &str) -> Result<Vec<Project>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT id, project_path, entered_path, app_type, name, spec, enabled,
                        created_at, updated_at
                 FROM projects WHERE app_type = ?1 ORDER BY created_at ASC, id ASC",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([app_type], Self::map_project_row)
            .map_err(|e| AppError::Database(e.to_string()))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| AppError::Database(e.to_string()))?);
        }
        Ok(out)
    }

    /// 获取单个 Project（按 id）
    pub fn get_project(&self, id: &str) -> Result<Option<Project>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT id, project_path, entered_path, app_type, name, spec, enabled,
                        created_at, updated_at
                 FROM projects WHERE id = ?1",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        match stmt.query_row([id], Self::map_project_row) {
            Ok(p) => Ok(Some(p)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AppError::Database(e.to_string())),
        }
    }

    /// 按规范路径 + app 查找 Project（UNIQUE 键查询，用于绑定冲突检测）
    pub fn get_project_by_path_and_app(
        &self,
        project_path: &str,
        app_type: &str,
    ) -> Result<Option<Project>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT id, project_path, entered_path, app_type, name, spec, enabled,
                        created_at, updated_at
                 FROM projects WHERE project_path = ?1 AND app_type = ?2",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        match stmt.query_row(params![project_path, app_type], Self::map_project_row) {
            Ok(p) => Ok(Some(p)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AppError::Database(e.to_string())),
        }
    }

    /// 保存 Project（INSERT OR REPLACE on PK id；UNIQUE(project_path, app_type) 仍会拒绝冲突）
    pub fn save_project(&self, p: &Project) -> Result<(), AppError> {
        let spec_json = serde_json::to_string(&p.spec)
            .map_err(|e| AppError::Config(format!("project spec serialize failed: {e}")))?;
        let conn = lock_conn!(self.conn);
        conn.execute(
            "INSERT INTO projects
             (id, project_path, entered_path, app_type, name, spec, enabled,
              created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(id) DO UPDATE SET
               project_path = excluded.project_path,
               entered_path = excluded.entered_path,
               app_type     = excluded.app_type,
               name         = excluded.name,
               spec         = excluded.spec,
               enabled      = excluded.enabled,
               created_at   = excluded.created_at,
               updated_at   = excluded.updated_at",
            params![
                p.id,
                p.project_path,
                p.entered_path,
                p.app_type,
                p.name,
                spec_json,
                p.enabled,
                p.created_at,
                p.updated_at,
            ],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 删除 Project；返回是否找到行。注意：project_id 列无 FK CASCADE（设计决策，见 Task 1），
    /// 该项目残留的 manifest 行由 detach / `prune_orphan_project_channels`（Task 9）清理。
    pub fn delete_project(&self, id: &str) -> Result<bool, AppError> {
        let conn = lock_conn!(self.conn);
        let affected = conn
            .execute("DELETE FROM projects WHERE id = ?1", params![id])
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(affected > 0)
    }

    /// 行映射助手（projects 表 → Project）。
    fn map_project_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Project> {
        let spec_raw: String = row.get(5)?;
        let spec = serde_json::from_str(&spec_raw).unwrap_or_default();
        Ok(Project {
            id: row.get(0)?,
            project_path: row.get(1)?,
            entered_path: row.get(2)?,
            app_type: row.get(3)?,
            name: row.get(4)?,
            spec,
            enabled: row.get(6)?,
            created_at: row.get(7)?,
            updated_at: row.get(8)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_config::{ProfileContent, ProjectSpec};
    use crate::database::Database;

    fn make_project(id: &str, canon: &str, app: &str) -> Project {
        Project {
            id: id.into(),
            project_path: canon.into(),
            entered_path: format!("entered:{canon}"),
            app_type: app.into(),
            name: Some(format!("name:{id}")),
            spec: ProjectSpec {
                content: ProfileContent {
                    skills: vec!["skill-a".into()],
                    commands: vec!["cmd-foo".into()],
                    agents: vec!["agent-x".into()],
                    mcp: vec![],
                },
                vars: serde_json::Map::new(),
            },
            enabled: true,
            created_at: 100,
            updated_at: 200,
        }
    }

    #[test]
    fn project_dao_crud_roundtrip() -> Result<(), AppError> {
        let db = Database::memory()?;
        let p = make_project("proj:1", "/abs/repo", "claude");
        db.save_project(&p)?;

        let got = db.get_project("proj:1")?.expect("exists");
        assert_eq!(got.project_path, "/abs/repo");
        assert_eq!(got.entered_path, "entered:/abs/repo");
        assert_eq!(got.spec.content.skills, vec!["skill-a"]);
        assert!(got.enabled);

        assert_eq!(db.get_all_projects()?.len(), 1);
        assert_eq!(db.get_projects_for_app("claude")?.len(), 1);
        assert_eq!(db.get_projects_for_app("codex")?.len(), 0);

        let by_path = db
            .get_project_by_path_and_app("/abs/repo", "claude")?
            .expect("found by path");
        assert_eq!(by_path.id, "proj:1");

        // update via save (INSERT OR REPLACE on PK id)
        let mut p2 = got.clone();
        p2.enabled = false;
        p2.name = Some("renamed".into());
        db.save_project(&p2)?;
        let got2 = db.get_project("proj:1")?.expect("exists");
        assert!(!got2.enabled);
        assert_eq!(got2.name.as_deref(), Some("renamed"));

        assert!(db.delete_project("proj:1")?);
        assert!(db.get_project("proj:1")?.is_none());
        Ok(())
    }

    #[test]
    fn project_unique_path_app_enforced() -> Result<(), AppError> {
        let db = Database::memory()?;
        db.save_project(&make_project("proj:1", "/abs/repo", "claude"))?;
        // same canonical path + app under a DIFFERENT id violates UNIQUE(project_path, app_type)
        let dup = make_project("proj:2", "/abs/repo", "claude");
        let err = db.save_project(&dup);
        assert!(err.is_err(), "duplicate (path, app) must be rejected");
        Ok(())
    }
}
