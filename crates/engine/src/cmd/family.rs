//! Families: layers that stand for the same element on different screens (a header copied onto
//! every screen, a card repeated down a list). `family.find` scores likely twins of a layer,
//! `family.set` tags layers as one family (a `figcraft.family` plugin-data entry, so the file
//! format is unchanged), `family.list` shows the families and the untagged repeats, and
//! `family.propagate` copies what was changed on one layer onto its twins, through `node.set`.

use std::collections::{BTreeMap, BTreeSet};

use figcraft_doc::{Document, Node, NodeId, NodeKind};
use serde_json::{Map, Value, json};

use super::CommandSpec;
use crate::params::{bool_of, f64_of, get, opt_id, req_targets, string};
use crate::{EngineError, Result, Session, bad, find_command, spec};

/// The plugin-data key holding a layer's family name.
const TAG: &str = "figcraft.family";
/// Layers one call looks at, and layer pairs one propagation edits.
const MAX_SCAN: usize = 200_000;
const MAX_PAIRS: usize = 5_000;
/// What `family.propagate` leaves alone unless `props` names it: identity, geometry, content and
/// anything that belongs to one place in one screen.
const OWN: [&str; 17] = [
    "id",
    "name",
    "type",
    "x",
    "y",
    "width",
    "height",
    "absoluteBounds",
    "characters",
    "children",
    "childCount",
    "parent",
    "pluginData",
    "key",
    "sizing",
    "constraints",
    "rotation",
];
/// Never copied, even when named.
const NEVER: [&str; 5] = ["id", "type", "children", "childCount", "parent"];

pub fn specs() -> Vec<CommandSpec> {
    vec![
        spec!(
            "family.find",
            "Find similar layers",
            "{id? (default: the selection), minScore? (0-1, default 0.6), limit? (default 50)}  scores the layers on every page that look like the same element as `id` (same family tag, same component, or alike in structure, name and size) → {id, family, total, twins: [{id, name, type, page: {id, name}, frame: {id, name}, score, reasons, sameFrame}]}, best first",
            false,
            find
        ),
        spec!(
            "family.set",
            "Tag layers as one family",
            "{ids?, family? (default: the first layer's name)}  marks the layers as the same element so `family.find` and `family.propagate` treat them as twins at score 1 → {family, ids}",
            true,
            set_family
        ),
        spec!(
            "family.list",
            "List families",
            "{limit? (default 30)}  → {families: [{family, count, ids, frames}], suggested: [{name, type, count, ids, frames}]}; `suggested` are untagged layers with one name, type and structure that appear on two or more frames",
            false,
            list
        ),
        spec!(
            "family.propagate",
            "Apply a change to similar layers",
            "{from, props? (names to copy; default: every look property that differs, never name, text, position or size), ids? (the twins; default: the layers `family.find` scores at `minScore`), minScore? (default 0.8), deep? (default true: children are paired by position when both layers have the same child types)}  copies the values of `from` onto each twin through `node.set`, as one undo step. Use `dryRun` to see what would change → {from, twins, changed: [{id, props}], skipped: [{id, prop, why}]}",
            true,
            propagate
        ),
    ]
}

/// A name without its trailing counter or `copy`, lower-cased: "Header 2" and "header copy" match.
fn fold_name(name: &str) -> String {
    let mut t = name.trim().to_lowercase();
    loop {
        let cut = t.trim_end_matches(|c: char| c.is_ascii_digit() || c.is_whitespace() || matches!(c, '-' | '_' | '#')).to_string();
        let cut = cut.strip_suffix("copy").map(str::to_string).unwrap_or(cut);
        if cut == t {
            return t;
        }
        t = cut;
    }
}

pub(super) fn tag_of(n: &Node) -> Option<&str> {
    n.plugin_data.get(TAG).map(String::as_str).filter(|t| !t.is_empty())
}

/// The layer's type with the types of its descendants to `depth`, so equal strings mean equal structure.
fn shape(doc: &Document, id: NodeId, depth: usize) -> String {
    let Some(n) = doc.node(id) else { return String::new() };
    let mut out = n.kind.type_name().to_string();
    if depth > 0 && !n.children.is_empty() {
        out.push('(');
        for c in n.children.iter().take(16) {
            out.push_str(&shape(doc, *c, depth - 1));
            out.push(',');
        }
        out.push(')');
    }
    out
}

