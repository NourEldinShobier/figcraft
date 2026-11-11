//! Bulk-edit safety for agents. Four pieces share this file:
//! - the count and the compact `changed` summary of a call ([`affected`], [`summary`]), which the
//!   `maxAffected` guard and the `withChanged` option of [`crate::Session::execute`] use (see
//!   `session/generic.rs`);
//! - the scope fence (`scope.set`, `scope.clear`, `scope.get`): while one is set, an agent's edits
//!   outside the layers and pages it names are refused;
//! - layer locks by owner (`lock.set`, `lock.release`, `lock.list`): the layer's `locked` flag plus
//!   the owner's label in plugin data. An agent edits a layer locked by someone else only by
//!   passing that owner's label as `as`;
//! - [`Session::check_safety`], the fence and lock check every command of an agent runs through.
//!
//! The fence and the locks hold against AI agents (calls made through `Session::execute_as`), not
//! against the person at the keyboard. A fence is session state of one file tab, like a saved list:
//! it is not saved in the file. Locks live in the file (the flag and a `figcraft.lockOwner` entry).

use std::collections::BTreeSet;
use std::sync::Arc;

use figcraft_doc::{Document, Node, NodeId};
use serde_json::{Value, json};

use super::CommandSpec;
use crate::params::{get, id_of, req_targets, string};
use crate::{EngineError, Result, Session, bad, find_command, spec};

/// The plugin-data key holding the owner label of a locked layer.
pub const OWNER_KEY: &str = "figcraft.lockOwner";
/// Most layers one fence or one lock call names.
const MAX_IDS: usize = 10_000;
/// Longest owner label, in characters.
const MAX_LABEL: usize = 100;
/// Layer ids a summary, a refusal or a list shows.
const SAMPLE: usize = 10;

/// The layers and pages an agent may edit while a fence is set.
#[derive(Clone, Debug, Default)]
pub(crate) struct Fence {
    pub ids: BTreeSet<NodeId>,
    pub pages: BTreeSet<NodeId>,
}

impl Fence {
    /// Whether layer `id` of `doc` is a fenced layer, inside one or on a fenced page.
    fn holds(&self, doc: &Document, id: NodeId) -> bool {
        self.ids.contains(&id)
            || self.pages.contains(&id)
            || doc.page_of(id).is_some_and(|p| self.pages.contains(&p))
            || doc.ancestors(id).iter().any(|a| self.ids.contains(a))
    }

    fn describe(&self, doc: &Document) -> String {
        let ids: Vec<String> = self.ids.iter().take(SAMPLE).map(|i| i.0.to_string()).collect();
        let more = self.ids.len().saturating_sub(SAMPLE);
        let mut parts = Vec::new();
        if !ids.is_empty() {
            parts.push(format!("layers {}{}", ids.join(", "), if more > 0 { format!(" and {more} more") } else { String::new() }));
        }
        let pages: Vec<String> =
            self.pages.iter().take(SAMPLE).map(|p| doc.node(*p).map_or_else(|| p.0.to_string(), |n| format!("\"{}\" ({})", n.name, p.0))).collect();
        if !pages.is_empty() {
            parts.push(format!("pages {}", pages.join(", ")));
        }
        parts.join("; ")
    }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        spec!(
            "scope.set",
            "Fence agent edits to layers or pages",
            "{ids? (layers: they and everything inside them), pages? (page ids or names)}  from now on an AI agent's edits to layers outside the fence are refused, naming it (also a batch or script that touches one is rolled back whole). Session state of this file tab, not saved; replaces any earlier fence; the person's own edits are not fenced. Changes to a container's child list (adding or removing a fenced layer) are allowed → {ids, pages}",
            false,
            scope_set
        ),
        spec!("scope.clear", "Lift the agent scope fence", "{}  → {cleared}", false, scope_clear),
        spec!("scope.get", "Read the agent scope fence", "{}  → {active, ids, pages: [{id, name}]}", false, scope_get),
        spec!(
            "lock.set",
            "Lock layers for an owner",
            "{ids? (default: the selection), owner? (a label; default: the `as` of the call)}  locks the layers (the `locked` flag plus the owner's label in plugin data). An AI agent's edit to a layer locked by another label is refused, naming the owner, unless it passes that label as `as`. A layer another owner holds is refused → {owner, locked: [ids]}",
            true,
            lock_set
        ),
        spec!(
            "lock.release",
            "Release layer locks",
            "{ids? (default: the selection), owner? (the releasing label, default the `as` of the call; without `ids` it also names the layers: all that owner holds), force? (the person only: release another owner's lock)}  unlocks the layers the owner holds → {released: [ids], skipped: [{id, owner}], notLocked}",
            true,
            lock_release
        ),
        spec!(
            "lock.list",
            "List owned layer locks",
            "{owner? (only this owner's), limit? (default 100, up to 5000)}  → {total, owners: {label: count}, locks: [{id, name, owner}], truncated?}",
            false,
            lock_list
        ),
    ]
}

