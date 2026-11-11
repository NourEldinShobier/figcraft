//! `doc.diff`: a structural diff of two open documents, or of the current document against one of
//! its saved versions. Layers are matched by id for a version (the same file) and by path
//! (page / ancestor names / name) for two documents, whose ids are unrelated. The result lists
//! added, removed, moved and changed layers with the names of the properties that changed and
//! their old and new values, for a bounded number of layers.

use std::collections::{BTreeMap, BTreeSet};

use figcraft_doc::{Document, NodeId};
use serde_json::{Map, Value, json};

use super::CommandSpec;
use crate::params::get;
use crate::{Result, Session, bad, spec};

/// Layers read from one document; a larger one is refused rather than half compared.
const MAX_LAYERS: usize = 100_000;
/// Items listed by default, and at most.
const LISTED: usize = 50;
const MAX_LIMIT: usize = 1000;
/// Changed properties listed per layer, and the longest old or new value shown (chars).
const PROPS_LISTED: usize = 8;
const MAX_VALUE: usize = 80;

pub fn specs() -> Vec<CommandSpec> {
    vec![spec!(
        "doc.diff",
        "Diff documents",
        "{a? (open document id or path), b? (default the current document), version? (a version id of the current document, instead of `a`), limit? (items listed, default 50, up to 1000)}  structural diff from `a` (or the version) to `b` → {mode, a, b, identical, counts: {added, removed, moved, changed}, changed: [{id, path, type, changes: [{prop, old, new}], moreProps?}], moved: [{id, path, from, to, changes?}], added: [{id, path, type, layers}], removed: [...], truncated?, note?}. Layers match by id for a version and by page/name path for two documents; `added`/`removed` list the top layer of each added or removed subtree (`layers` counts it all); a moved layer is one whose parent changed (two documents: an identical layer that disappeared from one place and appeared in another). Z-order changes are not reported",
        false,
        diff
    )]
}

/// One layer as the diff sees it, in document order (a parent before its children).
struct Entry {
    id: NodeId,
    path: String,
    parent: Option<usize>,
    kind: String,
    name: String,
    props: Map<String, Value>,
}

fn snapshot(cmd: &str, doc: &Document) -> Result<Vec<Entry>> {
    let mut out: Vec<Entry> = Vec::new();
    let mut stack: Vec<(NodeId, Option<usize>, String)> = Vec::new();
    let mut first: BTreeMap<String, usize> = BTreeMap::new();
    let mut paths = Vec::new();
    for p in &doc.pages {
        let name = doc.node(*p).map(|n| n.name.clone()).unwrap_or_default();
        paths.push((*p, unique(&mut first, &name, "")));
    }
    stack.extend(paths.into_iter().rev().map(|(id, path)| (id, None, path)));
    while let Some((id, parent, path)) = stack.pop() {
        if out.len() >= MAX_LAYERS {
            return Err(bad(cmd, format!("a document with more than {MAX_LAYERS} layers is too large to diff")));
        }
        let Some(n) = doc.node(id) else { continue };
        let mut props = match crate::inspect::node_json(doc, id, 0) {
            Value::Object(o) => o,
            _ => Map::new(),
        };
        for k in ["id", "childCount", "children", "absoluteBounds"] {
            props.remove(k);
        }
        let at = out.len();
        out.push(Entry { id, path: path.clone(), parent, kind: n.kind.type_name().to_string(), name: n.name.clone(), props });
        let mut seen: BTreeMap<String, usize> = BTreeMap::new();
        let kids: Vec<_> = n.children.iter().filter_map(|c| doc.node(*c).map(|k| (*c, Some(at), unique(&mut seen, &k.name, &path)))).collect();
        stack.extend(kids.into_iter().rev());
    }
    Ok(out)
}

/// `parent/name`, with `#2`, `#3`… for the second and later sibling of the same name.
fn unique(seen: &mut BTreeMap<String, usize>, name: &str, parent: &str) -> String {
    let n = seen.entry(name.to_string()).or_insert(0);
    *n += 1;
    let base = if parent.is_empty() { name.to_string() } else { format!("{parent}/{name}") };
    if *n > 1 { format!("{base}#{n}") } else { base }
}

fn brief(v: &Value) -> Value {
    let s = v.to_string();
    if s.chars().count() <= MAX_VALUE {
        return v.clone();
    }
    Value::String(format!("{}…", s.chars().take(MAX_VALUE).collect::<String>()))
}

