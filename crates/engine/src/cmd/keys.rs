//! Layer keys (see `figcraft_doc::key`): set and clear them, resolve `{"key": "icon/folder"}` to a
//! layer id in any command's id parameters, and keep them unique per document.

use std::collections::HashMap;
use std::sync::Arc;

use figcraft_doc::key::key_problem;
use figcraft_doc::{Document, NodeId};
use serde_json::{Value, json};

use super::CommandSpec;
use crate::{Result, bad, spec};

/// Parameters that name layers; a `{"key": ".."}` value (or list entry) in one is replaced by the id.
const ID_PARAMS: [&str; 9] = ["id", "ids", "parent", "root", "target", "targets", "component", "from", "to"];

/// Commands that take a bare top-level `"key": ".."` as their target layer (in place of `id`).
const KEY_TARGETED: [&str; 10] = [
    "node.set",
    "node.get",
    "node.reorder",
    "node.reparent",
    "edit.delete",
    "edit.duplicate",
    "select.set",
    "object.rename",
    "object.toggleVisible",
    "object.toggleLock",
];

/// The error for a key nothing has: the closest keys, so the caller can fix a typo.
fn unknown(cmd: &str, doc: &Document, key: &str) -> crate::EngineError {
    let lower = key.to_lowercase();
    let tail = lower.rsplit('/').next().unwrap_or(&lower).to_string();
    let all = doc.keys();
    let mut near: Vec<&str> = all.iter().map(|(k, _)| k.as_str()).filter(|k| k.to_lowercase().contains(&tail)).take(5).collect();
    if near.is_empty() {
        near = all.iter().map(|(k, _)| k.as_str()).take(5).collect();
    }
    let hint = if near.is_empty() {
        "no layer has a key yet (set one with node.set {props: {key}})".to_string()
    } else {
        format!("did you mean one of: {}", near.join(", "))
    };
    bad(cmd, format!("no layer has the key `{key}`; {hint}"))
}

/// `v` as an id when it is `{"key": ".."}`; `None` for any other value.
fn resolve_one(cmd: &str, doc: &Document, v: &Value) -> Result<Option<Value>> {
    let Some(k) = v.as_object().filter(|o| o.len() == 1).and_then(|o| o.get("key")).and_then(Value::as_str) else { return Ok(None) };
    doc.node_by_key(k).map(|id| Some(json!(id))).ok_or_else(|| unknown(cmd, doc, k))
}

/// `p` with every key reference replaced by the layer id, or `None` when it has none (the common case).
pub(crate) fn resolve_params(cmd: &str, doc: &Document, p: &Value) -> Result<Option<Value>> {
    let Some(o) = p.as_object() else { return Ok(None) };
    let mut out: Option<serde_json::Map<String, Value>> = None;
    for name in ID_PARAMS {
        let Some(v) = o.get(name) else { continue };
        let new = match v {
            Value::Array(a) => {
                let mut list = Vec::with_capacity(a.len());
                let mut changed = false;
                for x in a {
                    match resolve_one(cmd, doc, x)? {
                        Some(id) => {
                            list.push(id);
                            changed = true;
                        }
                        None => list.push(x.clone()),
                    }
                }
                changed.then_some(Value::Array(list))
            }
            other => resolve_one(cmd, doc, other)?,
        };
        if let Some(new) = new {
            out.get_or_insert_with(|| o.clone()).insert(name.to_string(), new);
        }
    }
    if KEY_TARGETED.contains(&cmd)
        && let Some(k) = o.get("key").and_then(Value::as_str)
        && !o.contains_key("id")
        && !o.contains_key("ids")
    {
        let id = doc.node_by_key(k).ok_or_else(|| unknown(cmd, doc, k))?;
        let m = out.get_or_insert_with(|| o.clone());
        m.insert("id".into(), json!(id));
        m.remove("key");
    }
    Ok(out.map(Value::Object))
}

/// Give layer `id` the key `key` (`None` or an empty string clears it). A key another layer has is an
/// error that names that layer and a free alternative.
pub(crate) fn claim(cmd: &str, doc: &mut Document, id: NodeId, key: Option<&str>) -> Result<()> {
    let key = key.filter(|k| !k.is_empty());
    if let Some(k) = key {
        if let Some(why) = key_problem(k) {
            return Err(bad(cmd, why));
        }
        if let Some(other) = doc.node_by_key(k).filter(|o| *o != id) {
            return Err(bad(
                cmd,
                format!("the key `{k}` is already used by layer {other}; keys are unique per document (try `{}`)", doc.free_key(k)),
            ));
        }
    }
    doc.get_mut(id)?.set_key(key);
    Ok(())
}

/// After a command: keys stay unique. A copy (a layer that did not exist before) whose key another
/// layer has loses it (duplicates get no key); an existing layer given a taken key fails the command.
pub(crate) fn enforce_unique(cmd: &str, doc: &mut Arc<Document>, before: &Document) -> Result<()> {
    let changed: Vec<NodeId> = doc
        .nodes()
        .filter(|n| n.key().is_some())
        .filter(|n| match before.node_arc(n.id) {
            Some(b) => b.key() != n.key(),
            None => true,
        })
        .map(|n| n.id)
        .collect();
    if changed.is_empty() {
        return Ok(());
    }
    let mut count: HashMap<&str, usize> = HashMap::new();
    for n in doc.nodes() {
        if let Some(k) = n.key() {
            *count.entry(k).or_default() += 1;
        }
    }
    let clashing: Vec<NodeId> =
        changed.into_iter().filter(|id| doc.node(*id).and_then(|n| n.key()).is_some_and(|k| count.get(k).copied().unwrap_or(0) > 1)).collect();
    for id in clashing {
        if before.contains(id) {
            let k = doc.node(id).and_then(|n| n.key()).unwrap_or_default().to_string();
            return Err(bad(
                cmd,
                format!("the key `{k}` is already used by another layer; keys are unique per document (try `{}`)", doc.free_key(&k)),
            ));
        }
        if let Some(n) = Arc::make_mut(doc).node_mut(id) {
            n.set_key(None);
        }
    }
    Ok(())
}