fn structure_sim(doc: &Document, a: &Node, b: &Node) -> f64 {
    if shape(doc, a.id, 3) == shape(doc, b.id, 3) {
        return 1.0;
    }
    let count = |n: &Node| {
        let mut m: BTreeMap<&'static str, usize> = BTreeMap::new();
        for c in &n.children {
            if let Some(c) = doc.node(*c) {
                *m.entry(c.kind.type_name()).or_default() += 1;
            }
        }
        m
    };
    let (ca, cb) = (count(a), count(b));
    let keys: BTreeSet<_> = ca.keys().chain(cb.keys()).collect();
    let (mut lo, mut hi) = (0usize, 0usize);
    for k in keys {
        let (x, y) = (ca.get(k).copied().unwrap_or(0), cb.get(k).copied().unwrap_or(0));
        lo += x.min(y);
        hi += x.max(y);
    }
    if hi == 0 { 1.0 } else { lo as f64 / hi as f64 }
}

fn ratio(a: f64, b: f64) -> f64 {
    let (a, b) = (a.abs(), b.abs());
    if !a.is_finite() || !b.is_finite() || a.max(b) <= f64::EPSILON { 1.0 } else { a.min(b) / a.max(b) }
}

/// How alike `a` and `b` are as one element, 0-1, with the reasons worth telling the caller.
fn score(doc: &Document, a: &Node, b: &Node) -> (f64, Vec<&'static str>) {
    let mut why = Vec::new();
    if let (Some(x), Some(y)) = (tag_of(a), tag_of(b))
        && x == y
    {
        return (1.0, vec!["same family tag"]);
    }
    let same_component = matches!((&a.kind, &b.kind), (NodeKind::Instance { component: x, .. }, NodeKind::Instance { component: y, .. }) if x == y);
    let (fa, fb) = (fold_name(&a.name), fold_name(&b.name));
    let name = if fa.is_empty() || fb.is_empty() {
        0.0
    } else if fa == fb {
        1.0
    } else if fa.contains(&fb) || fb.contains(&fa) {
        0.5
    } else {
        0.0
    };
    let st = if matches!(a.kind, NodeKind::Text(_)) { 1.0 } else { structure_sim(doc, a, b) };
    let size = (ratio(a.width, b.width) + ratio(a.height, b.height)) / 2.0;
    let mut s = 0.1 + 0.45 * st + 0.25 * name + 0.2 * size;
    if matches!(a.kind, NodeKind::Text(_)) && name == 0.0 {
        s = s.min(0.5);
    }
    if same_component {
        s = s.max(0.95);
        why.push("same component");
    }
    if st >= 0.999 && !matches!(a.kind, NodeKind::Text(_)) {
        why.push("same structure");
    }
    if name >= 0.999 {
        why.push("same name");
    }
    if size >= 0.98 {
        why.push("same size");
    }
    (s.min(1.0), why)
}

/// Every layer under the pages, parents first.
fn all_nodes(doc: &Document) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut stack: Vec<NodeId> = doc.pages.iter().rev().copied().collect();
    while let Some(id) = stack.pop() {
        if out.len() >= MAX_SCAN {
            break;
        }
        let Some(n) = doc.node(id) else { continue };
        if !matches!(n.kind, NodeKind::Page { .. }) {
            out.push(id);
        }
        stack.extend(n.children.iter().rev().copied());
    }
    out
}

/// The top-level frame holding `id` (a child of the page), and the page.
pub(super) fn frame_and_page(doc: &Document, id: NodeId) -> (Option<NodeId>, Option<NodeId>) {
    let (mut cur, mut frame) = (id, None);
    for _ in 0..10_000 {
        let Some(parent) = doc.node(cur).and_then(|n| n.parent) else { return (frame, None) };
        if doc.pages.contains(&parent) {
            return (Some(cur), Some(parent));
        }
        frame = Some(parent);
        cur = parent;
    }
    (frame, None)
}

pub(super) fn label(doc: &Document, id: Option<NodeId>) -> Value {
    match id.and_then(|i| doc.node(i)) {
        Some(n) => json!({"id": n.id, "name": n.name}),
        None => Value::Null,
    }
}

/// Layers like `from`, best first. `from` itself, what is inside it and what holds it are left out.
pub(super) fn twins_of(doc: &Document, from: NodeId, min: f64) -> Result<Vec<(NodeId, f64, Vec<&'static str>)>> {
    let a = doc.get(from).map_err(|e| bad("family.find", e.to_string()))?;
    let mut skip: BTreeSet<NodeId> = doc.ancestors(from).into_iter().collect();
    skip.insert(from);
    let mut stack = a.children.clone();
    while let Some(id) = stack.pop() {
        if skip.insert(id)
            && let Some(n) = doc.node(id)
        {
            stack.extend(n.children.iter().copied());
        }
    }
    let kind = a.kind.type_name();
    let mut out = Vec::new();
    for id in all_nodes(doc) {
        let Some(b) = doc.node(id) else { continue };
        if skip.contains(&id) || b.kind.type_name() != kind {
            continue;
        }
        let (s, why) = score(doc, a, b);
        if s >= min {
            out.push((id, s, why));
        }
    }
    out.sort_by(|x, y| y.1.total_cmp(&x.1).then(x.0.cmp(&y.0)));
    Ok(out)
}