/// The properties that differ between two layers, as `{prop, old, new}`.
fn prop_changes(a: &Map<String, Value>, b: &Map<String, Value>) -> Vec<Value> {
    let keys: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
    keys.into_iter()
        .filter(|k| a.get(*k) != b.get(*k))
        .map(|k| json!({"prop": k, "old": a.get(k).map(brief).unwrap_or(Value::Null), "new": b.get(k).map(brief).unwrap_or(Value::Null)}))
        .collect()
}

fn with_changes(mut item: Value, mut changes: Vec<Value>) -> Value {
    if changes.len() > PROPS_LISTED {
        item["moreProps"] = json!(changes.len() - PROPS_LISTED);
        changes.truncate(PROPS_LISTED);
    }
    if !changes.is_empty() {
        item["changes"] = json!(changes);
    }
    item
}

/// Entries whose parent is not itself in `set`, with the number of entries under (and with) each.
fn roots(es: &[Entry], set: &BTreeSet<usize>) -> Vec<(usize, usize)> {
    let mut root_of: BTreeMap<usize, usize> = BTreeMap::new();
    let mut layers: BTreeMap<usize, usize> = BTreeMap::new();
    for i in set {
        let root = match es.get(*i).and_then(|e| e.parent).and_then(|p| root_of.get(&p)) {
            Some(r) => *r,
            None => *i,
        };
        root_of.insert(*i, root);
        *layers.entry(root).or_default() += 1;
    }
    layers.into_iter().collect()
}

fn same_look(a: &Entry, b: &Entry) -> bool {
    let strip = |m: &Map<String, Value>| {
        let mut m = m.clone();
        m.remove("x");
        m.remove("y");
        m
    };
    a.kind == b.kind && a.name == b.name && strip(&a.props) == strip(&b.props)
}

fn diff(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "doc.diff";
    let limit = match get(p, "limit") {
        None | Some(Value::Null) => LISTED,
        Some(v) => v.as_u64().filter(|n| *n <= MAX_LIMIT as u64).ok_or_else(|| bad(CMD, "`limit` must be an integer from 0 to 1000"))? as usize,
    };
    let find = |s: &Session, v: &Value| -> Result<(std::sync::Arc<Document>, String)> {
        let uid = s.document_uid(v).map_err(|e| bad(CMD, e.to_string()))?;
        let d = s.docs.iter().find(|d| d.uid == uid).ok_or_else(|| bad(CMD, "no such document"))?;
        Ok((d.doc.clone(), d.doc.name.clone()))
    };
    let present = |k: &str| get(p, k).filter(|v| !v.is_null()).cloned();
    let (after, b_label) = match present("b") {
        Some(v) => find(s, &v)?,
        None => {
            let d = s.ds()?;
            (d.doc.clone(), d.doc.name.clone())
        }
    };
    let (before, a_label, by_id, mode) = match (present("a"), present("version")) {
        (Some(_), Some(_)) => return Err(bad(CMD, "pass `a` or `version`, not both")),
        (Some(a), None) => {
            let (d, name) = find(s, &a)?;
            (d, json!(name), false, "documents")
        }
        (None, Some(v)) => {
            let id = v.as_str().ok_or_else(|| bad(CMD, "`version` must be a version id (version.list)"))?;
            let ds = s.ds()?;
            let ver = ds.versions.iter().find(|x| x.id == id).ok_or_else(|| bad(CMD, format!("no version `{id}`")))?;
            let d = crate::cmd::file::read_version(ver)?;
            (d, json!({"id": ver.id, "name": ver.name, "time": ver.time}), true, "version")
        }
        (None, None) => return Err(bad(CMD, "pass `a` (another open document) or `version`")),
    };
    diff_documents(CMD, &before, &after, by_id, limit, mode, a_label, b_label)
}