/// A layer's lock owner: it holds while the layer is locked and carries a non-empty label.
fn lock_owner(n: &Node) -> Option<&str> {
    n.locked.then(|| n.plugin_data.get(OWNER_KEY).map(String::as_str)).flatten().filter(|o| !o.is_empty())
}

fn label(n: &Node) -> String {
    format!("layer {} \"{}\"", n.id.0, n.name)
}

fn owner_label(cmd: &str, v: &str) -> Result<String> {
    let v = v.trim();
    if v.is_empty() || v.chars().count() > MAX_LABEL {
        return Err(bad(cmd, format!("an owner label is 1 to {MAX_LABEL} characters")));
    }
    Ok(v.to_string())
}

// ---------- what a call changed ----------

/// The layers a call changed, created and deleted.
#[derive(Default)]
pub(crate) struct Affected {
    pub changed: Vec<NodeId>,
    pub created: Vec<NodeId>,
    pub deleted: Vec<NodeId>,
}

impl Affected {
    pub fn count(&self) -> usize {
        self.changed.len() + self.created.len() + self.deleted.len()
    }

    /// Every affected id, ascending.
    pub fn all(&self) -> Vec<NodeId> {
        let mut all: Vec<NodeId> = self.changed.iter().chain(&self.created).chain(&self.deleted).copied().collect();
        all.sort();
        all
    }
}

/// A container whose only change is that layers came or went from its child list: not an edit of
/// the container itself (a new layer changes its parent too).
fn child_list_only(before: &Node, after: &Node) -> bool {
    if before.children == after.children {
        return false;
    }
    let (a, b): (BTreeSet<_>, BTreeSet<_>) = (before.children.iter().collect(), after.children.iter().collect());
    if a == b {
        return false;
    }
    let mut moved = before.clone();
    moved.children = after.children.clone();
    moved == *after
}

/// What differs between two versions of a document, without containers that only gained or lost
/// children.
pub(crate) fn affected(before: &Document, after: &Document) -> Affected {
    let mut out = Affected::default();
    for id in after.changed_ids(before, usize::MAX).unwrap_or_default() {
        match (before.node(id), after.node(id)) {
            (None, _) => out.created.push(id),
            (Some(_), None) => out.deleted.push(id),
            (Some(a), Some(b)) => {
                if !child_list_only(a, b) {
                    out.changed.push(id);
                }
            }
        }
    }
    out
}

/// The top-level fields (file-format names) that differ on up to 50 of the changed layers.
fn touched_props(before: &Document, after: &Document, ids: &[NodeId]) -> Vec<String> {
    let mut names = BTreeSet::new();
    for id in ids.iter().take(50) {
        let (Some(a), Some(b)) = (before.node(*id), after.node(*id)) else { continue };
        let (Ok(Value::Object(x)), Ok(Value::Object(y))) = (serde_json::to_value(a), serde_json::to_value(b)) else { continue };
        names.extend(x.keys().chain(y.keys()).filter(|k| x.get(*k) != y.get(*k)).cloned());
    }
    names.into_iter().take(30).collect()
}

