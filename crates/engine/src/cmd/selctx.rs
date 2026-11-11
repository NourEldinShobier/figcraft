//! `selection.context`: what an agent needs about the current selection in one call: the layers,
//! their family twins, component and variable bindings, the frame they sit in as an outline of
//! siblings, and the open comments on them.

use std::collections::BTreeSet;

use figcraft_doc::{Document, NodeId, NodeKind};
use serde_json::{Value, json};

use super::CommandSpec;
use super::family::{frame_and_page, label, tag_of, twins_of};
use crate::params::get;
use crate::{Result, Session, bad, spec};

const LAYERS: usize = 20;
const SIBLINGS: usize = 30;
const CONTAINERS: usize = 5;
const TWINS_SAMPLE: usize = 3;
const BINDINGS: usize = 20;
const COMMENTS: usize = 20;
const TWIN_SCORE: f64 = 0.6;

pub fn specs() -> Vec<CommandSpec> {
    vec![spec!(
        "selection.context",
        "Describe the selection",
        "{limit? (layers described, default 20, up to 200), siblings? (siblings listed per frame, default 30, up to 500)}  reads the selection → {page, count, layers: [{id, name, type, width, height, parent, frame, family: {name?, twins, sample: [{id, name, frame}]}, component?: {id, name, props?}, variables: [{field, id, name}]}], containers: [{id, name, width, height, siblings: [{id, name, type, width, height, selected?}], total}], comments: [{id, anchor, author, text, replies}], truncated?}. Read-only; an empty selection gives count 0",
        false,
        context
    )]
}

fn count_arg(cmd: &str, p: &Value, key: &str, default: usize, max: u64) -> Result<usize> {
    match get(p, key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => {
            v.as_u64().filter(|n| *n <= max).map(|n| n as usize).ok_or_else(|| bad(cmd, format!("`{key}` must be an integer from 0 to {max}")))
        }
    }
}

fn r(v: f64) -> f64 {
    if v.is_finite() { (v * 10.0).round() / 10.0 } else { 0.0 }
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max { s.to_string() } else { format!("{}…", s.chars().take(max).collect::<String>()) }
}

fn brief(doc: &Document, id: NodeId) -> Value {
    match doc.node(id) {
        Some(n) => json!({"id": id, "name": n.name, "type": n.kind.type_name(), "width": r(n.width), "height": r(n.height)}),
        None => Value::Null,
    }
}

fn layer(doc: &Document, id: NodeId) -> Option<Value> {
    let n = doc.node(id)?;
    let (frame, _) = frame_and_page(doc, id);
    let mut o = json!({
        "id": id, "name": n.name, "type": n.kind.type_name(), "width": r(n.width), "height": r(n.height),
        "parent": label(doc, n.parent), "frame": label(doc, frame),
    });
    let twins = twins_of(doc, id, TWIN_SCORE).unwrap_or_default();
    let sample: Vec<Value> = twins
        .iter()
        .take(TWINS_SAMPLE)
        .map(|(t, ..)| json!({"id": t, "name": doc.node(*t).map(|k| k.name.clone()), "frame": label(doc, frame_and_page(doc, *t).0)}))
        .collect();
    o["family"] = json!({"name": tag_of(n), "twins": twins.len(), "sample": sample});
    match &n.kind {
        NodeKind::Instance { component, props } => {
            let mut c = json!({"id": component, "name": doc.node(*component).map(|k| k.name.clone())});
            if !props.is_empty() {
                c["props"] = json!(props);
            }
            o["component"] = c;
        }
        NodeKind::Component { .. } => o["component"] = json!({"id": id, "name": n.name, "main": true}),
        _ => {}
    }
    o["variables"] = n
        .bound
        .iter()
        .take(BINDINGS)
        .map(|(field, vid)| json!({"field": field, "id": vid, "name": doc.variables.variables.iter().find(|v| &v.id == vid).map(|v| v.name.clone())}))
        .collect();
    if n.bound.len() > BINDINGS {
        o["variablesTruncated"] = json!(format!("{} of {} bindings listed", BINDINGS, n.bound.len()));
    }
    Some(o)
}

fn context(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "selection.context";
    let limit = count_arg(CMD, p, "limit", LAYERS, 200)?;
    let siblings = count_arg(CMD, p, "siblings", SIBLINGS, 500)?;
    let sel = s.selection();
    let doc = s.doc()?;
    let sel: Vec<NodeId> = sel.into_iter().filter(|i| doc.contains(*i)).collect();
    let page = s.page().ok().and_then(|pg| doc.node(pg)).map(|n| json!({"id": n.id, "name": n.name}));
    if sel.is_empty() {
        return Ok(json!({"page": page, "count": 0, "layers": [], "containers": [], "comments": [], "note": "nothing is selected"}));
    }
    let layers: Vec<Value> = sel.iter().take(limit).filter_map(|i| layer(doc, *i)).collect();
    let chosen: BTreeSet<NodeId> = sel.iter().copied().collect();
    // The parents the selection sits in, in selection order.
    let mut parents: Vec<NodeId> = Vec::new();
    for i in &sel {
        if let Some(pa) = doc.node(*i).and_then(|n| n.parent).filter(|pa| !parents.contains(pa)) {
            parents.push(pa);
        }
    }
    let containers: Vec<Value> = parents
        .iter()
        .take(CONTAINERS)
        .filter_map(|pa| {
            let n = doc.node(*pa)?;
            let items: Vec<Value> = n
                .children
                .iter()
                .take(siblings)
                .map(|c| {
                    let mut b = brief(doc, *c);
                    if chosen.contains(c) {
                        b["selected"] = json!(true);
                    }
                    b
                })
                .collect();
            Some(json!({"id": pa, "name": n.name, "width": r(n.width), "height": r(n.height), "siblings": items, "total": n.children.len()}))
        })
        .collect();
    let on_selection = |anchor: NodeId| chosen.contains(&anchor) || doc.ancestors(anchor).iter().any(|a| chosen.contains(a));
    let open: Vec<&figcraft_doc::Comment> = doc.comments.iter().filter(|c| !c.resolved && c.anchor.is_some_and(on_selection)).collect();
    let comments: Vec<Value> = open
        .iter()
        .take(COMMENTS)
        .map(|c| json!({"id": c.id, "anchor": c.anchor, "author": c.first.author, "text": clip(&c.first.text, 300), "replies": c.replies.len()}))
        .collect();
    let mut out = json!({"page": page, "count": sel.len(), "layers": layers, "containers": containers, "comments": comments});
    let mut cut = Vec::new();
    if sel.len() > limit {
        cut.push(format!("{limit} of {} selected layers described", sel.len()));
    }
    if parents.len() > CONTAINERS {
        cut.push(format!("{CONTAINERS} of {} containers outlined", parents.len()));
    }
    if open.len() > COMMENTS {
        cut.push(format!("{COMMENTS} of {} open comments listed", open.len()));
    }
    if !cut.is_empty() {
        out["truncated"] = json!(cut.join("; "));
    }
    Ok(out)
}
