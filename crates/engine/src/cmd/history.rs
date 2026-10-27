//! The undo stack as data: `history.list`, `history.revertTo` and `history.hash`, so agents can see
//! what each step changed, go back to a point as one operation and verify the document state.

use std::sync::Arc;

use figcraft_doc::{Document, NodeId};
use serde_json::{Value, json};

use super::CommandSpec;
use crate::params::*;
use crate::{EngineError, Result, Session, bad, spec};

const MAX_LIMIT: usize = 200;
/// Changed ids listed per step (the count is always given).
const MAX_IDS: usize = 200;
/// The label `version.restore` records; going back across it would undo a restored version.
const RESTORE_LABEL: &str = "Restore version";

/// A 64-bit FNV-1a hash of the document's serialized content (not cryptographic). Equal documents
/// give equal hashes; an undone edit gives the hash it had before.
pub fn doc_hash(doc: &Document) -> String {
    let bytes = serde_json::to_vec(doc).unwrap_or_default();
    let h = bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3));
    format!("{h:016x}")
}

/// Layers added, removed or edited between two states of a document (pages excluded: they change
/// whenever a top-level layer does).
fn changed_ids(before: &Document, after: &Document) -> Vec<NodeId> {
    let differs = |a: &Document, b: &Document, id: NodeId| match (a.node_arc(id), b.node_arc(id)) {
        (Some(x), Some(y)) => !Arc::ptr_eq(x, y) && **x != **y,
        _ => true,
    };
    let mut out: Vec<NodeId> = after.nodes().filter(|n| !n.is_page() && differs(before, after, n.id)).map(|n| n.id).collect();
    out.extend(before.nodes().filter(|n| !n.is_page() && !after.contains(n.id)).map(|n| n.id));
    out.sort_by_key(|i| i.0);
    out
}

/// A generic "Create layer" says what it created.
fn describe(label: &str, before: &Document, after: &Document, changed: &[NodeId]) -> String {
    if label != "Create layer" || changed.is_empty() || changed.iter().any(|i| before.contains(*i)) {
        return label.to_string();
    }
    match changed {
        [one] => after.node(*one).map_or_else(|| label.to_string(), |n| format!("Create layer \"{}\"", n.name)),
        many => format!("Create {} layers", many.len()),
    }
}

fn limit_arg(cmd: &str, p: &Value) -> Result<usize> {
    match get(p, "limit") {
        None | Some(Value::Null) => Ok(20),
        Some(v) => f64_of(v).filter(|n| (1.0..=MAX_LIMIT as f64).contains(n)).map(|n| n as usize).ok_or_else(|| bad(cmd, "`limit` must be 1 to 200")),
    }
}

/// Kind and label of timeline entry `i` (undo steps first, then redo steps, next redo first).
fn timeline(s: &Session) -> Result<(usize, Vec<(String, &'static str)>)> {
    if s.live().doc_uid.is_some() {
        return Err(EngineError::Disabled("history".into(), "not available in a live session".into()));
    }
    let h = &s.ds()?.history;
    let kind = |l: &str, k: &'static str| if l == RESTORE_LABEL { "restore" } else { k };
    let entries = h.undo.iter().map(|e| (e.label.clone(), kind(&e.label, e.kind))).chain(h.redo.iter().rev().map(|e| (e.label.clone(), kind(&e.label, e.kind))));
    Ok((h.undo.len(), entries.collect()))
}

fn list(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "history.list";
    let limit = limit_arg(CMD, p)?;
    let (applied, kinds) = timeline(s)?;
    let ds = s.ds()?;
    let h = &ds.history;
    // The document with `k` steps applied: before undo step k, the document itself, or after redo step k - applied.
    let state = |k: usize| -> Option<&Arc<Document>> {
        match k.cmp(&applied) {
            std::cmp::Ordering::Less => h.undo.get(k).map(|e| &e.doc),
            std::cmp::Ordering::Equal => Some(&ds.doc),
            std::cmp::Ordering::Greater => h.redo.get(h.redo.len().checked_sub(k - applied)?).map(|e| &e.doc),
        }
    };
    let start = kinds.len().saturating_sub(limit);
    let mut entries = Vec::new();
    for (i, (label, kind)) in kinds.iter().enumerate().skip(start) {
        let (Some(pre), Some(post)) = (state(i), state(i + 1)) else { continue };
        let changed = changed_ids(pre, post);
        let step = if i < applied { h.undo.get(i) } else { h.redo.len().checked_sub(i - applied + 1).and_then(|j| h.redo.get(j)) };
        let time = step.map_or(0, |e| e.time);
        entries.push(json!({
            "index": i,
            "label": describe(label, pre, post, &changed),
            "changedIds": changed.iter().take(MAX_IDS).collect::<Vec<_>>(),
            "changedCount": changed.len(),
            "time": time,
            "kind": kind,
            "applied": i < applied,
        }));
    }
    Ok(json!({"entries": entries, "applied": applied, "total": kinds.len(), "hash": doc_hash(&ds.doc)}))
}

fn revert_to(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "history.revertTo";
    let (applied, kinds) = timeline(s)?;
    let target = get(p, "index")
        .and_then(f64_of)
        .filter(|n| *n >= 0.0 && *n <= kinds.len() as f64 && n.fract() == 0.0)
        .map(|n| n as usize)
        .ok_or_else(|| bad(CMD, format!("`index` must be a whole number from 0 to {} (the state just before that history entry; {} is the latest)", kinds.len(), kinds.len())))?;
    let (lo, hi) = (target.min(applied), target.max(applied));
    if kinds.get(lo..hi).is_some_and(|k| k.iter().any(|(_, kind)| *kind == "restore")) {
        return Err(bad(CMD, "the range crosses a version restore; use version.restore to go back further"));
    }
    let before = doc_hash(&s.ds()?.doc);
    for _ in target..applied {
        if !s.undo()? {
            break;
        }
    }
    for _ in applied..target {
        if !s.redo()? {
            break;
        }
    }
    let ds = s.ds()?;
    Ok(json!({"applied": ds.history.undo.len(), "total": kinds.len(), "hash": doc_hash(&ds.doc), "previousHash": before}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        spec!(
            "history.list",
            "List history",
            "{limit? (1-200; default 20, the newest)} → {entries: [{index, label, changedIds, changedCount, time (unix seconds), kind (edit|batch|restore), applied}], applied, total, hash}  the active document's undo steps oldest first, then the redo steps; `index` n is the state just before that entry",
            false,
            list
        ),
        spec!(
            "history.revertTo",
            "Revert to history point",
            "{index} → {applied, total, hash, previousHash}  undo or redo until the state is just before entry `index` of history.list (`total` is the latest state); refused when it crosses a version restore",
            false,
            revert_to
        ),
        spec!("history.hash", "Document hash", "{} → {hash}  a 64-bit content hash of the active document: equal documents have equal hashes", false, |s, _| {
            Ok(json!({"hash": doc_hash(&s.ds()?.doc)}))
        }),
    ]
}
