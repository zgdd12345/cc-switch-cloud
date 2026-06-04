//! Project settings.json MERGE engine (increment 4b-2, Claude only).
//!
//! `merge_with_snapshot` is a PURE forward deep-merge that records a FLAT list of
//! independent `OwnedKey` leaves describing EXACTLY what we wrote and the prior
//! value at each leaf. It intentionally MIRRORS the Object×Object-recurse /
//! else-overwrite shape of `services::provider::live::json_deep_merge` but is
//! SELF-CONTAINED (it does NOT call json_deep_merge — no cross-module coupling).
//! `reverse_merge` (T6) is the pure teardown: a function of (owned_keys + current
//! disk) only — NO re-render, NO env read. There is deliberately NO
//! `collapse_empty_created_ancestors` step: emptied user objects are left as `{}`.

use crate::error::AppError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

#[allow(dead_code)] // consumed by reverse_merge in T6
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PriorLeaf {
    pub present: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
}

#[allow(dead_code)] // consumed by reverse_merge in T6
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OwnedKey {
    pub path: Vec<String>,
    pub prior: PriorLeaf,
    pub wrote: Value,
}

#[allow(dead_code)] // consumed by reverse_merge in T6
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OwnedKeysEnvelope {
    pub v: u32,
    pub keys: Vec<OwnedKey>,
}

#[allow(dead_code)] // consumed by reverse_merge in T6
pub const OWNED_KEYS_VERSION: u32 = 1;

/// PURE forward deep-merge of `frag` into `user`, recording a flat list of the
/// leaves we wrote (with their prior values) into `out`.
///
/// Mirrors json_deep_merge's recursion shape (Object×Object recurse, else
/// overwrite) but records a snapshot leaf at every overwrite/insert so the
/// teardown can be a pure function of the snapshot. Absent frag key → ONE
/// whole-subtree leaf (`prior.present=false`). Present-as-object → recurse
/// (deeper leaves). Present-as-non-object (or frag-leaf vs user-object) → one
/// leaf `prior.present=true`. Arrays are leaves (WHOLE replace, no element union).
#[allow(dead_code)] // consumed by reverse_merge in T6
pub fn merge_with_snapshot(
    user: &mut Value,
    frag: &Value,
    path: &mut Vec<String>,
    out: &mut Vec<OwnedKey>,
) {
    match (user, frag) {
        (Value::Object(user_map), Value::Object(frag_map)) => {
            for (key, frag_value) in frag_map {
                path.push(key.clone());
                match user_map.get_mut(key) {
                    Some(uv) if uv.is_object() && frag_value.is_object() => {
                        merge_with_snapshot(uv, frag_value, path, out)
                    }
                    Some(uv) => {
                        out.push(OwnedKey {
                            path: path.clone(),
                            prior: PriorLeaf {
                                present: true,
                                value: Some(uv.clone()),
                            },
                            wrote: frag_value.clone(),
                        });
                        *uv = frag_value.clone();
                    }
                    None => {
                        out.push(OwnedKey {
                            path: path.clone(),
                            prior: PriorLeaf {
                                present: false,
                                value: None,
                            },
                            wrote: frag_value.clone(),
                        });
                        user_map.insert(key.clone(), frag_value.clone());
                    }
                }
                path.pop();
            }
        }
        (user_slot, frag_value) => {
            out.push(OwnedKey {
                path: path.clone(),
                prior: PriorLeaf {
                    present: true,
                    value: Some(user_slot.clone()),
                },
                wrote: frag_value.clone(),
            });
            *user_slot = frag_value.clone();
        }
    }
}