/// The structural diff of `before` to `after` as `doc.diff` reports it (`by_id`: match layers by
/// id, else by path). `doc.diff` and the preview of `command.batch` share it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn diff_documents(
    cmd: &str,
    before: &Document,
    after: &Document,
    by_id: bool,
    limit: usize,
    mode: &str,
    a_label: Value,
    b_label: String,
) -> Result<Value> {
    let (ea, eb) = (snapshot(cmd, before)?, snapshot(cmd, after)?);
    let key = |e: &Entry| if by_id { e.id.0.to_string() } else { e.path.clone() };
    let index_a: BTreeMap<String, usize> = ea.iter().enumerate().map(|(i, e)| (key(e), i)).collect();
    let index_b: BTreeMap<String, usize> = eb.iter().enumerate().map(|(i, e)| (key(e), i)).collect();

    let (mut changed, mut moved) = (Vec::new(), Vec::new());
    let mut gone: BTreeSet<usize> = (0..ea.len()).filter(|i| ea.get(*i).is_some_and(|e| !index_b.contains_key(&key(e)))).collect();
    let mut new: BTreeSet<usize> = (0..eb.len()).filter(|i| eb.get(*i).is_some_and(|e| !index_a.contains_key(&key(e)))).collect();
    for (k, bi) in &index_b {
        let (Some(ai), Some(a), Some(b)) = (index_a.get(k), index_a.get(k).and_then(|i| ea.get(*i)), eb.get(*bi)) else { continue };
        let changes = prop_changes(&a.props, &b.props);
        let parent_id = |es: &[Entry], e: &Entry| e.parent.and_then(|p| es.get(p)).map(|p| p.id);
        let item = json!({"id": b.id, "path": b.path, "type": b.kind});
        if by_id && parent_id(&ea, a) != parent_id(&eb, b) {
            let mut m = item;
            m["from"] = json!(a.path);
            m["to"] = json!(b.path);
            moved.push(with_changes(m, changes));
        } else if !changes.is_empty() {
            let mut c = item;
            if !by_id {
                c["idInA"] = json!(ea.get(*ai).map(|e| e.id));
            }
            changed.push(with_changes(c, changes));
        }
    }
    // Two documents: an unchanged subtree that left one place and turned up in another is a move.
    if !by_id {
        let gone_roots: Vec<usize> = roots(&ea, &gone).into_iter().map(|r| r.0).collect();
        let mut free: Vec<usize> = Vec::new();
        for g in gone_roots {
            let Some(ge) = ea.get(g) else { continue };
            let hit = roots(&eb, &new).into_iter().map(|r| r.0).find(|n| eb.get(*n).is_some_and(|ne| same_look(ge, ne) && !free.contains(n)));
            if let Some(n) = hit.and_then(|n| eb.get(n).map(|ne| (n, ne))) {
                free.push(n.0);
                moved.push(json!({"id": n.1.id, "idInA": ge.id, "path": n.1.path, "type": n.1.kind, "from": ge.path, "to": n.1.path}));
                for i in subtree(&ea, g) {
                    gone.remove(&i);
                }
                for i in subtree(&eb, n.0) {
                    new.remove(&i);
                }
            }
        }
    }
    let listing = |es: &[Entry], set: &BTreeSet<usize>| -> Vec<Value> {
        roots(es, set)
            .into_iter()
            .filter_map(|(r, layers)| es.get(r).map(|e| json!({"id": e.id, "path": e.path, "type": e.kind, "layers": layers})))
            .collect()
    };
    let (added, removed) = (listing(&eb, &new), listing(&ea, &gone));
    let counts = json!({"added": added.len(), "removed": removed.len(), "moved": moved.len(), "changed": changed.len()});
    let total = added.len() + removed.len() + moved.len() + changed.len();
    let mut room = limit;
    let mut out = json!({
        "mode": mode, "a": a_label, "b": b_label, "identical": total == 0, "counts": counts,
        "layers": {"a": ea.len(), "b": eb.len()},
    });
    for (name, list) in [("changed", changed), ("moved", moved), ("added", added), ("removed", removed)] {
        let shown: Vec<Value> = list.into_iter().take(room).collect();
        room -= shown.len();
        out[name] = json!(shown);
    }
    if total > limit {
        out["truncated"] = json!(true);
        out["note"] = json!(format!(
            "{} of {total} items listed (changed, moved, added, removed in that order); raise `limit` (up to {MAX_LIMIT}) or diff a smaller document",
            limit.min(total)
        ));
    }
    Ok(out)
}