pub fn specs() -> Vec<CommandSpec> {
    vec![spec!(
        "node.resolveKey",
        "Layer ids from keys",
        "{key | keys: [..]} → {id} | {ids}  (the layers with these keys; fails naming close keys when one is unknown). Every command that takes an id also takes {\"key\": \"icon/folder\"} there (id, ids, parent, root, target, component, from, to), and node.set / node.get / node.reorder / node.reparent / edit.delete / edit.duplicate / select.set / object.rename take a top-level key as their target",
        false,
        |s, p| {
            let cmd = "node.resolveKey";
            let doc = s.doc()?;
            if let Some(keys) = p.get("keys").and_then(Value::as_array) {
                let ids = keys
                    .iter()
                    .map(|k| {
                        k.as_str()
                            .and_then(|k| doc.node_by_key(k).ok_or_else(|| unknown(cmd, doc, k)).ok())
                            .ok_or_else(|| bad(cmd, "`keys` must be known key strings"))
                    })
                    .collect::<Result<Vec<_>>>()?;
                return Ok(json!({"ids": ids}));
            }
            let k = crate::params::req_string(cmd, p, "key")?;
            Ok(json!({"id": doc.node_by_key(&k).ok_or_else(|| unknown(cmd, doc, &k))?}))
        }
    )]
}

#[cfg(test)]
mod tests {
    use crate::Session;
    use serde_json::json;

    fn session() -> Session {
        Session::new()
    }

    fn make(s: &mut Session, key: Option<&str>) -> u64 {
        let mut p = json!({"type": "rectangle", "width": 10, "height": 10});
        if let Some(k) = key {
            p["key"] = json!(k);
        }
        s.execute("node.create", &p).unwrap()["id"].as_u64().unwrap()
    }

    #[test]
    fn a_key_names_a_layer_in_ids_and_in_node_get_and_find() {
        let mut s = session();
        let a = make(&mut s, Some("icon/folder"));
        let b = make(&mut s, None);
        s.execute("node.set", &json!({"key": "icon/folder", "props": {"opacity": 0.5}})).unwrap();
        assert_eq!(s.execute("node.get", &json!({"id": {"key": "icon/folder"}})).unwrap()["opacity"], json!(0.5));
        assert_eq!(s.execute("node.find", &json!({"key": "icon/folder"})).unwrap()["ids"], json!([a]));
        assert_eq!(s.execute("node.find", &json!({"keyGlob": "icon/*"})).unwrap()["ids"], json!([a]));
        assert_eq!(s.execute("node.resolveKey", &json!({"key": "icon/folder"})).unwrap(), json!({"id": a}));
        // The key shows in get_node's JSON, and layers without one show none.
        assert_eq!(s.execute("node.get", &json!({"id": a})).unwrap()["key"], json!("icon/folder"));
        assert!(s.execute("node.get", &json!({"id": b})).unwrap().get("key").is_none());
        let e = s.execute("node.set", &json!({"key": "icon/folde", "props": {"opacity": 0.5}})).unwrap_err().to_string();
        assert!(e.contains("did you mean one of: icon/folder"), "{e}");
    }

    #[test]
    fn keys_are_unique_and_survive_undo_but_not_duplication() {
        let mut s = session();
        let a = make(&mut s, Some("k"));
        let e = s.execute("node.create", &json!({"type": "rectangle", "key": "k"})).unwrap_err().to_string();
        assert!(e.contains("already used by layer") && e.contains("try `k-2`"), "{e}");
        let b = make(&mut s, None);
        let e = s.execute("node.set", &json!({"id": b, "props": {"key": "k"}})).unwrap_err().to_string();
        assert!(e.contains("try `k-2`"), "{e}");
        s.execute("node.set", &json!({"id": a, "props": {"key": "j"}})).unwrap();
        s.undo().unwrap();
        assert_eq!(s.execute("node.resolveKey", &json!({"key": "k"})).unwrap(), json!({"id": a}));
        s.redo().unwrap();
        assert_eq!(s.execute("node.resolveKey", &json!({"key": "j"})).unwrap(), json!({"id": a}));
        let copy = s.execute("edit.duplicate", &json!({"ids": [{"key": "j"}]})).unwrap();
        let copy = copy["ids"][0].as_u64().unwrap();
        assert!(s.execute("node.get", &json!({"id": copy})).unwrap().get("key").is_none());
        assert_eq!(s.execute("node.find", &json!({"keyGlob": "*"})).unwrap()["ids"], json!([a]));
        s.execute("node.set", &json!({"id": a, "props": {"key": null}})).unwrap();
        assert_eq!(s.execute("node.find", &json!({"keyGlob": "*"})).unwrap()["total"], json!(0));
    }

    #[test]
    fn batch_steps_use_keys_made_by_earlier_steps() {
        let mut s = session();
        let r = s
            .execute(
                "command.batch",
                &json!({"commands": [
                    {"command": "node.create", "params": {"type": "frame", "key": "card"}},
                    {"command": "node.create", "params": {"type": "rectangle", "parent": {"key": "card"}, "key": "card/bg"}}
                ]}),
            )
            .unwrap();
        let _ = r;
        let ids = s.execute("node.find", &json!({"keyGlob": "card*"})).unwrap();
        assert_eq!(ids["total"], json!(2));
    }
}