/// The compact `changed` summary of one call: `{count, ids (up to 10), more?, props?, created?,
/// deleted?, noop?}`. `noop` counts the layers the call named (`targets`) that it left as they
/// were, for a single command (a batch or script has no one target list) that made no layers.
pub(crate) fn summary(before: &Document, after: &Document, targets: &[NodeId], single: bool) -> Value {
    let a = affected(before, after);
    let all = a.all();
    let mut out = json!({"count": all.len(), "ids": all.iter().take(SAMPLE).collect::<Vec<_>>()});
    if all.len() > SAMPLE {
        out["more"] = json!(all.len() - SAMPLE);
    }
    let props = touched_props(before, after, &a.changed);
    if !props.is_empty() {
        out["props"] = json!(props);
    }
    if !a.created.is_empty() {
        out["created"] = json!(a.created.len());
    }
    if !a.deleted.is_empty() {
        out["deleted"] = json!(a.deleted.len());
    }
    if single && a.created.is_empty() && a.deleted.is_empty() {
        let hit: BTreeSet<NodeId> = all.iter().copied().collect();
        let noop = targets.iter().filter(|t| !hit.contains(t)).count();
        if noop > 0 {
            out["noop"] = json!(noop);
        }
    }
    out
}

/// Whether running command `id` with `params` edits the document: an undoable command, a script,
/// or a batch that is not a preview.
pub(crate) fn mutates(id: &str, params: &Value) -> bool {
    match id {
        "script.run" => true,
        "command.batch" => params.get("preview").and_then(Value::as_bool) != Some(true),
        _ => find_command(id).is_some_and(|s| s.history),
    }
}

/// The refusal of a call that would change more than `max` layers.
pub(crate) fn over_limit(id: &str, a: &Affected, max: usize) -> EngineError {
    let all = a.all();
    let sample: Vec<String> = all.iter().take(SAMPLE).map(|i| i.0.to_string()).collect();
    EngineError::Other(format!(
        "refused: {id} would change {} layers, more than maxAffected {max} (sample ids: {}{}). Nothing was changed. Narrow the target (ids, where) or pass confirm: true to apply it anyway; propagate_change takes a larger maxAffected instead",
        all.len(),
        sample.join(", "),
        if all.len() > SAMPLE { ", ..." } else { "" }
    ))
}

// ---------- the fence and lock check ----------

impl Session {
    /// Refuse (the caller rolls the edit back) a command of an AI agent that changed a layer locked
    /// by another owner than the call's `as` label, or, while a fence is set, a layer outside it.
    /// The person's own commands pass; so do the lock commands themselves for the lock part (they
    /// check owners on their own).
    pub(crate) fn check_safety(&mut self, id: &str, before: &Option<(Arc<Document>, Vec<NodeId>, NodeId, u64)>) -> Result<()> {
        if self.agent.is_none() {
            return Ok(());
        }
        let Some((bdoc, _, _, uid)) = before else { return Ok(()) };
        let Some(ds) = self.docs.iter().find(|d| d.uid == *uid) else { return Ok(()) };
        if Arc::ptr_eq(&ds.doc, bdoc) {
            return Ok(());
        }
        let after = &*ds.doc;
        let changed = after.changed_ids(bdoc, usize::MAX).unwrap_or_default();
        if !id.starts_with("lock.") {
            let held: Vec<(&Node, &str)> =
                changed.iter().filter_map(|c| bdoc.node(*c)).filter_map(|n| lock_owner(n).filter(|o| *o != self.actor).map(|o| (n, o))).collect();
            if let Some((n, owner)) = held.first() {
                let more = if held.len() > 1 { format!(" (and {} more locked layers)", held.len() - 1) } else { String::new() };
                return Err(EngineError::Other(format!(
                    "refused: {id} would change {}, locked by \"{owner}\"{more}. Pass `as: \"{owner}\"` if you are that owner, or leave the layer alone; lock.list shows the locks",
                    label(n)
                )));
            }
        }
        let Some(fence) = &ds.fence else { return Ok(()) };
        // Layers that hold a fenced layer may gain or lose children.
        let mut holders: Option<BTreeSet<NodeId>> = None;
        let mut outside = Vec::new();
        for c in &changed {
            let (b, a) = (bdoc.node(*c), after.node(*c));
            if b.is_none_or(|_| fence.holds(bdoc, *c)) && a.is_none_or(|_| fence.holds(after, *c)) {
                continue;
            }
            if let (Some(b), Some(a)) = (b, a)
                && child_list_only(b, a)
            {
                let held = holders.get_or_insert_with(|| fence.ids.iter().flat_map(|f| after.ancestors(*f)).collect());
                if held.contains(c) {
                    continue;
                }
            }
            outside.push(*c);
        }
        let Some(first) = outside.first() else { return Ok(()) };
        let name = after.node(*first).or_else(|| bdoc.node(*first)).map_or_else(|| first.0.to_string(), label);
        let more = if outside.len() > 1 { format!(" (and {} more)", outside.len() - 1) } else { String::new() };
        Err(EngineError::Other(format!(
            "refused: {id} would change {name}{more}, outside the scope fence ({}). Keep to the fenced layers, or ask the person to lift it (scope.clear)",
            fence.describe(after)
        )))
    }
}

