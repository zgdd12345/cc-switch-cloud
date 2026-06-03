//! Project path-safety perimeter (increment 4a, CRITICAL #1).
//!
//! Before ANY write and on EVERY apply we canonicalize the project root and
//! REFUSE to operate if the canonical path equals/ancestors/descends $HOME, any
//! tool config dir (~/.claude, ~/.codex, ~/.config/opencode, ~/.gemini),
//! ~/.agenthub, or "/", OR if <root>/.claude canonicalizes to ~/.claude. This
//! makes the <project>/.claude == ~/.claude HOME-collapse impossible, which
//! would otherwise let a global deactivate owned-delete project files (and vice
//! versa) because get_manifest_for_profile filters by (profile_id, app_type)
//! only, not by channel.

use std::path::{Path, PathBuf};

use crate::app_config::AppType;
use crate::config::{get_app_config_dir, get_home_dir};
use crate::error::AppError;

/// A validated project base: the canonical project root + its app dotdir.
/// Construct ONLY via `resolve`, which runs the path-safety perimeter gate.
///
/// `allow(dead_code)`: this perimeter primitive is introduced here (increment 4a
/// CRITICAL #1) ahead of its bind/apply callers, which are wired in by later
/// tasks; the inline test module exercises the full surface today.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ProjectBase {
    root: PathBuf,   // canonical absolute project root
    dotdir: PathBuf, // <root>/<app.project_dotdir()>
}

#[allow(dead_code)] // surface consumed by bind/apply wiring in later tasks
impl ProjectBase {
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn dotdir(&self) -> &Path {
        &self.dotdir
    }
    /// Canonical channel key for the manifest: "project:<canonical-abspath>".
    pub fn channel(&self) -> String {
        format!("project:{}", self.root.to_string_lossy())
    }

    /// Resolve + validate a user-entered project root for `app`.
    ///
    /// Returns Err (refusing all writes) when the canonical root equals,
    /// is an ancestor of, or is a descendant of any forbidden anchor, or when
    /// <root>/.claude collapses onto ~/.claude.
    pub fn resolve(entered: &str, app: &AppType) -> Result<ProjectBase, AppError> {
        let trimmed = entered.trim();
        if trimmed.is_empty() {
            return Err(AppError::InvalidInput("project root 为空".into()));
        }
        let raw = PathBuf::from(trimmed);
        if !raw.is_absolute() {
            return Err(AppError::InvalidInput(format!(
                "project root 必须为绝对路径: {trimmed}"
            )));
        }
        // Canonicalize the root. The root must exist (a real dir to bind).
        let root = raw.canonicalize().map_err(|e| {
            AppError::InvalidInput(format!(
                "无法解析 project root（需为已存在目录）: {trimmed}: {e}"
            ))
        })?;
        if !root.is_dir() {
            return Err(AppError::InvalidInput(format!(
                "project root 不是目录: {}",
                root.display()
            )));
        }

        let home = get_home_dir();
        let home_canon = home.canonicalize().unwrap_or(home.clone());

        // "Boundary" anchors (HOME and "/"): a normal project legitimately lives
        // *inside* HOME, so these are only forbidden when root EQUALS them or is an
        // ANCESTOR of them — NOT when root is a descendant (that is the common case).
        let boundary_anchors: Vec<PathBuf> = vec![home_canon.clone(), PathBuf::from("/")];

        // "Protected" anchors (tool/app dirs): forbidden if root equals, ancestors,
        // OR descends them (e.g. ~/.claude/skills must also be refused).
        let mut protected_anchors: Vec<PathBuf> = Vec::new();
        for sub in [".claude", ".codex", ".gemini", ".agenthub"] {
            protected_anchors.push(canon_or_join(&home_canon, sub));
        }
        // ~/.config/opencode
        protected_anchors.push(canon_or_join(&home_canon.join(".config"), "opencode"));
        // app-config dir (override-aware) — usually ~/.agenthub
        let app_cfg = get_app_config_dir();
        protected_anchors.push(app_cfg.canonicalize().unwrap_or(app_cfg));

        for anchor in &boundary_anchors {
            if &root == anchor {
                return Err(refuse(&root, "等于受保护目录(HOME/工具目录/应用配置/根)"));
            }
            if anchor.starts_with(&root) {
                // root is an ancestor of a protected boundary
                return Err(refuse(
                    &root,
                    "是受保护目录的祖先(HOME/工具目录/应用配置/根)",
                ));
            }
        }

        for anchor in &protected_anchors {
            if &root == anchor {
                return Err(refuse(&root, "等于受保护目录(HOME/工具目录/应用配置/根)"));
            }
            if anchor.starts_with(&root) {
                // root is an ancestor of a protected anchor
                return Err(refuse(
                    &root,
                    "是受保护目录的祖先(HOME/工具目录/应用配置/根)",
                ));
            }
            if root.starts_with(anchor) {
                // root is a descendant of a protected anchor
                return Err(refuse(&root, "位于受保护目录内(HOME/工具目录/应用配置/根)"));
            }
        }

        // <root>/.claude collapse check: if the app dotdir canonicalizes to ~/.claude, refuse.
        let dotdir = root.join(app.project_dotdir());
        let claude_canon = canon_or_join(&home_canon, ".claude");
        if let Ok(dotdir_canon) = dotdir.canonicalize() {
            if dotdir_canon == claude_canon {
                return Err(refuse(
                    &root,
                    "<root>/.claude 解析后等于 ~/.claude（HOME 坍缩）",
                ));
            }
        }

        Ok(ProjectBase { root, dotdir })
    }
}