fn min_score(cmd: &str, p: &Value, default: f64) -> Result<f64> {
    match get(p, "minScore") {
        None | Some(Value::Null) => Ok(default),
        Some(v) => f64_of(v).filter(|f| (0.0..=1.0).contains(f)).ok_or_else(|| bad(cmd, "`minScore` must be a number from 0 to 1")),
    }
}

fn limit(cmd: &str, p: &Value, default: usize) -> Result<usize> {
    match get(p, "limit") {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v.as_u64().filter(|n| *n <= 1000).map(|n| n as usize).ok_or_else(|| bad(cmd, "`limit` must be an integer from 0 to 1000")),
    }
}

fn find(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "family.find";
    let from = *req_targets(CMD, s, p)?.first().ok_or_else(|| bad(CMD, "pass `id`"))?;
    let min = min_score(CMD, p, 0.6)?;
    let limit = limit(CMD, p, 50)?;
    let doc = s.doc()?;
    let hits = twins_of(doc, from, min)?;
    let own_frame = frame_and_page(doc, from).0;
    let twins: Vec<Value> = hits
        .iter()
        .take(limit)
        .map(|(id, sc, why)| {
            let (frame, page) = frame_and_page(doc, *id);
            json!({
                "id": id,
                "name": doc.node(*id).map(|n| n.name.clone()),
                "type": doc.node(*id).map(|n| n.kind.type_name()),
                "page": label(doc, page),
                "frame": label(doc, frame),
                "score": (sc * 100.0).round() / 100.0,
                "reasons": why,
                "sameFrame": frame.is_some() && frame == own_frame,
            })
        })
        .collect();
    let family = doc.node(from).and_then(tag_of);
    Ok(json!({"id": from, "family": family, "total": hits.len(), "twins": twins}))
}

fn set_family(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "family.set";
    let ids = req_targets(CMD, s, p)?;
    let family = match string(p, "family") {
        Some(f) if !f.trim().is_empty() => f.trim().to_string(),
        _ => {
            let doc = s.doc()?;
            let name = ids.first().and_then(|i| doc.node(*i)).map(|n| fold_name(&n.name)).unwrap_or_default();
            if name.is_empty() { "family".to_string() } else { name }
        }
    };
    let set = find_command("node.set").ok_or_else(|| EngineError::UnknownCommand("node.set".into()))?;
    (set.run)(s, &json!({"ids": ids, "props": {"pluginData": {TAG: family}}}))?;
    Ok(json!({"family": family, "ids": ids}))
}

