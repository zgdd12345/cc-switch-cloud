//! Profile dotfile rendering — whole-file renderer (e.g. statusline.sh).
//!
//! A "whole file" dotfile is written verbatim into the Claude config dir
//! (`~/.claude/<rel_path>`). To avoid clobbering files a user wrote (or hand
//! edited), the renderer only overwrites a file when we can prove we own it:
//! the file on disk hashes to the `prior_owned_hash` recorded in the manifest.
//! Anything else (missing prior hash, or a hash mismatch meaning the user
//! edited it) is left untouched and reported as a skip (`Ok(None)`).
//!
//! NOTE: These functions are the public renderer API consumed by the
//! profile-apply orchestration (T5: `ProfileService::activate`/`deactivate`).
//! `render_whole_file` and `remove_whole_file_if_owned` are now wired in there;
//! `validate_rel_path` / `content_hash` are reached transitively via them. The
//! previously-needed module-scoped `#![allow(dead_code)]` has been removed now
//! that T5 consumes this API.

use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::app_config::{AppType, ManifestEntry};
use crate::config::{delete_file, get_claude_config_dir, write_text_file};
use crate::database::Database;
use crate::error::AppError;

/// Walk up the path hierarchy from `path` until we find the longest existing
/// ancestor (or `path` itself if it exists).  Returns `None` only if `path`
/// has no parent and doesn't exist (shouldn't happen for absolute paths under a
/// tempdir/home).
fn nearest_existing_ancestor(path: &Path) -> Option<PathBuf> {
    let mut cur = path;
    loop {
        if cur.exists() {
            return Some(cur.to_path_buf());
        }
        match cur.parent() {
            Some(parent) => cur = parent,
            None => return None,
        }
    }
}

/// Validate a profile-supplied relative dotfile path and resolve it under the
/// Claude config dir.
///
/// Rejects empty paths, absolute paths, and any traversal component (`..`,
/// root, or a Windows prefix). Returns the resolved absolute path, which is
/// guaranteed to live under `get_claude_config_dir()`.
///
/// The file is NOT required to exist; the component check plus the
/// `starts_with` assertion is sufficient to prevent escaping the config dir.
pub fn validate_rel_path(rel_path: &str) -> Result<PathBuf, AppError> {
    if rel_path.trim().is_empty() {
        return Err(AppError::InvalidInput("dotfile rel_path 为空".to_string()));
    }
    if rel_path.starts_with('/') {
        return Err(AppError::InvalidInput(format!(
            "dotfile rel_path 不能为绝对路径: {rel_path}"
        )));
    }

    for comp in Path::new(rel_path).components() {
        match comp {
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(AppError::InvalidInput(format!(
                    "dotfile rel_path 含非法路径成分: {rel_path}"
                )));
            }
            _ => {}
        }
    }

    let base = get_claude_config_dir();
    let p = base.join(rel_path);
    if !p.starts_with(&base) {
        return Err(AppError::InvalidInput(format!(
            "dotfile rel_path 解析后逃逸出配置目录: {rel_path}"
        )));
    }

    // Canonicalize re-check: if the nearest existing ancestor of the joined path
    // resolves to something outside the config dir (e.g. a pre-existing symlinked
    // subdir under ~/.claude pointing outside), reject the path.  We only attempt
    // this when the ancestor actually exists; if no ancestor exists yet the lexical
    // check above is already sufficient.
    // If canonicalization fails (e.g. base itself doesn't exist yet in some test
    // scenarios) fall through — the lexical check already passed.
    if let Some(existing_ancestor) = nearest_existing_ancestor(&p) {
        if let (Ok(canon_ancestor), Ok(canon_base)) =
            (existing_ancestor.canonicalize(), base.canonicalize())
        {
            if !canon_ancestor.starts_with(&canon_base) {
                return Err(AppError::InvalidInput(format!(
                    "dotfile rel_path 的现有祖先路径经符号链接解析后逃逸出配置目录: {rel_path}"
                )));
            }
        }
    }

    Ok(p)
}