/// PURE teardown: a function of (owned_keys + current disk) ONLY — NO re-render,
/// NO env read. Per-leaf `cur==wrote` gate. There is deliberately NO
/// collapse_empty_created_ancestors (adversarial fix #1): emptied user objects
/// are left as `{}`. FAIL-CLOSED on unknown envelope version / unparseable
/// owned_keys / invalid on-disk JSON (warn, leave the file, return Ok).
#[allow(dead_code)] // wired into apply pre-delete + detach in a later task
pub fn reverse_merge(
    file_path: &Path,
    owned_json: Option<&str>,
    warnings: &mut Vec<String>,
) -> Result<(), AppError> {
    let Some(text) = owned_json else {
        return Ok(());
    };
    if text.trim().is_empty() {
        return Ok(());
    }
    let mut env: OwnedKeysEnvelope = match serde_json::from_str(text) {
        Ok(e) => e,
        Err(e) => {
            warnings.push(format!(
                "settings.json owned_keys unparseable, leaving untouched ({file_path:?}): {e}"
            ));
            return Ok(());
        }
    };
    if env.v != OWNED_KEYS_VERSION {
        warnings.push(format!(
            "settings.json owned_keys version {} unsupported (expected {OWNED_KEYS_VERSION}); leaving untouched ({file_path:?})",
            env.v
        ));
        return Ok(()); // FAIL-CLOSED
    }
    if !file_path.exists() {
        return Ok(());
    }
    let disk = std::fs::read(file_path).map_err(|e| AppError::io(file_path, e))?;
    let mut root: Value = match serde_json::from_slice(&disk) {
        Ok(v) => v,
        Err(e) => {
            warnings.push(format!(
                "settings.json on disk not valid JSON, leaving untouched ({file_path:?}): {e}"
            ));
            return Ok(());
        }
    };
    // Independent leaves; order is irrelevant for correctness now that collapse is
    // removed, but process deepest-first for determinism.
    env.keys.sort_by_key(|k| std::cmp::Reverse(k.path.len()));
    for k in &env.keys {
        if k.path.is_empty() {
            apply_root_leaf(&mut root, k);
            continue;
        }
        let (parents, leaf) = k.path.split_at(k.path.len() - 1);
        let leaf = &leaf[0];
        let Some(parent) = navigate_mut(&mut root, parents) else {
            continue;
        };
        let Some(pm) = parent.as_object_mut() else {
            continue;
        };
        match (k.prior.present, pm.get(leaf)) {
            // we inserted it and it is still ours → remove (leaves emptied user
            // objects as {} — NO collapse).
            (false, Some(cur)) if *cur == k.wrote => {
                pm.remove(leaf);
            }
            // user edited/removed our inserted leaf → leave it.
            (false, _) => {}
            // we overwrote it and it is still ours → restore the original value.
            (true, Some(cur)) if *cur == k.wrote => {
                pm.insert(leaf.clone(), k.prior.value.clone().unwrap_or(Value::Null));
            }
            // user re-edited → leave it.
            (true, _) => {}
        }
    }
    // NO collapse_empty_created_ancestors (removed per adversarial fix #1).
    let bytes = serde_json::to_vec_pretty(&crate::config::sort_json_keys(&root))
        .map_err(|e| AppError::Message(format!("serialize settings.json: {e}")))?;
    crate::config::atomic_write(file_path, &bytes)?;
    Ok(())
}

#[allow(dead_code)] // helper for reverse_merge (wired in a later task)
fn navigate_mut<'a>(root: &'a mut Value, segs: &[String]) -> Option<&'a mut Value> {
    let mut cur = root;
    for s in segs {
        cur = cur.as_object_mut()?.get_mut(s)?;
    }
    Some(cur)
}