// ---------- scope.* ----------

fn id_list(cmd: &str, p: &Value) -> Result<Option<Vec<NodeId>>> {
    let items = match (get(p, "ids"), get(p, "id")) {
        (Some(Value::Array(a)), _) => a.clone(),
        (Some(v), _) => return Err(bad(cmd, format!("`ids` must be an array of layer ids (got {v})"))),
        (None, Some(v)) => vec![v.clone()],
        (None, None) => return Ok(None),
    };
    if items.len() > MAX_IDS {
        return Err(bad(cmd, format!("at most {MAX_IDS} layers")));
    }
    items.iter().map(|v| id_of(v).ok_or_else(|| bad(cmd, format!("every id must be a layer id (got {v})")))).collect::<Result<Vec<_>>>().map(Some)
}

fn scope_set(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "scope.set";
    let ids = id_list(CMD, p)?;
    let pages = match get(p, "pages") {
        None => None,
        Some(Value::Array(a)) if a.len() <= MAX_IDS => Some(a.clone()),
        Some(v @ (Value::String(_) | Value::Number(_))) => Some(vec![v.clone()]),
        Some(v) => return Err(bad(CMD, format!("`pages` must be a list of page ids or names (got {v})"))),
    };
    if ids.is_none() && pages.is_none() {
        return Err(bad(CMD, "give `ids` (layers) or `pages` to fence the agent to"));
    }
    let doc = s.doc()?;
    let mut fence = Fence::default();
    for id in ids.into_iter().flatten() {
        if !doc.contains(id) {
            return Err(EngineError::NoNode(id));
        }
        fence.ids.insert(id);
    }
    for v in pages.into_iter().flatten() {
        let by_id = id_of(&v).filter(|i| doc.pages.contains(i));
        let by_name =
            v.as_str().and_then(|name| doc.pages.iter().copied().find(|p| doc.node(*p).is_some_and(|n| n.name.eq_ignore_ascii_case(name.trim()))));
        let Some(page) = by_id.or(by_name) else { return Err(bad(CMD, format!("no page {v} (a page id or name)"))) };
        fence.pages.insert(page);
    }
    let out = json!({"ids": fence.ids, "pages": fence.pages});
    s.ds_mut()?.fence = Some(fence);
    Ok(out)
}

fn scope_clear(s: &mut Session, _p: &Value) -> Result<Value> {
    Ok(json!({"cleared": s.ds_mut()?.fence.take().is_some()}))
}

fn scope_get(s: &mut Session, _p: &Value) -> Result<Value> {
    let ds = s.ds()?;
    let Some(f) = &ds.fence else { return Ok(json!({"active": false, "ids": [], "pages": []})) };
    let pages: Vec<Value> = f.pages.iter().map(|p| json!({"id": p, "name": ds.doc.node(*p).map(|n| n.name.clone())})).collect();
    Ok(json!({"active": true, "ids": f.ids, "pages": pages}))
}

// ---------- lock.* ----------