/// SHA-256 hex digest of arbitrary bytes (mirrors skill.rs hashing).
pub fn content_hash(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// Render a whole-file dotfile to `~/.claude/<rel_path>`.
///
/// Safety contract:
/// - A malformed/traversing `rel_path` returns `Err` (nothing is written).
/// - If the target exists but we cannot prove ownership (no `prior_owned_hash`,
///   or the on-disk hash differs — i.e. the user edited it), the file is left
///   untouched and `Ok(None)` is returned (a skip).
/// - Otherwise (missing file, or on-disk hash matches `prior_owned_hash`) the
///   content is written and an unpersisted `ManifestEntry` is returned for the
///   caller to record via `db.record_manifest_entry`.
pub fn render_whole_file(
    db: &Database,
    profile_id: &str,
    app_type: &AppType,
    rel_path: &str,
    content: &str,
    prior_owned_hash: Option<&str>,
) -> Result<Option<ManifestEntry>, AppError> {
    let _ = db; // caller persists; db reserved for future ownership lookups
    let path = validate_rel_path(rel_path)?;

    if path.exists() {
        let disk = std::fs::read(&path).map_err(|e| AppError::io(&path, e))?;
        let disk_hash = content_hash(&disk);
        let owned = matches!(prior_owned_hash, Some(h) if h == disk_hash);
        if !owned {
            log::warn!("拒绝覆盖未托管/被用户编辑的文件: {}", path.display());
            return Ok(None);
        }
    }

    write_text_file(&path, content)?;
    let new_hash = content_hash(content.as_bytes());
    Ok(Some(ManifestEntry {
        id: 0,
        channel: "global".to_string(),
        profile_id: Some(profile_id.to_string()),
        project_id: None,
        app_type: app_type.as_str().to_string(),
        target_path: path.to_string_lossy().to_string(),
        kind: "whole_file".to_string(),
        content_hash: Some(new_hash),
        created_at: 0,
    }))
}

/// Remove a whole-file dotfile only if we own it (exists AND its on-disk hash
/// equals `recorded_hash`). Returns `true` if a file was deleted, `false` if it
/// was skipped (missing, no recorded hash, or user-edited/unknown).
pub fn remove_whole_file_if_owned(
    path: &str,
    recorded_hash: Option<&str>,
) -> Result<bool, AppError> {
    let p = Path::new(path);
    if !p.exists() {
        return Ok(false);
    }
    let disk = std::fs::read(p).map_err(|e| AppError::io(p, e))?;
    let disk_hash = content_hash(&disk);
    let owned = matches!(recorded_hash, Some(h) if h == disk_hash);
    if !owned {
        log::warn!("拒绝删除未托管/被用户编辑的文件: {}", p.display());
        return Ok(false);
    }
    delete_file(p)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::get_claude_config_dir;
    use crate::database::Database;
    use serial_test::serial;
    use std::env;
    use std::fs;
    use tempfile::TempDir;

    /// 测试用临时 HOME 守卫（镜像 profile.rs / backup.rs 的模式）。
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

    #[test]
    #[serial]
    fn whole_file_writes_and_records_manifest() -> Result<(), AppError> {
        let _home = TempHome::new();
        let db = Database::memory()?;

        let content = "echo hi";
        let entry = render_whole_file(
            &db,
            "prof:render-1",
            &AppType::Claude,
            "statusline.sh",
            content,
            None,
        )?
        .expect("expected a written entry");

        let path = get_claude_config_dir().join("statusline.sh");
        assert!(path.exists(), "file should be written");
        assert_eq!(fs::read_to_string(&path).unwrap(), content);

        assert_eq!(entry.kind, "whole_file");
        assert!(
            entry.target_path.ends_with("statusline.sh"),
            "target_path should end with statusline.sh, got {}",
            entry.target_path
        );
        assert_eq!(entry.content_hash, Some(content_hash(content.as_bytes())));
        Ok(())
    }

    #[test]
    #[serial]
    fn whole_file_refuses_to_overwrite_unowned_user_file() -> Result<(), AppError> {
        let _home = TempHome::new();
        let db = Database::memory()?;

        // Pre-create a user-owned file with NO manifest row.
        let path = get_claude_config_dir().join("statusline.sh");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, "USER").unwrap();

        let result = render_whole_file(
            &db,
            "prof:render-2",
            &AppType::Claude,
            "statusline.sh",
            "echo overwritten",
            None, // no prior ownership
        )?;

        assert!(
            result.is_none(),
            "should skip (return None) for un-owned file"
        );
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "USER",
            "user file must be untouched"
        );
        Ok(())
    }

    #[test]
    #[serial]
    fn whole_file_overwrites_own_managed_file() -> Result<(), AppError> {
        let _home = TempHome::new();
        let db = Database::memory()?;

        // First render creates the file; we capture the hash it recorded.
        let first = render_whole_file(
            &db,
            "prof:render-3",
            &AppType::Claude,
            "statusline.sh",
            "echo v1",
            None,
        )?
        .expect("first render writes");
        let prior_hash = first.content_hash.clone().expect("hash recorded");

        let path = get_claude_config_dir().join("statusline.sh");
        assert_eq!(fs::read_to_string(&path).unwrap(), "echo v1");

        // Second render: prior hash matches disk → we own it → overwrite.
        let second = render_whole_file(
            &db,
            "prof:render-3",
            &AppType::Claude,
            "statusline.sh",
            "echo v2",
            Some(prior_hash.as_str()),
        )?
        .expect("owned file should be overwritten");

        assert_eq!(fs::read_to_string(&path).unwrap(), "echo v2");
        assert_eq!(
            second.content_hash,
            Some(content_hash("echo v2".as_bytes()))
        );
        assert_ne!(second.content_hash, Some(prior_hash), "hash should change");
        Ok(())
    }

    #[test]
    #[serial]
    fn rel_path_traversal_rejected() -> Result<(), AppError> {
        let _home = TempHome::new();
        let db = Database::memory()?;

        // Relative traversal escaping the config dir.
        let r1 = render_whole_file(
            &db,
            "prof:render-4",
            &AppType::Claude,
            "../evil.sh",
            "PWN",
            None,
        );
        assert!(r1.is_err(), "traversal path must be rejected");

        // Absolute path.
        let r2 = render_whole_file(
            &db,
            "prof:render-4",
            &AppType::Claude,
            "/etc/evil",
            "PWN",
            None,
        );
        assert!(r2.is_err(), "absolute path must be rejected");

        // Nothing should have been written to the sibling-of-config location.
        let evil = get_claude_config_dir().parent().map(|p| p.join("evil.sh"));
        if let Some(evil) = evil {
            assert!(!evil.exists(), "no file should be created at {evil:?}");
        }
        assert!(
            !Path::new("/etc/evil").exists(),
            "must not create /etc/evil"
        );
        Ok(())
    }

    #[test]
    #[serial]
    fn remove_whole_file_only_when_owned() -> Result<(), AppError> {
        let _home = TempHome::new();

        let path = get_claude_config_dir().join("statusline.sh");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, "echo owned").unwrap();
        let owned_hash = content_hash(b"echo owned");
        let path_str = path.to_string_lossy().to_string();

        // Wrong hash (user edited) → skip, file stays.
        assert!(!remove_whole_file_if_owned(&path_str, Some("deadbeef"))?);
        assert!(path.exists(), "must not delete user-edited file");

        // No recorded hash → skip.
        assert!(!remove_whole_file_if_owned(&path_str, None)?);
        assert!(path.exists());

        // Correct hash → delete.
        assert!(remove_whole_file_if_owned(
            &path_str,
            Some(owned_hash.as_str())
        )?);
        assert!(!path.exists(), "owned file should be deleted");

        // Missing file → skip (false), no error.
        assert!(!remove_whole_file_if_owned(
            &path_str,
            Some(owned_hash.as_str())
        )?);
        Ok(())
    }

    /// Ensure validate_rel_path rejects a path whose first component is a
    /// symlink pointing outside the config dir.
    ///
    /// Layout:
    ///   <tmp_home>/.claude/           ← config base (created)
    ///   <tmp_home>/.claude/sub        ← symlink → <external_tmp>/
    ///   validate_rel_path("sub/x.sh") ← must return Err
    #[test]
    #[serial]
    #[cfg(unix)]
    fn validate_rel_path_rejects_symlinked_escape() {
        let _home = TempHome::new();
        let config_dir = get_claude_config_dir();
        fs::create_dir_all(&config_dir).expect("create config dir");

        // Create an external temp dir (outside ~/.claude) that the symlink will point to.
        let external = TempDir::new().expect("external temp dir");

        // Create a symlink ~/.claude/sub -> external dir.
        let symlink_path = config_dir.join("sub");
        std::os::unix::fs::symlink(external.path(), &symlink_path)
            .expect("create symlink sub -> external");

        // validate_rel_path must reject "sub/x.sh" because canonicalizing the
        // existing ancestor "sub" escapes the config dir.
        let result = validate_rel_path("sub/x.sh");
        assert!(
            result.is_err(),
            "validate_rel_path should reject path whose subdir is a symlink escaping ~/.claude, got {result:?}"
        );

        // Confirm no file was written outside.
        assert!(
            !external.path().join("x.sh").exists(),
            "no file should have been created in the external dir"
        );
    }
}
