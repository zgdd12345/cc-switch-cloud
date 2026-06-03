//! `${VAR}` substitution engine for profile-templated dotfiles.
//!
//! Increment 3b-2 renders `${VAR}` references inside whole-file dotfiles
//! (`settings.json`, `statusline.sh`). A profile carries raw templates; at
//! activation time we resolve `${NAME}` (with optional `${NAME:-default}`
//! shell-style fallback) against a layered variable map.
//!
//! ## Variable precedence (low -> high; higher overwrites)
//! 1. Process environment, filtered to the [`ENV_ALLOWLIST_PREFIXES`].
//! 2. The active provider's `settings_config.env` object.
//! 3. The profile's own `spec.vars`.
//!
//! ## Substitution semantics
//! - A name is `[A-Za-z_][A-Za-z0-9_]*`.
//! - `${NAME:-default}` uses `default` when `NAME` is unset OR empty (shell
//!   `:-` semantics). The default literal runs up to the matching `}` and does
//!   NOT support nested `${...}` (a `${` inside it is literal text).
//! - Resolved values are NOT re-scanned (no recursive expansion).
//! - When `json_escape` is true the spliced text (value or default) is escaped
//!   as a JSON string fragment so it can sit safely inside a JSON literal.
//! - An unknown `${NAME}` (no value, no default) is left verbatim and a
//!   warning is recorded.
//! - A lone `$` (not followed by `{`) or an unterminated `${` is emitted
//!   verbatim.
//!
//! NOTE: `build_var_map` / `substitute_vars` / `render_with_profile_vars` are
//! the public engine API. They are consumed by the profile-apply rendering
//! path (3b-2 T2/T3); they may be transiently unconsumed at T1.

use indexmap::IndexMap;
use serde_json::Value;

use crate::app_config::{AppType, Profile};
use crate::database::Database;
use crate::error::AppError;

/// Process-environment variables are only admitted into the variable map when
/// their key starts with one of these prefixes. This keeps unrelated/sensitive
/// host env out of rendered profile dotfiles.
pub const ENV_ALLOWLIST_PREFIXES: &[&str] = &["ANTHROPIC_", "AGENTHUB_", "CLAUDE_"];

/// An ordered name -> value variable map.
///
/// Insertion order is preserved (low-to-high precedence layers are inserted in
/// order, with later inserts overwriting earlier ones in place).
#[derive(Debug, Clone, Default)]
pub struct VarMap(IndexMap<String, String>);

impl VarMap {
    /// Look up a variable by name, returning its value if present.
    pub fn get(&self, k: &str) -> Option<&str> {
        self.0.get(k).map(|s| s.as_str())
    }

    /// Build a `VarMap` directly from an ordered index map (the inner field is
    /// private; `build_project_var_map` (T7) constructs the layers itself and
    /// hands the finished map in here).
    #[allow(dead_code)]
    pub(crate) fn from_index_map(map: IndexMap<String, String>) -> VarMap {
        VarMap(map)
    }
}