fn lock_set(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "lock.set";
    let owner = match string(p, "owner") {
        Some(o) => owner_label(CMD, &o)?,
        None if !s.actor.is_empty() => s.actor.clone(),
        None => return Err(bad(CMD, "give `owner` (a label), or the call's `as`")),
    };
    let ids = req_targets(CMD, s, p)?;
    if ids.len() > MAX_IDS {
        return Err(bad(CMD, format!("at most {MAX_IDS} layers")));
    }
    let doc = s.doc()?;
    let held: Vec<String> = ids
        .iter()
        .filter_map(|i| doc.node(*i))
        .filter_map(|n| lock_owner(n).filter(|o| *o != owner).map(|o| format!("{} is locked by \"{o}\"", label(n))))
        .collect();
    if let Some(first) = held.first() {
        return Err(EngineError::Other(format!(
            "refused: {first}{}",
            if held.len() > 1 { format!(" (and {} more)", held.len() - 1) } else { String::new() }
        )));
    }
    run_node_set(s, &ids, json!({"locked": true, "pluginData": {OWNER_KEY: owner}}))?;
    Ok(json!({"owner": owner, "locked": ids}))
}

fn lock_release(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "lock.release";
    let owner = match string(p, "owner") {
        Some(o) => Some(owner_label(CMD, &o)?),
        None => Some(s.actor.clone()).filter(|a| !a.is_empty()),
    };
    let by_owner = get(p, "ids").is_none() && get(p, "id").is_none() && string(p, "owner").is_some();
    let force = match get(p, "force") {
        None => false,
        Some(Value::Bool(b)) => *b,
        Some(v) => return Err(bad(CMD, format!("`force` must be a boolean (got {v})"))),
    };
    if force && s.is_agent_call() {
        return Err(bad(CMD, "`force` is for the person: an agent releases only the locks of its own label"));
    }
    let ids: Vec<NodeId> = match (by_owner, &owner) {
        (true, Some(o)) => s.doc()?.nodes().filter(|n| lock_owner(n) == Some(o.as_str())).map(|n| n.id).take(MAX_IDS).collect(),
        _ => req_targets(CMD, s, p)?,
    };
    if ids.len() > MAX_IDS {
        return Err(bad(CMD, format!("at most {MAX_IDS} layers")));
    }
    let doc = s.doc()?;
    let (mut free, mut skipped, mut not_locked) = (Vec::new(), Vec::new(), 0usize);
    for id in &ids {
        let Some(n) = doc.node(*id) else { return Err(EngineError::NoNode(*id)) };
        match lock_owner(n) {
            None => not_locked += 1,
            Some(o) if force || owner.as_deref() == Some(o) => free.push(*id),
            Some(o) => skipped.push(json!({"id": id, "owner": o})),
        }
    }
    if !free.is_empty() {
        run_node_set(s, &free, json!({"locked": false, "pluginData": {OWNER_KEY: null}}))?;
    }
    Ok(json!({"released": free, "skipped": skipped, "notLocked": not_locked}))
}

fn run_node_set(s: &mut Session, ids: &[NodeId], props: Value) -> Result<()> {
    let set = find_command("node.set").ok_or_else(|| EngineError::UnknownCommand("node.set".into()))?;
    (set.run)(s, &json!({"ids": ids, "props": props})).map(|_| ())
}

fn lock_list(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "lock.list";
    let limit = match get(p, "limit") {
        None => 100,
        Some(v) => v.as_u64().filter(|n| (1..=5000).contains(n)).ok_or_else(|| bad(CMD, "`limit` must be an integer from 1 to 5000"))? as usize,
    };
    let only = string(p, "owner");
    let doc = s.doc()?;
    let mut owners: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let (mut total, mut locks) = (0usize, Vec::new());
    for n in doc.nodes() {
        let Some(o) = lock_owner(n) else { continue };
        if only.as_deref().is_some_and(|w| w != o) {
            continue;
        }
        *owners.entry(o.to_string()).or_default() += 1;
        total += 1;
        if locks.len() < limit {
            locks.push(json!({"id": n.id, "name": n.name, "owner": o}));
        }
    }
    let mut out = json!({"total": total, "owners": owners, "locks": locks});
    if total > limit {
        out["truncated"] = json!(true);
    }
    Ok(out)
}