#[allow(dead_code)] // helper for reverse_merge (wired in a later task)
fn apply_root_leaf(root: &mut Value, k: &OwnedKey) {
    if *root == k.wrote {
        match (k.prior.present, &k.prior.value) {
            (true, Some(v)) => *root = v.clone(),
            _ => *root = Value::Object(serde_json::Map::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run(user: Value, frag: Value) -> (Value, Vec<OwnedKey>) {
        let mut u = user;
        let mut out = Vec::new();
        let mut path = Vec::new();
        merge_with_snapshot(&mut u, &frag, &mut path, &mut out);
        (u, out)
    }

    #[test]
    fn absent_key_is_one_whole_subtree_leaf() {
        let (merged, owned) = run(json!({}), json!({"a": {"b": {"c": 1}}}));
        assert_eq!(merged, json!({"a": {"b": {"c": 1}}}));
        assert_eq!(owned.len(), 1, "absent frag key → exactly ONE leaf");
        assert_eq!(owned[0].path, vec!["a".to_string()]);
        assert!(!owned[0].prior.present);
        assert_eq!(owned[0].prior.value, None);
        assert_eq!(owned[0].wrote, json!({"b": {"c": 1}}));
    }

    #[test]
    fn scalar_overwrite_records_prior_present() {
        let (merged, owned) = run(json!({"k": "old"}), json!({"k": "new"}));
        assert_eq!(merged, json!({"k": "new"}));
        assert_eq!(owned.len(), 1);
        assert_eq!(owned[0].path, vec!["k".to_string()]);
        assert!(owned[0].prior.present);
        assert_eq!(owned[0].prior.value, Some(json!("old")));
        assert_eq!(owned[0].wrote, json!("new"));
    }

    #[test]
    fn nested_recurse_yields_multiple_independent_leaves() {
        // user has permissions.otherKey; frag adds permissions.defaultMode + a new top key.
        let (merged, owned) = run(
            json!({"permissions": {"otherKey": true}}),
            json!({"permissions": {"defaultMode": "x"}, "model": "claude"}),
        );
        assert_eq!(
            merged,
            json!({"permissions": {"otherKey": true, "defaultMode": "x"}, "model": "claude"})
        );
        // two leaves: [permissions, defaultMode] (absent) and [model] (absent).
        assert_eq!(owned.len(), 2);
        let dm = owned
            .iter()
            .find(|k| k.path == vec!["permissions".to_string(), "defaultMode".to_string()])
            .expect("defaultMode leaf");
        assert!(!dm.prior.present);
        assert_eq!(dm.wrote, json!("x"));
        let model = owned
            .iter()
            .find(|k| k.path == vec!["model".to_string()])
            .expect("model leaf");
        assert!(!model.prior.present);
    }

    #[test]
    fn array_is_one_whole_array_leaf() {
        let (merged, owned) = run(
            json!({"permissions": {"allow": ["a", "b"]}}),
            json!({"permissions": {"allow": ["c"]}}),
        );
        assert_eq!(merged, json!({"permissions": {"allow": ["c"]}}));
        assert_eq!(owned.len(), 1, "array = ONE leaf (whole replace, no union)");
        assert_eq!(
            owned[0].path,
            vec!["permissions".to_string(), "allow".to_string()]
        );
        assert!(owned[0].prior.present);
        assert_eq!(owned[0].prior.value, Some(json!(["a", "b"])));
        assert_eq!(owned[0].wrote, json!(["c"]));
    }

    #[test]
    fn frag_object_replaces_user_scalar_as_one_leaf() {
        // user.permissions is a scalar; frag.permissions is an object → NOT both
        // objects → overwrite as one leaf prior.present=true.
        let (merged, owned) = run(
            json!({"permissions": "deny"}),
            json!({"permissions": {"defaultMode": "x"}}),
        );
        assert_eq!(merged, json!({"permissions": {"defaultMode": "x"}}));
        assert_eq!(owned.len(), 1);
        assert_eq!(owned[0].path, vec!["permissions".to_string()]);
        assert!(owned[0].prior.present);
        assert_eq!(owned[0].prior.value, Some(json!("deny")));
    }

    #[test]
    fn non_object_root_overwrite_root_leaf() {
        // both roots non-object-vs-object mismatch at the very top → root leaf path=[].
        let (merged, owned) = run(json!("old-root"), json!({"k": 1}));
        assert_eq!(merged, json!({"k": 1}));
        assert_eq!(owned.len(), 1);
        assert!(owned[0].path.is_empty(), "root leaf has empty path");
        assert!(owned[0].prior.present);
        assert_eq!(owned[0].prior.value, Some(json!("old-root")));
        assert_eq!(owned[0].wrote, json!({"k": 1}));
    }

    use std::fs;
    use tempfile::TempDir;

    fn write_disk(dir: &TempDir, v: &Value) -> std::path::PathBuf {
        let p = dir.path().join("settings.json");
        fs::write(&p, serde_json::to_vec_pretty(v).unwrap()).unwrap();
        p
    }
    fn env_json(keys: Vec<OwnedKey>) -> String {
        serde_json::to_string(&OwnedKeysEnvelope {
            v: OWNED_KEYS_VERSION,
            keys,
        })
        .unwrap()
    }
    fn read_disk(p: &std::path::Path) -> Value {
        serde_json::from_slice(&fs::read(p).unwrap()).unwrap()
    }

    #[test]
    fn reverse_removes_inserted_leaf_when_still_ours() {
        let dir = TempDir::new().unwrap();
        // we inserted [model] (prior absent), wrote "claude"; disk still has it.
        let p = write_disk(&dir, &json!({"model": "claude", "userKept": 1}));
        let env = env_json(vec![OwnedKey {
            path: vec!["model".into()],
            prior: PriorLeaf {
                present: false,
                value: None,
            },
            wrote: json!("claude"),
        }]);
        let mut w = Vec::new();
        super::reverse_merge(&p, Some(&env), &mut w).unwrap();
        assert_eq!(
            read_disk(&p),
            json!({"userKept": 1}),
            "our leaf removed, user key survives"
        );
    }

    #[test]
    fn reverse_leaves_user_edited_leaf() {
        let dir = TempDir::new().unwrap();
        // we inserted [model]="claude", but user edited disk to "gpt" → cur!=wrote → leave.
        let p = write_disk(&dir, &json!({"model": "gpt"}));
        let env = env_json(vec![OwnedKey {
            path: vec!["model".into()],
            prior: PriorLeaf {
                present: false,
                value: None,
            },
            wrote: json!("claude"),
        }]);
        let mut w = Vec::new();
        super::reverse_merge(&p, Some(&env), &mut w).unwrap();
        assert_eq!(
            read_disk(&p),
            json!({"model": "gpt"}),
            "user edit preserved (M1)"
        );
    }

    #[test]
    fn reverse_restores_overwritten_leaf_to_prior() {
        let dir = TempDir::new().unwrap();
        // we overwrote [model] from "old" to "new"; disk still "new" → restore "old".
        let p = write_disk(&dir, &json!({"model": "new"}));
        let env = env_json(vec![OwnedKey {
            path: vec!["model".into()],
            prior: PriorLeaf {
                present: true,
                value: Some(json!("old")),
            },
            wrote: json!("new"),
        }]);
        let mut w = Vec::new();
        super::reverse_merge(&p, Some(&env), &mut w).unwrap();
        assert_eq!(
            read_disk(&p),
            json!({"model": "old"}),
            "prior restored (M7)"
        );
    }

    #[test]
    fn reverse_user_empty_object_survives_as_empty_object() {
        // M3b (the adversarial-fix headline): user {"permissions":{}} + frag adds
        // [permissions, defaultMode] (prior absent). Detach removes defaultMode; the
        // user's `permissions` MUST survive as {} — NO collapse of created ancestors.
        let dir = TempDir::new().unwrap();
        let p = write_disk(&dir, &json!({"permissions": {"defaultMode": "x"}}));
        let env = env_json(vec![OwnedKey {
            path: vec!["permissions".into(), "defaultMode".into()],
            prior: PriorLeaf {
                present: false,
                value: None,
            },
            wrote: json!("x"),
        }]);
        let mut w = Vec::new();
        super::reverse_merge(&p, Some(&env), &mut w).unwrap();
        assert_eq!(
            read_disk(&p),
            json!({"permissions": {}}),
            "user's permissions object MUST survive as {{}} (no collapse)"
        );
    }

    #[test]
    fn reverse_partial_nested_keeps_sibling_user_key() {
        // M3: frag {permissions:{defaultMode}} into user {permissions:{otherKey}};
        // reverse removes defaultMode, otherKey survives, permissions NOT removed.
        let dir = TempDir::new().unwrap();
        let p = write_disk(
            &dir,
            &json!({"permissions": {"otherKey": true, "defaultMode": "x"}}),
        );
        let env = env_json(vec![OwnedKey {
            path: vec!["permissions".into(), "defaultMode".into()],
            prior: PriorLeaf {
                present: false,
                value: None,
            },
            wrote: json!("x"),
        }]);
        let mut w = Vec::new();
        super::reverse_merge(&p, Some(&env), &mut w).unwrap();
        assert_eq!(read_disk(&p), json!({"permissions": {"otherKey": true}}));
    }

    #[test]
    fn reverse_whole_array_restores_prior_array() {
        // M5: we replaced [permissions, allow] from ["a","b"] to ["c"]; restore.
        let dir = TempDir::new().unwrap();
        let p = write_disk(&dir, &json!({"permissions": {"allow": ["c"]}}));
        let env = env_json(vec![OwnedKey {
            path: vec!["permissions".into(), "allow".into()],
            prior: PriorLeaf {
                present: true,
                value: Some(json!(["a", "b"])),
            },
            wrote: json!(["c"]),
        }]);
        let mut w = Vec::new();
        super::reverse_merge(&p, Some(&env), &mut w).unwrap();
        assert_eq!(read_disk(&p), json!({"permissions": {"allow": ["a", "b"]}}));
    }

    #[test]
    fn reverse_fail_closed_on_unknown_version_or_bad_input() {
        let dir = TempDir::new().unwrap();
        // M8a: version 99 → leave file untouched, Ok, warn.
        let p = write_disk(&dir, &json!({"model": "claude"}));
        let mut w = Vec::new();
        super::reverse_merge(&p, Some(r#"{"v":99,"keys":[]}"#), &mut w).unwrap();
        assert_eq!(read_disk(&p), json!({"model": "claude"}), "v99 leaves file");
        assert!(w.iter().any(|m| m.contains("version 99")));

        // M8b: not JSON → leave, Ok, warn.
        let mut w2 = Vec::new();
        super::reverse_merge(&p, Some("not json"), &mut w2).unwrap();
        assert_eq!(read_disk(&p), json!({"model": "claude"}));
        assert!(!w2.is_empty());

        // M8c: None / empty → Ok, no warn, untouched.
        let mut w3 = Vec::new();
        super::reverse_merge(&p, None, &mut w3).unwrap();
        super::reverse_merge(&p, Some(""), &mut w3).unwrap();
        assert!(w3.is_empty());
        assert_eq!(read_disk(&p), json!({"model": "claude"}));
    }

    #[test]
    fn reverse_missing_file_is_ok() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("settings.json"); // does not exist
        let env = env_json(vec![OwnedKey {
            path: vec!["model".into()],
            prior: PriorLeaf {
                present: false,
                value: None,
            },
            wrote: json!("claude"),
        }]);
        let mut w = Vec::new();
        super::reverse_merge(&p, Some(&env), &mut w).unwrap();
        assert!(!p.exists(), "missing file stays missing");
    }

    #[test]
    fn reverse_bad_disk_json_leaves_file() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("settings.json");
        fs::write(&p, b"{ this is : not json").unwrap();
        let env = env_json(vec![OwnedKey {
            path: vec!["model".into()],
            prior: PriorLeaf {
                present: false,
                value: None,
            },
            wrote: json!("claude"),
        }]);
        let mut w = Vec::new();
        super::reverse_merge(&p, Some(&env), &mut w).unwrap();
        assert_eq!(
            fs::read(&p).unwrap(),
            b"{ this is : not json",
            "byte-identical"
        );
        assert!(w.iter().any(|m| m.contains("not valid JSON")));
    }
}