/// `i` and the entries under it.
fn subtree(es: &[Entry], i: usize) -> Vec<usize> {
    let mut set: BTreeSet<usize> = BTreeSet::from([i]);
    for (j, e) in es.iter().enumerate().skip(i + 1) {
        if e.parent.is_some_and(|p| set.contains(&p)) {
            set.insert(j);
        }
    }
    set.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use figcraft_doc::NodeKind;
    use serde_json::json;

    use crate::Session;

    fn rect(s: &mut Session, name: &str, x: f64) -> figcraft_doc::NodeId {
        let page = s.page().unwrap();
        s.doc_mut().unwrap().create(page, NodeKind::Rectangle, name, x, 0.0, 40.0, 40.0).unwrap()
    }

    #[test]
    fn a_version_diff_lists_added_removed_and_changed_layers_with_old_and_new_values() {
        let mut s = Session::new();
        let keep = rect(&mut s, "Keep", 0.0);
        let _gone = rect(&mut s, "Gone", 100.0);
        let v = s.execute("file.saveVersion", &json!({"name": "base"})).unwrap();
        s.execute("node.set", &json!({"ids": [keep], "props": {"width": 80}})).unwrap();
        let gone = s.doc().unwrap().pages.first().copied().and_then(|p| s.doc().unwrap().node(p).unwrap().children.get(1).copied()).unwrap();
        s.execute("edit.delete", &json!({"ids": [gone]})).unwrap();
        rect(&mut s, "Fresh", 200.0);
        let r = s.execute("doc.diff", &json!({"version": v["id"]})).unwrap();
        assert_eq!(r["counts"], json!({"added": 1, "removed": 1, "moved": 0, "changed": 1}), "{r}");
        assert_eq!(r["changed"][0]["changes"], json!([{"prop": "width", "old": 40.0, "new": 80.0}]), "{r}");
        assert_eq!(r["added"][0]["path"], json!("Page 1/Fresh"), "{r}");
        assert_eq!(r["removed"][0]["path"], json!("Page 1/Gone"), "{r}");
    }

    #[test]
    fn two_documents_match_by_path_and_a_relocated_layer_is_moved() {
        let mut s = Session::new();
        rect(&mut s, "Logo", 0.0);
        rect(&mut s, "Title", 50.0);
        let a = s.ds().unwrap().uid;
        s.execute("file.new", &json!({"name": "Second"})).unwrap();
        let b = s.ds().unwrap().uid;
        assert_ne!(a, b);
        rect(&mut s, "Logo", 0.0);
        let r = s.execute("doc.diff", &json!({"a": a, "b": b})).unwrap();
        assert_eq!(r["mode"], json!("documents"), "{r}");
        assert!(r["counts"]["removed"].as_u64().unwrap() >= 1, "Title is only in a: {r}");
        assert!(r["removed"].as_array().unwrap().iter().any(|x| x["path"] == json!("Page 1/Title")), "{r}");
        let same = s.execute("doc.diff", &json!({"a": a, "b": a})).unwrap();
        assert_eq!(same["identical"], json!(true), "{same}");
    }

    #[test]
    fn a_layer_that_changed_frames_between_two_documents_is_moved_not_added_and_removed() {
        let mut s = Session::new();
        let build = |s: &mut Session, logo_in: usize| {
            let page = s.page().unwrap();
            for i in 1..=2 {
                let f = s.doc_mut().unwrap().create(page, NodeKind::Frame, &format!("F{i}"), 0.0, i as f64 * 500.0, 300.0, 300.0).unwrap();
                if i == logo_in {
                    s.doc_mut().unwrap().create(f, NodeKind::Rectangle, "Logo", 0.0, 0.0, 40.0, 40.0).unwrap();
                }
            }
        };
        build(&mut s, 1);
        let a = s.ds().unwrap().uid;
        s.execute("file.new", &json!({"name": "Second"})).unwrap();
        build(&mut s, 2);
        let r = s.execute("doc.diff", &json!({"a": a})).unwrap();
        assert_eq!(r["counts"], json!({"added": 0, "removed": 0, "moved": 1, "changed": 0}), "{r}");
        assert_eq!((r["moved"][0]["from"].clone(), r["moved"][0]["to"].clone()), (json!("Page 1/F1/Logo"), json!("Page 1/F2/Logo")), "{r}");
    }

    #[test]
    fn the_listing_is_bounded_and_says_so() {
        let mut s = Session::new();
        let v = s.execute("file.saveVersion", &json!({})).unwrap();
        for i in 0..5 {
            rect(&mut s, &format!("R{i}"), f64::from(i) * 50.0);
        }
        let r = s.execute("doc.diff", &json!({"version": v["id"], "limit": 2})).unwrap();
        assert_eq!(r["counts"]["added"], json!(5), "{r}");
        assert_eq!(r["added"].as_array().unwrap().len(), 2, "{r}");
        assert_eq!(r["truncated"], json!(true), "{r}");
    }

    #[test]
    fn junk_parameters_are_errors_not_panics() {
        let mut s = Session::new();
        for junk in [
            json!({}),
            json!({"a": "nope"}),
            json!({"a": 9999}),
            json!({"version": 3}),
            json!({"version": "x"}),
            json!({"a": 1, "version": "x"}),
            json!({"limit": -1, "a": 1}),
            json!({"limit": 99999, "version": "x"}),
            json!({"b": [1]}),
        ] {
            assert!(s.execute("doc.diff", &junk).is_err(), "{junk}");
        }
    }
}