#[allow(dead_code)] // used by ProjectBase::resolve + later bind/apply callers
fn refuse(root: &Path, why: &str) -> AppError {
    AppError::InvalidInput(format!(
        "拒绝绑定/应用：project root {} {why} [HOME-safety]",
        root.display()
    ))
}

/// Canonicalize `base/sub` if it exists, else return the lexical join (still a
/// valid anchor for prefix checks).
#[allow(dead_code)] // used by ProjectBase::resolve + later bind/apply callers
fn canon_or_join(base: &Path, sub: &str) -> PathBuf {
    let p = base.join(sub);
    p.canonicalize().unwrap_or(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::env;
    use tempfile::TempDir;

    struct TempHome {
        dir: TempDir,
        original_home: Option<String>,
        original_userprofile: Option<String>,
        original_test_home: Option<String>,
    }
    impl TempHome {
        fn new() -> Self {
            let dir = TempDir::new().expect("tempdir");
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
        fn home(&self) -> &Path {
            self.dir.path()
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
    fn accepts_normal_project_dir() {
        let home = TempHome::new();
        let proj = home.home().join("work").join("repo");
        std::fs::create_dir_all(&proj).expect("mkdir proj");
        let base = ProjectBase::resolve(proj.to_str().unwrap(), &AppType::Claude)
            .expect("a normal project dir must be accepted");
        assert_eq!(base.dotdir(), base.root().join(".claude"));
    }

    #[test]
    #[serial]
    fn refuses_home_itself() {
        let home = TempHome::new();
        let err = ProjectBase::resolve(home.home().to_str().unwrap(), &AppType::Claude)
            .expect_err("binding $HOME must be refused (collapse)");
        assert!(
            err.to_string().contains("HOME") || err.to_string().contains("拒绝"),
            "err={err}"
        );
    }

    #[test]
    #[serial]
    fn refuses_claude_config_dir() {
        let home = TempHome::new();
        let claude = home.home().join(".claude");
        std::fs::create_dir_all(&claude).expect("mkdir .claude");
        assert!(ProjectBase::resolve(claude.to_str().unwrap(), &AppType::Claude).is_err());
    }

    #[test]
    #[serial]
    fn refuses_tool_dirs() {
        let home = TempHome::new();
        for d in [".codex", ".gemini"] {
            let p = home.home().join(d);
            std::fs::create_dir_all(&p).expect("mkdir tool dir");
            assert!(
                ProjectBase::resolve(p.to_str().unwrap(), &AppType::Claude).is_err(),
                "{d} must be refused"
            );
        }
        let oc = home.home().join(".config").join("opencode");
        std::fs::create_dir_all(&oc).expect("mkdir opencode");
        assert!(
            ProjectBase::resolve(oc.to_str().unwrap(), &AppType::Claude).is_err(),
            ".config/opencode must be refused"
        );
    }

    #[test]
    #[serial]
    fn refuses_app_config_dir() {
        let home = TempHome::new();
        let ah = home.home().join(".agenthub");
        std::fs::create_dir_all(&ah).expect("mkdir .agenthub");
        assert!(ProjectBase::resolve(ah.to_str().unwrap(), &AppType::Claude).is_err());
    }

    #[test]
    #[serial]
    fn refuses_root() {
        let _home = TempHome::new();
        assert!(ProjectBase::resolve("/", &AppType::Claude).is_err());
    }

    #[test]
    #[serial]
    fn refuses_ancestor_of_home() {
        let home = TempHome::new();
        // parent of $HOME is an ancestor → refuse
        let parent = home.home().parent().expect("home has parent").to_path_buf();
        assert!(ProjectBase::resolve(parent.to_str().unwrap(), &AppType::Claude).is_err());
    }

    #[test]
    #[serial]
    fn refuses_descendant_of_claude_dir() {
        let home = TempHome::new();
        let inside = home.home().join(".claude").join("skills");
        std::fs::create_dir_all(&inside).expect("mkdir inside .claude");
        assert!(ProjectBase::resolve(inside.to_str().unwrap(), &AppType::Claude).is_err());
    }

    #[test]
    #[serial]
    fn refuses_when_dotdir_symlinks_to_claude() {
        let home = TempHome::new();
        let proj = home.home().join("work").join("trick");
        std::fs::create_dir_all(&proj).expect("mkdir proj");
        let real_claude = home.home().join(".claude");
        std::fs::create_dir_all(&real_claude).expect("mkdir .claude");
        // <proj>/.claude -> ~/.claude : the dotdir collapse the gate must catch
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real_claude, proj.join(".claude")).expect("symlink");
        #[cfg(unix)]
        assert!(ProjectBase::resolve(proj.to_str().unwrap(), &AppType::Claude).is_err());
    }
}