pub(super) fn list(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "family.list";
    let limit = limit(CMD, p, 30)?;
    let doc = s.doc()?;
    let mut tagged: BTreeMap<String, Vec<NodeId>> = BTreeMap::new();
    let mut repeats: BTreeMap<(String, &'static str, String), Vec<NodeId>> = BTreeMap::new();
    for id in all_nodes(doc) {
        let Some(n) = doc.node(id) else { continue };
        if let Some(t) = tag_of(n) {
            tagged.entry(t.to_string()).or_default().push(id);
        } else if matches!(n.kind, NodeKind::Frame | NodeKind::Group | NodeKind::Instance { .. } | NodeKind::Component { .. })
            && !n.children.is_empty()
        {
            let name = fold_name(&n.name);
            if !name.is_empty() {
                repeats.entry((name, n.kind.type_name(), shape(doc, id, 2))).or_default().push(id);
            }
        }
    }
    let frames = |ids: &[NodeId]| -> Vec<Value> {
        let set: BTreeSet<NodeId> = ids.iter().filter_map(|i| frame_and_page(doc, *i).0).collect();
        set.into_iter().take(20).map(|f| label(doc, Some(f))).collect()
    };
    let mut families: Vec<Value> = tagged
        .iter()
        .map(|(f, ids)| json!({"family": f, "count": ids.len(), "ids": ids.iter().take(50).collect::<Vec<_>>(), "frames": frames(ids)}))
        .collect();
    families.truncate(limit);
    let mut suggested: Vec<(usize, Value)> = repeats
        .iter()
        .filter(|(_, ids)| ids.len() >= 2 && ids.iter().filter_map(|i| frame_and_page(doc, *i).0).collect::<BTreeSet<_>>().len() >= 2)
        .map(|((name, ty, _), ids)| {
            (ids.len(), json!({"name": name, "type": ty, "count": ids.len(), "ids": ids.iter().take(50).collect::<Vec<_>>(), "frames": frames(ids)}))
        })
        .collect();
    suggested.sort_by_key(|x| std::cmp::Reverse(x.0));
    let suggested: Vec<Value> = suggested.into_iter().take(limit).map(|(_, v)| v).collect();
    Ok(json!({"families": families, "suggested": suggested}))
}

/// `(from layer, twin layer)` pairs: the two layers, then their children by position while both
/// layers have the same child types.
fn pair(doc: &Document, a: NodeId, b: NodeId, deep: bool, out: &mut Vec<(NodeId, NodeId)>) {
    let (Some(x), Some(y)) = (doc.node(a), doc.node(b)) else { return };
    if x.kind.type_name() != y.kind.type_name() || out.len() >= MAX_PAIRS {
        return;
    }
    out.push((a, b));
    if !deep || x.children.len() != y.children.len() {
        return;
    }
    let same_types = x
        .children
        .iter()
        .zip(&y.children)
        .all(|(c, d)| matches!((doc.node(*c), doc.node(*d)), (Some(c), Some(d)) if c.kind.type_name() == d.kind.type_name()));
    if same_types {
        for (c, d) in x.children.iter().zip(&y.children) {
            pair(doc, *c, *d, deep, out);
        }
    }
}

fn read(s: &mut Session, id: NodeId) -> Result<Map<String, Value>> {
    let get_node = find_command("node.get").ok_or_else(|| EngineError::UnknownCommand("node.get".into()))?;
    let v = (get_node.run)(s, &json!({"id": id}))?;
    Ok(v.get("node").unwrap_or(&v).as_object().cloned().unwrap_or_default())
}

fn propagate(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "family.propagate";
    let from = opt_id(CMD, p, "from")?.ok_or_else(|| bad(CMD, "`from` is required: the layer that was changed"))?;
    let names: Option<Vec<String>> = match get(p, "props") {
        None | Some(Value::Null) => None,
        Some(Value::Array(a)) => Some(
            a.iter()
                .map(|v| v.as_str().map(str::to_string).ok_or_else(|| bad(CMD, "`props` must be a list of property names")))
                .collect::<Result<_>>()?,
        ),
        Some(_) => return Err(bad(CMD, "`props` must be a list of property names")),
    };
    let deep = get(p, "deep").and_then(bool_of).unwrap_or(true);
    let twins: Vec<NodeId> = match get(p, "ids") {
        Some(Value::Array(a)) => {
            let ids: Vec<NodeId> = a.iter().filter_map(crate::params::id_of).collect();
            if ids.len() != a.len() {
                return Err(bad(CMD, "`ids`: every entry must be a node id"));
            }
            ids
        }
        _ => {
            let min = min_score(CMD, p, 0.8)?;
            twins_of(s.doc()?, from, min)?.into_iter().map(|t| t.0).collect()
        }
    };
    let mut pairs = Vec::new();
    {
        let doc = s.doc()?;
        for t in &twins {
            if *t != from {
                pair(doc, from, *t, deep, &mut pairs);
            }
        }
    }
    let set = find_command("node.set").ok_or_else(|| EngineError::UnknownCommand("node.set".into()))?;
    let (mut changed, mut skipped) = (Vec::new(), Vec::new());
    for (a, b) in pairs {
        let (src, dst) = (read(s, a)?, read(s, b)?);
        let mut diff = Map::new();
        for (k, v) in &src {
            if NEVER.contains(&k.as_str()) {
                continue;
            }
            let wanted = match &names {
                Some(list) => list.iter().any(|n| n == k),
                None => !OWN.contains(&k.as_str()),
            };
            if wanted && dst.get(k) != Some(v) {
                diff.insert(k.clone(), v.clone());
            }
        }
        if diff.is_empty() {
            continue;
        }
        match (set.run)(s, &json!({"ids": [b], "props": diff})) {
            Ok(_) => changed.push(json!({"id": b, "props": diff.keys().collect::<Vec<_>>()})),
            Err(_) => {
                // One bad property must not hold back the rest: retry each alone.
                let mut done = Vec::new();
                for (k, v) in &diff {
                    match (set.run)(s, &json!({"ids": [b], "props": {k: v}})) {
                        Ok(_) => done.push(k.clone()),
                        Err(e) => skipped.push(json!({"id": b, "prop": k, "why": e.to_string()})),
                    }
                }
                if !done.is_empty() {
                    changed.push(json!({"id": b, "props": done}));
                }
            }
        }
    }
    Ok(json!({"from": from, "twins": twins.len(), "changed": changed, "skipped": skipped}))
}

#[cfg(test)]
mod tests {
    use figcraft_doc::{NodeId, NodeKind};
    use serde_json::{Value, json};

    use crate::Session;

    /// A "Header" frame holding a "Logo" rectangle, at `x`.
    fn header(s: &mut Session, x: f64, name: &str) -> (NodeId, NodeId) {
        let page = s.page().unwrap();
        let doc = s.doc_mut().unwrap();
        let frame = doc.create(page, NodeKind::Frame, name, x, 0.0, 300.0, 60.0).unwrap();
        let logo = doc.create(frame, NodeKind::Rectangle, "Logo", 10.0, 10.0, 40.0, 40.0).unwrap();
        (frame, logo)
    }

    fn fills(s: &mut Session, id: NodeId) -> Value {
        let n = s.execute("node.get", &json!({"id": id})).unwrap();
        n.get("node").unwrap_or(&n)["fills"].clone()
    }

    fn red(s: &mut Session, id: NodeId) {
        s.execute("node.set", &json!({"ids": [id], "props": {"fills": [{"type": "solid", "color": "#FF0000"}]}})).unwrap();
    }

    #[test]
    fn find_scores_a_copy_high_and_an_unrelated_layer_out() {
        let mut s = Session::new();
        let (a, _) = header(&mut s, 0.0, "Header");
        let (b, _) = header(&mut s, 400.0, "Header 2");
        let page = s.page().unwrap();
        let other = s.doc_mut().unwrap().create(page, NodeKind::Frame, "Footer", 0.0, 900.0, 50.0, 500.0).unwrap();
        let r = s.execute("family.find", &json!({"id": a})).unwrap();
        let ids: Vec<Value> = r["twins"].as_array().unwrap().iter().map(|t| t["id"].clone()).collect();
        assert!(ids.contains(&json!(b)), "{r}");
        assert!(!ids.contains(&json!(other)), "{r}");
        assert!(r["twins"][0]["score"].as_f64().unwrap() > 0.9, "{r}");
    }

    #[test]
    fn a_tag_makes_unlike_layers_twins() {
        let mut s = Session::new();
        let (a, _) = header(&mut s, 0.0, "Header");
        let page = s.page().unwrap();
        let odd = s.doc_mut().unwrap().create(page, NodeKind::Frame, "Top bar", 0.0, 500.0, 90.0, 20.0).unwrap();
        s.execute("family.set", &json!({"ids": [a, odd], "family": "top"})).unwrap();
        let r = s.execute("family.find", &json!({"id": a, "minScore": 1})).unwrap();
        assert_eq!(r["twins"][0]["id"], json!(odd), "{r}");
        assert_eq!(r["family"], json!("top"));
        assert_eq!(s.execute("family.list", &json!({})).unwrap()["families"][0]["count"], json!(2));
    }

    #[test]
    fn propagate_copies_looks_to_the_twin_and_its_children_but_not_names_or_sizes() {
        let mut s = Session::new();
        let (a, logo_a) = header(&mut s, 0.0, "Header");
        let (b, logo_b) = header(&mut s, 400.0, "Header 2");
        s.doc_mut().unwrap().get_mut(b).unwrap().width = 320.0;
        red(&mut s, logo_a);
        assert_ne!(fills(&mut s, logo_a), fills(&mut s, logo_b));
        let r = s.execute("family.propagate", &json!({"from": a})).unwrap();
        assert_eq!(fills(&mut s, logo_b), fills(&mut s, logo_a), "{r}");
        let doc = s.doc().unwrap();
        assert_eq!(doc.node(b).unwrap().name, "Header 2");
        assert_eq!(doc.node(b).unwrap().width, 320.0);
        s.undo().unwrap();
        assert_ne!(fills(&mut s, logo_b), fills(&mut s, logo_a), "one undo step");
    }

    #[test]
    fn junk_parameters_are_errors_not_panics() {
        let mut s = Session::new();
        for cmd in ["family.find", "family.set", "family.list", "family.propagate"] {
            for junk in
                [json!({"id": "x"}), json!({"ids": 3}), json!({"minScore": 7, "from": -1}), json!({"props": 4, "from": 1}), json!({"limit": "a"})]
            {
                let _ = s.execute(cmd, &junk);
            }
        }
    }
}