/// Coerce a JSON value into a flat string for use as a variable value.
///
/// - `String` is used as-is.
/// - `Number` / `Bool` use their `to_string()`.
/// - Anything else (null, array, object) is skipped (`None`).
pub(crate) fn coerce_value(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Build the layered variable map for a profile under a given app.
///
/// Layers are inserted low-to-high so higher layers overwrite lower ones:
/// 1. allowlisted process env, 2. active provider `env`, 3. profile `spec.vars`.
pub fn build_var_map(
    db: &Database,
    app_type: &AppType,
    profile: &Profile,
) -> Result<VarMap, AppError> {
    let mut map: IndexMap<String, String> = IndexMap::new();

    // Layer 1: allowlisted process env (lowest precedence).
    //
    // CAVEAT: process-env values are read fresh on every call to `build_var_map`,
    // which is invoked at both forward-render (activation) and backfill-strip
    // (deactivation / re-render) time.  A settings.json fragment that splices an
    // allowlisted process-env var (rather than `profile.spec.vars` or
    // `provider.settings_config.env`, which are DB-stable) can therefore defeat
    // the backfill re-render strip if the host environment changes between the
    // earlier session's write and a later activation switch — the rendered value
    // will differ from the stored one, so the strip pass may fail to match.
    // Prefer `spec.vars` / `provider.env` for values that are spliced into
    // settings.json fragments.
    for (k, v) in std::env::vars() {
        if ENV_ALLOWLIST_PREFIXES
            .iter()
            .any(|prefix| k.starts_with(prefix))
        {
            map.insert(k, v);
        }
    }

    // Layer 2: active provider env (if any).
    if let Some(provider_id) = crate::settings::get_effective_current_provider(db, app_type)? {
        if let Some(provider) = db.get_provider_by_id(&provider_id, app_type.as_str())? {
            if let Some(env_obj) = provider
                .settings_config
                .get("env")
                .and_then(|v| v.as_object())
            {
                for (k, v) in env_obj {
                    if let Some(value) = coerce_value(v) {
                        map.insert(k.clone(), value);
                    }
                }
            }
        }
    }

    // Layer 3: profile spec.vars (highest precedence).
    for (k, v) in &profile.spec.vars {
        if let Some(value) = coerce_value(v) {
            map.insert(k.clone(), value);
        }
    }

    Ok(VarMap(map))
}

/// Escape `s` as the inner body of a JSON string (no surrounding quotes).
fn json_escape_fragment(s: &str) -> String {
    let quoted =
        serde_json::to_string(&Value::String(s.to_string())).unwrap_or_else(|_| format!("\"{s}\""));
    // Trim the surrounding quotes that `to_string` always adds.
    quoted
        .strip_prefix('"')
        .and_then(|q| q.strip_suffix('"'))
        .map(|inner| inner.to_string())
        .unwrap_or(quoted)
}

/// Substitute `${VAR}` references in `template` against `vars`.
///
/// See the module docs for the full semantics. Returns the rendered string;
/// any unresolved names are recorded in `warnings`.
pub fn substitute_vars(
    template: &str,
    vars: &VarMap,
    json_escape: bool,
    warnings: &mut Vec<String>,
) -> String {
    let bytes = template.as_bytes();
    let mut out = String::with_capacity(template.len());
    let mut i = 0;

    while i < bytes.len() {
        // Look for the start of a placeholder: "${".
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            // Parse NAME = [A-Za-z_][A-Za-z0-9_]* starting after "${".
            let name_start = i + 2;
            let mut j = name_start;

            // First char must be [A-Za-z_].
            let first_ok = j < bytes.len() && (bytes[j].is_ascii_alphabetic() || bytes[j] == b'_');
            if first_ok {
                j += 1;
                while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                    j += 1;
                }
                let name = &template[name_start..j];

                // After the name we expect either "}" or ":-<default>}".
                if j < bytes.len() && bytes[j] == b'}' {
                    // No default.
                    splice_resolved(name, None, vars, json_escape, &mut out, warnings);
                    i = j + 1;
                    continue;
                } else if j + 1 < bytes.len() && bytes[j] == b':' && bytes[j + 1] == b'-' {
                    // ":-default" — read default literal up to the matching "}".
                    // The default does NOT support nested "${}"; a "${" inside
                    // it is literal, so we scan for the next plain "}".
                    let default_start = j + 2;
                    if let Some(close_rel) = template[default_start..].find('}') {
                        let default = &template[default_start..default_start + close_rel];
                        splice_resolved(name, Some(default), vars, json_escape, &mut out, warnings);
                        i = default_start + close_rel + 1;
                        continue;
                    }
                    // Unterminated "${NAME:-..." with no "}" — emit verbatim.
                    out.push_str(&template[i..]);
                    break;
                }
                // Not a valid terminator (e.g. "${NAME" unterminated, or a
                // stray char). Emit the "${" verbatim and resume scanning at
                // the character after it so any later valid placeholder is
                // still picked up.
                out.push_str("${");
                i += 2;
                continue;
            }

            // "${" not followed by a valid name char — emit verbatim and
            // continue after the "${".
            out.push_str("${");
            i += 2;
            continue;
        }

        // Ordinary byte (including a lone "$" not followed by "{"). Copy the
        // whole UTF-8 char to keep the output valid.
        let ch_len = utf8_char_len(bytes[i]);
        out.push_str(&template[i..i + ch_len]);
        i += ch_len;
    }

    out
}

