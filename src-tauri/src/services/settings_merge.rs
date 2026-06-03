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

use serde::{Deserialize, Serialize};
use serde_json::Value;

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
}