/// Resolve a placeholder and splice the result (or the verbatim `${...}`) into
/// `out`, recording a warning for an unknown name.
fn splice_resolved(
    name: &str,
    default: Option<&str>,
    vars: &VarMap,
    json_escape: bool,
    out: &mut String,
    warnings: &mut Vec<String>,
) {
    // Shell `:-` semantics: a value is "set" only when present AND non-empty.
    let resolved: Option<String> = match vars.get(name) {
        Some(v) if !v.is_empty() => Some(v.to_string()),
        _ => default.map(|d| d.to_string()),
    };

    match resolved {
        Some(value) => {
            if json_escape {
                out.push_str(&json_escape_fragment(&value));
            } else {
                out.push_str(&value);
            }
        }
        None => {
            // Unknown var, no default — leave the literal placeholder and warn.
            out.push_str("${");
            out.push_str(name);
            out.push('}');
            warnings.push(format!("unknown var: {name}"));
        }
    }
}

/// Length in bytes of the UTF-8 character whose first byte is `b`.
fn utf8_char_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b >> 5 == 0b110 {
        2
    } else if b >> 4 == 0b1110 {
        3
    } else {
        4
    }
}

/// Build the variable map for `profile` and render `template`.
///
/// Convenience wrapper combining [`build_var_map`] + [`substitute_vars`].
pub fn render_with_profile_vars(
    db: &Database,
    app_type: &AppType,
    profile: &Profile,
    template: &str,
    json_escape: bool,
    warnings: &mut Vec<String>,
) -> Result<String, AppError> {
    let vars = build_var_map(db, app_type, profile)?;
    Ok(substitute_vars(template, &vars, json_escape, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Provider;
    use serde_json::json;
    use tempfile::TempDir;

    /// Test-only HOME guard: isolates `HOME`/`USERPROFILE`/`CC_SWITCH_TEST_HOME`
    /// so the local "current provider" settings are empty and
    /// `get_effective_current_provider` falls back to the DB `is_current` flag.
    /// Mirrors the `TempHome` pattern in `services::profile` / `database::backup`.
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
            let original_home = std::env::var("HOME").ok();
            let original_userprofile = std::env::var("USERPROFILE").ok();
            let original_test_home = std::env::var("CC_SWITCH_TEST_HOME").ok();
            std::env::set_var("HOME", dir.path());
            std::env::set_var("USERPROFILE", dir.path());
            std::env::set_var("CC_SWITCH_TEST_HOME", dir.path());
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
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match &self.original_userprofile {
                Some(v) => std::env::set_var("USERPROFILE", v),
                None => std::env::remove_var("USERPROFILE"),
            }
            match &self.original_test_home {
                Some(v) => std::env::set_var("CC_SWITCH_TEST_HOME", v),
                None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
            }
        }
    }

    /// Construct a `VarMap` from `(name, value)` pairs for unit tests.
    fn varmap(pairs: &[(&str, &str)]) -> VarMap {
        let mut m: IndexMap<String, String> = IndexMap::new();
        for (k, v) in pairs {
            m.insert((*k).to_string(), (*v).to_string());
        }
        VarMap(m)
    }

    #[test]
    fn substitutes_basic_var() {
        let vars = varmap(&[("A", "hello")]);
        let mut w = Vec::new();
        let out = substitute_vars("x=${A}!", &vars, false, &mut w);
        assert_eq!(out, "x=hello!");
        assert!(w.is_empty());
    }

    #[test]
    fn default_used_when_unset() {
        let vars = varmap(&[]);
        let mut w = Vec::new();
        let out = substitute_vars("${X:-def}", &vars, false, &mut w);
        assert_eq!(out, "def");
        assert!(w.is_empty());
    }

    #[test]
    fn empty_value_uses_default() {
        let vars = varmap(&[("X", "")]);
        let mut w = Vec::new();
        let out = substitute_vars("${X:-def}", &vars, false, &mut w);
        assert_eq!(out, "def");
        assert!(w.is_empty());
    }

    #[test]
    fn non_empty_value_beats_default() {
        let vars = varmap(&[("X", "set")]);
        let mut w = Vec::new();
        let out = substitute_vars("${X:-def}", &vars, false, &mut w);
        assert_eq!(out, "set");
    }

    #[test]
    fn unknown_kept_literal_and_warns() {
        let vars = varmap(&[]);
        let mut w = Vec::new();
        let out = substitute_vars("a${NOPE}b", &vars, false, &mut w);
        assert_eq!(out, "a${NOPE}b");
        assert_eq!(w, vec!["unknown var: NOPE".to_string()]);
    }

    #[test]
    fn json_escapes_quote_and_backslash() {
        let vars = varmap(&[("A", "he said \"hi\"\\path")]);
        let mut w = Vec::new();
        let out = substitute_vars("\"key\": \"${A}\"", &vars, true, &mut w);
        assert_eq!(out, "\"key\": \"he said \\\"hi\\\"\\\\path\"");
        assert!(w.is_empty());
    }

    #[test]
    fn json_escape_off_splices_raw() {
        let vars = varmap(&[("A", "a\"b")]);
        let mut w = Vec::new();
        let out = substitute_vars("${A}", &vars, false, &mut w);
        assert_eq!(out, "a\"b");
    }

    #[test]
    fn default_branch_is_json_escaped_too() {
        let vars = varmap(&[]);
        let mut w = Vec::new();
        let out = substitute_vars("${X:-a\"b}", &vars, true, &mut w);
        assert_eq!(out, "a\\\"b");
        assert!(w.is_empty());
    }

    #[test]
    fn resolved_value_is_not_rescanned() {
        // Value contains ${Y}; it must NOT be re-substituted even though Y is set.
        let vars = varmap(&[("A", "pre ${Y} post"), ("Y", "SHOULD_NOT_APPEAR")]);
        let mut w = Vec::new();
        let out = substitute_vars("${A}", &vars, false, &mut w);
        assert_eq!(out, "pre ${Y} post");
        assert!(w.is_empty());
    }

    #[test]
    fn lone_dollar_emitted_verbatim() {
        let vars = varmap(&[]);
        let mut w = Vec::new();
        let out = substitute_vars("cost is $5 and $ here", &vars, false, &mut w);
        assert_eq!(out, "cost is $5 and $ here");
        assert!(w.is_empty());
    }

    #[test]
    fn unterminated_placeholder_emitted_verbatim() {
        let vars = varmap(&[("A", "x")]);
        let mut w = Vec::new();
        let out = substitute_vars("${A} then ${UNCLOSED", &vars, false, &mut w);
        assert_eq!(out, "x then ${UNCLOSED");
        // No warning: it was never a complete placeholder.
        assert!(w.is_empty());
    }

    #[test]
    fn unterminated_default_emitted_verbatim() {
        let vars = varmap(&[]);
        let mut w = Vec::new();
        let out = substitute_vars("${X:-no close", &vars, false, &mut w);
        assert_eq!(out, "${X:-no close");
        assert!(w.is_empty());
    }

    #[test]
    fn dollar_brace_invalid_name_verbatim() {
        let vars = varmap(&[]);
        let mut w = Vec::new();
        // "${1abc}" — name cannot start with a digit.
        let out = substitute_vars("${1abc}", &vars, false, &mut w);
        assert_eq!(out, "${1abc}");
        assert!(w.is_empty());
    }

    #[test]
    fn nested_default_brace_is_literal() {
        // A "${" inside the default is literal; the default ends at the first "}".
        let vars = varmap(&[]);
        let mut w = Vec::new();
        let out = substitute_vars("${X:-a${Y}b", &vars, false, &mut w);
        // default literal = "a${Y" (up to first "}"), then "b" trails.
        assert_eq!(out, "a${Yb");
        assert!(w.is_empty());
    }

    #[test]
    fn utf8_preserved_around_placeholders() {
        let vars = varmap(&[("A", "值")]);
        let mut w = Vec::new();
        let out = substitute_vars("前${A}后", &vars, false, &mut w);
        assert_eq!(out, "前值后");
    }

    // ----- build_var_map precedence / allowlist (FS-touching: #[serial]) -----

    fn profile_with_vars(vars: serde_json::Map<String, Value>) -> Profile {
        Profile {
            id: "p1".to_string(),
            app_type: "claude".to_string(),
            name: "P1".to_string(),
            description: None,
            is_active: false,
            current_provider_id: None,
            spec: crate::app_config::ProfileSpec {
                content: Default::default(),
                vars,
            },
            sort_index: 0,
            created_at: 0,
        }
    }

    #[test]
    #[serial_test::serial]
    fn build_var_map_precedence_profile_over_provider_over_process() {
        // Isolate HOME/settings so the local "current provider" is empty and
        // get_effective_current_provider falls back to the DB is_current flag.
        let _home = TempHome::new();

        // Set an allowlisted process env var (layer 1) that all higher layers
        // will override; also one only the process provides.
        std::env::set_var("ANTHROPIC_SHARED", "from_process");
        std::env::set_var("AGENTHUB_ONLY_PROCESS", "process_only");
        // A non-allowlisted var must be filtered out.
        std::env::set_var("RANDOM_HOST_SECRET", "leak");

        let db = Database::memory().expect("memory db");
        let app = AppType::Claude;

        // Active provider with env (layer 2) — overrides process for SHARED,
        // and contributes PROVIDER_KEY (string), a number, and a bool.
        let provider = Provider::with_id(
            "prov1".to_string(),
            "Prov 1".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_SHARED": "from_provider",
                    "ANTHROPIC_PROVIDER_KEY": "pk",
                    "ANTHROPIC_NUM": 42,
                    "ANTHROPIC_BOOL": true,
                    "ANTHROPIC_OBJ": { "nested": 1 }
                }
            }),
            None,
        );
        db.save_provider(app.as_str(), &provider)
            .expect("save provider");
        db.set_current_provider(app.as_str(), "prov1")
            .expect("set current");

        // Profile vars (layer 3) — overrides provider for SHARED.
        let mut pvars = serde_json::Map::new();
        pvars.insert(
            "ANTHROPIC_SHARED".to_string(),
            Value::String("from_profile".to_string()),
        );
        let profile = profile_with_vars(pvars);

        let map = build_var_map(&db, &app, &profile).expect("build var map");

        // Precedence: profile wins.
        assert_eq!(map.get("ANTHROPIC_SHARED"), Some("from_profile"));
        // Process-only var survives (not overridden).
        assert_eq!(map.get("AGENTHUB_ONLY_PROCESS"), Some("process_only"));
        // Provider contributions.
        assert_eq!(map.get("ANTHROPIC_PROVIDER_KEY"), Some("pk"));
        assert_eq!(map.get("ANTHROPIC_NUM"), Some("42"));
        assert_eq!(map.get("ANTHROPIC_BOOL"), Some("true"));
        // Non-coercible object skipped.
        assert_eq!(map.get("ANTHROPIC_OBJ"), None);
        // Non-allowlisted process var filtered out.
        assert_eq!(map.get("RANDOM_HOST_SECRET"), None);

        std::env::remove_var("ANTHROPIC_SHARED");
        std::env::remove_var("AGENTHUB_ONLY_PROCESS");
        std::env::remove_var("RANDOM_HOST_SECRET");
    }

    #[test]
    #[serial_test::serial]
    fn build_var_map_skips_provider_layer_when_none() {
        let _home = TempHome::new();
        std::env::set_var("CLAUDE_PROC", "p");

        let db = Database::memory().expect("memory db");
        let app = AppType::Claude;
        // No provider set -> layer 2 skipped, no error.
        let profile = profile_with_vars(serde_json::Map::new());
        let map = build_var_map(&db, &app, &profile).expect("build var map");
        assert_eq!(map.get("CLAUDE_PROC"), Some("p"));

        std::env::remove_var("CLAUDE_PROC");
    }

    #[test]
    #[serial_test::serial]
    fn render_with_profile_vars_end_to_end() {
        let _home = TempHome::new();
        std::env::set_var("ANTHROPIC_MODEL", "claude-x");

        let db = Database::memory().expect("memory db");
        let app = AppType::Claude;
        let profile = profile_with_vars(serde_json::Map::new());

        let mut w = Vec::new();
        let out = render_with_profile_vars(
            &db,
            &app,
            &profile,
            "model=${ANTHROPIC_MODEL} fallback=${MISSING:-none}",
            false,
            &mut w,
        )
        .expect("render");
        assert_eq!(out, "model=claude-x fallback=none");
        assert!(w.is_empty());

        std::env::remove_var("ANTHROPIC_MODEL");
    }

    #[test]
    fn varmap_from_index_map_preserves_entries() {
        let mut m: IndexMap<String, String> = IndexMap::new();
        m.insert("A".to_string(), "1".to_string());
        m.insert("B".to_string(), "two".to_string());
        let vm = VarMap::from_index_map(m);
        assert_eq!(vm.get("A"), Some("1"));
        assert_eq!(vm.get("B"), Some("two"));
        assert_eq!(vm.get("MISSING"), None);
    }

    #[test]
    fn coerce_value_is_callable_from_module() {
        // coerce_value is pub(crate) so build_project_var_map (T7) can reuse it.
        assert_eq!(coerce_value(&serde_json::json!("s")), Some("s".to_string()));
        assert_eq!(coerce_value(&serde_json::json!(7)), Some("7".to_string()));
        assert_eq!(
            coerce_value(&serde_json::json!(true)),
            Some("true".to_string())
        );
        assert_eq!(coerce_value(&serde_json::json!({"k": 1})), None);
    }
}
