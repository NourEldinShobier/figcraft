//! The change journal (`changeset.begin`, `changeset.end`, `changeset.list`, `changeset.revert`):
//! an agent names a logical change, does its edits, and can later undo just that change even when
//! other changes came after it, as long as they touched different layers.
//!
//! A changeset keeps the document as it was at `begin` and at `end`. Its layers are those that
//! differ between the two ([`safety::affected`]). `revert` refuses when a later change touched one
//! of them (naming the layers) and otherwise puts those layers back from the `begin` snapshot, as
//! one undo step of its own. Only layers are reverted: styles, variables and comments a changeset
//! changed stay as they are (the result says so). The journal is session state of the file tab,
//! not saved in the file.

use std::collections::BTreeSet;
use std::sync::Arc;

use figcraft_doc::{Document, NodeId};
use serde_json::{Value, json};

use super::CommandSpec;
use super::safety;
use crate::params::{f64_of, get, id_of, string};
use crate::{Result, Session, bad, spec};

/// Longest changeset label, in characters.
const MAX_LABEL: usize = 100;
/// Changesets a tab keeps; the oldest go first.
const MAX_KEPT: usize = 50;
/// Layers named in a refusal.
const SHOWN: usize = 8;

/// The journal of one file tab.
#[derive(Clone, Debug, Default)]
pub(crate) struct Journal {
    next: u64,
    open: Option<Open>,
    done: Vec<Done>,
}

impl Journal {
    /// The label of the changeset being recorded, if any.
    pub(crate) fn open_label(&self) -> Option<&str> {
        self.open.as_ref().map(|o| o.label.as_str())
    }
}

#[derive(Clone, Debug)]
struct Open {
    id: u64,
    label: String,
    before: Arc<Document>,
    undo_at: usize,
}

#[derive(Clone, Debug)]
struct Done {
    id: u64,
    label: String,
    before: Arc<Document>,
    after: Arc<Document>,
    steps: usize,
    time: u64,
    created: Vec<NodeId>,
    changed: Vec<NodeId>,
    deleted: Vec<NodeId>,
}

impl Done {
    fn layers(&self) -> usize {
        self.created.len() + self.changed.len() + self.deleted.len()
    }

    fn touched(&self) -> BTreeSet<NodeId> {
        self.created.iter().chain(&self.changed).chain(&self.deleted).copied().collect()
    }

    fn item(&self) -> Value {
        json!({
            "id": self.id,
            "label": self.label,
            "steps": self.steps,
            "layers": self.layers(),
            "created": self.created.len(),
            "changed": self.changed.len(),
            "deleted": self.deleted.len(),
            "time": self.time,
        })
    }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        spec!(
            "changeset.begin",
            "Start a changeset",
            "{label (what this change does, 1 to 100 characters)}  from now on the edits to this file belong to one named logical change, until changeset.end. One changeset is open at a time; while it is, the `changed` summary of mutating calls carries its label as `changeset` → {id, label}",
            false,
            begin
        ),
        spec!(
            "changeset.end",
            "End the open changeset",
            "{}  closes the open changeset and keeps it in the journal (up to 50 per file tab, session only) → {id, label, steps (undo steps made inside), layers, created, changed, deleted, time}",
            false,
            end
        ),
        spec!(
            "changeset.list",
            "List changesets",
            "{limit? (default 20, up to 100)}  the open changeset and the finished ones, newest first → {open: {id, label, layers} | null, total, changesets: [{id, label, steps, layers, created, changed, deleted, time, conflicts?}], truncated?}. `conflicts` counts the layers of that changeset that later edits touched too: changeset.revert refuses it while there are any",
            false,
            list
        ),
        spec!(
            "changeset.revert",
            "Revert a changeset",
            "{id (a changeset id, changeset.list)}  puts the layers the changeset changed back as they were before it (edits it made are undone, layers it deleted come back, layers it created go), keeping every later change, as one undo step. Refused, naming the layers, when a later change touched one of them (revert the later change first, or restore a checkpoint with restore_version). Layers only: styles and variables the changeset changed stay → {id, label, reverted, removed, restored, changed, note?, noop?}",
            true,
            revert
        ),
    ]
}

fn begin(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "changeset.begin";
    let label = string(p, "label").map(|l| l.trim().to_string()).filter(|l| !l.is_empty());
    let Some(label) = label.filter(|l| l.chars().count() <= MAX_LABEL) else {
        return Err(bad(CMD, format!("`label` must say what the change does (1 to {MAX_LABEL} characters)")));
    };
    let undo_at = s.undo_depth().0;
    let ds = s.ds_mut()?;
    if let Some(o) = &ds.changesets.open {
        return Err(bad(CMD, format!("changeset {} \"{}\" is still open: call changeset.end first", o.id, o.label)));
    }
    ds.changesets.next += 1;
    let id = ds.changesets.next;
    ds.changesets.open = Some(Open { id, label: label.clone(), before: ds.doc.clone(), undo_at });
    Ok(json!({"id": id, "label": label}))
}

fn end(s: &mut Session, _p: &Value) -> Result<Value> {
    let undo_now = s.undo_depth().0;
    let ds = s.ds_mut()?;
    let Some(o) = ds.changesets.open.take() else {
        return Err(bad("changeset.end", "no changeset is open: changeset.begin starts one"));
    };
    let after = ds.doc.clone();
    let a = safety::affected(&o.before, &after);
    let done = Done {
        id: o.id,
        label: o.label,
        before: o.before,
        after,
        steps: undo_now.saturating_sub(o.undo_at),
        time: super::comment::now_secs(),
        created: a.created,
        changed: a.changed,
        deleted: a.deleted,
    };
    let out = done.item();
    ds.changesets.done.push(done);
    if ds.changesets.done.len() > MAX_KEPT {
        ds.changesets.done.remove(0);
    }
    Ok(out)
}

fn list(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "changeset.list";
    let limit = match get(p, "limit") {
        None => 20,
        Some(v) => f64_of(v)
            .filter(|n| (1.0..=100.0).contains(n) && n.fract() == 0.0)
            .map(|n| n as usize)
            .ok_or_else(|| bad(CMD, "`limit` must be a whole number from 1 to 100"))?,
    };
    let ds = s.ds()?;
    let open = ds.changesets.open.as_ref().map(|o| json!({"id": o.id, "label": o.label, "layers": safety::affected(&o.before, &ds.doc).count()}));
    let items: Vec<Value> = ds
        .changesets
        .done
        .iter()
        .rev()
        .take(limit)
        .map(|d| {
            let mut v = d.item();
            let conflicts = conflicts(d, &ds.doc).len();
            if conflicts > 0 {
                v["conflicts"] = json!(conflicts);
            }
            v
        })
        .collect();
    let total = ds.changesets.done.len();
    let mut out = json!({"open": open, "total": total, "changesets": items});
    if total > limit {
        out["truncated"] = json!(true);
    }
    Ok(out)
}

/// The layers of `c` that a later change (anything since it ended) touched too, and layers whose
/// revert would lose later work: a layer made later inside one the changeset created, or a parent
/// to restore into that is gone. Ascending.
fn conflicts(c: &Done, cur: &Document) -> BTreeSet<NodeId> {
    let touched = c.touched();
    let mut out: BTreeSet<NodeId> = safety::affected(&c.after, cur).all().into_iter().filter(|i| touched.contains(i)).collect();
    for x in &c.created {
        out.extend(cur.subtree(*x).into_iter().filter(|n| !touched.contains(n)));
    }
    let deleted: BTreeSet<NodeId> = c.deleted.iter().copied().collect();
    for id in c.deleted.iter().chain(&c.changed) {
        if let Some(p) = c.before.node(*id).and_then(|n| n.parent)
            && !cur.contains(p)
            && !deleted.contains(&p)
        {
            out.insert(*id);
        }
    }
    out
}

fn name_of(c: &Done, cur: &Document, id: NodeId) -> String {
    let name = cur.node(id).or_else(|| c.after.node(id)).or_else(|| c.before.node(id)).map_or("?", |n| n.name.as_str());
    format!("\"{name}\" (#{})", id.0)
}

fn revert(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "changeset.revert";
    let Some(id) = get(p, "id").and_then(id_of).map(|n| n.0) else {
        return Err(bad(CMD, "`id` must be a changeset id (changeset.list shows them)"));
    };
    let ds = s.ds()?;
    if ds.changesets.open.as_ref().is_some_and(|o| o.id == id) {
        return Err(bad(CMD, format!("changeset {id} is still open: call changeset.end first")));
    }
    let Some(c) = ds.changesets.done.iter().find(|d| d.id == id).cloned() else {
        return Err(bad(CMD, format!("no changeset {id} in this file (changeset.list shows them; they last for the session)")));
    };
    let cur = ds.doc.clone();
    // Reverted already (or undone back to before it): the layers are as they were.
    if c.created.iter().all(|i| !cur.contains(*i)) && c.deleted.iter().chain(&c.changed).all(|i| cur.node(*i) == c.before.node(*i)) {
        return Ok(json!({"id": c.id, "label": c.label, "reverted": 0, "removed": 0, "restored": 0, "changed": 0, "noop": true}));
    }
    let blocked = conflicts(&c, &cur);
    if !blocked.is_empty() {
        let names: Vec<String> = blocked.iter().take(SHOWN).map(|i| name_of(&c, &cur, *i)).collect();
        let more = if blocked.len() > SHOWN { format!(" and {} more", blocked.len() - SHOWN) } else { String::new() };
        return Err(bad(
            CMD,
            format!(
                "refused: changeset {id} \"{}\" overlaps later changes to {}{more}. Nothing was changed. Revert or undo the later changes first, or go back to a checkpoint taken before it with restore_version (checkpoint, list_versions)",
                c.label,
                names.join(", ")
            ),
        ));
    }
    let mut d: Document = (*cur).clone();
    apply(&c, &mut d)?;
    let mut out = json!({
        "id": c.id,
        "label": c.label,
        "reverted": c.layers(),
        "removed": c.created.len(),
        "restored": c.deleted.len(),
        "changed": c.changed.len(),
    });
    if d.changed_ids(&cur, 1).is_some_and(|v| v.is_empty()) && d.pages == cur.pages {
        out["noop"] = json!(true);
        out["reverted"] = json!(0);
        return Ok(out);
    }
    if c.before.styles != c.after.styles || c.before.variables != c.after.variables {
        out["note"] = json!("the changeset also changed styles or variables; those stay as they are (restore_version brings a checkpoint back)");
    }
    *s.doc_mut()? = d;
    Ok(out)
}

/// Put the layers of `c` back in `d` (the document now), restoring before removing so that a layer
/// moved into one the changeset created leaves it first.
fn apply(c: &Done, d: &mut Document) -> Result<()> {
    let before = &*c.before;
    // 1. Layers the changeset deleted come back, parents first, each at its old position.
    let mut deleted = c.deleted.clone();
    deleted.sort_by_key(|id| {
        let at = before.node(*id).and_then(|n| n.parent).and_then(|p| before.node(p)).and_then(|p| p.children.iter().position(|ch| ch == id));
        (before.ancestors(*id).len(), at.unwrap_or(0))
    });
    for id in deleted {
        let Some(b) = before.node(id) else { continue };
        let mut n = b.clone();
        // Its children come back by their own turn.
        n.children.clear();
        match b.parent.filter(|_| !b.is_page()) {
            Some(parent) => {
                let at = before.node(parent).and_then(|p| p.children.iter().position(|ch| *ch == id));
                d.insert(n, parent, at)?;
            }
            None => {
                let at = before.pages.iter().position(|pg| *pg == id).unwrap_or(d.pages.len()).min(d.pages.len());
                d.put_node(n);
                d.pages.insert(at, id);
                d.mark_dirty([id]);
            }
        }
    }
    // 2. Layers it changed get their old properties and place back, keeping children it did not
    //    know about (made or moved in by later changes).
    let mut changed = c.changed.clone();
    changed.sort_by_key(|id| before.ancestors(*id).len());
    for id in changed {
        let (Some(b), Some(now)) = (before.node(id), d.node(id)) else { continue };
        let now_parent = now.parent;
        let mut n = b.clone();
        n.children = b
            .children
            .iter()
            .filter(|ch| now.children.contains(ch))
            .chain(now.children.iter().filter(|ch| !b.children.contains(ch)))
            .copied()
            .collect();
        let parent = b.parent;
        *d.get_mut(id)? = n;
        if now_parent != parent {
            if let Some(old) = now_parent.and_then(|p| d.node_mut(p)) {
                old.children.retain(|ch| *ch != id);
            }
            if let Some(p) = parent.and_then(|p| d.node_mut(p))
                && !p.children.contains(&id)
            {
                let at = before.node(p.id).and_then(|pb| pb.children.iter().position(|ch| *ch == id)).unwrap_or(p.children.len());
                p.children.insert(at.min(p.children.len()), id);
            }
        }
    }
    // 3. Layers it created go (they hold nothing from before any more).
    for id in &c.created {
        if d.contains(*id) {
            d.remove(*id)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use figcraft_doc::NodeId;
    use serde_json::{Value, json};

    use crate::Session;

    fn id(v: &Value) -> NodeId {
        NodeId(v["id"].as_u64().unwrap())
    }

    fn rect(s: &mut Session, x: i32) -> NodeId {
        id(&s.execute("shape.rectangle", &json!({"x": x, "y": 0, "width": 40, "height": 40})).unwrap())
    }

    fn opacity(s: &Session, id: NodeId) -> f32 {
        s.doc().unwrap().node(id).unwrap().opacity
    }

    fn dim(s: &mut Session, label: &str, ids: &[NodeId], to: f64) -> u64 {
        let cs = s.execute("changeset.begin", &json!({"label": label})).unwrap()["id"].as_u64().unwrap();
        s.execute("node.set", &json!({"ids": ids, "props": {"opacity": to}})).unwrap();
        s.execute("changeset.end", &json!({})).unwrap();
        cs
    }

    #[test]
    fn reverting_the_middle_change_keeps_the_ones_around_it() {
        let mut s = Session::new();
        let (a, b, c) = (rect(&mut s, 0), rect(&mut s, 50), rect(&mut s, 100));
        let one = dim(&mut s, "dim a", &[a], 0.5);
        let two = dim(&mut s, "dim b", &[b], 0.4);
        let three = dim(&mut s, "dim c", &[c], 0.3);
        let list = s.execute("changeset.list", &json!({})).unwrap();
        assert_eq!(list["total"], json!(3), "{list}");
        assert_eq!(list["changesets"][0]["label"], json!("dim c"), "newest first: {list}");
        assert_eq!(list["changesets"][1]["layers"], json!(1), "{list}");
        assert!(list["changesets"][1].get("conflicts").is_none(), "{list}");

        let r = s.execute("changeset.revert", &json!({"id": two})).unwrap();
        assert_eq!((r["reverted"].clone(), r["label"].clone()), (json!(1), json!("dim b")), "{r}");
        assert_eq!((opacity(&s, a), opacity(&s, b), opacity(&s, c)), (0.5, 1.0, 0.3));
        // One undo step: undoing it brings the change back.
        assert!(s.undo().unwrap());
        assert_eq!(opacity(&s, b), 0.4);
        // The outer ones revert too, in any order.
        s.execute("changeset.revert", &json!({"id": three})).unwrap();
        s.execute("changeset.revert", &json!({"id": one})).unwrap();
        assert_eq!((opacity(&s, a), opacity(&s, c)), (1.0, 1.0));
    }

    #[test]
    fn a_later_change_to_the_same_layer_refuses_the_revert_and_names_it() {
        let mut s = Session::new();
        let (a, b) = (rect(&mut s, 0), rect(&mut s, 50));
        let first = dim(&mut s, "dim a and b", &[a, b], 0.5);
        let later = dim(&mut s, "dim a more", &[a], 0.2);
        let list = s.execute("changeset.list", &json!({})).unwrap();
        assert_eq!(list["changesets"][1]["conflicts"], json!(1), "{list}");
        let e = s.execute("changeset.revert", &json!({"id": first})).unwrap_err().to_string();
        assert!(e.contains("refused") && e.contains(&format!("#{}", a.0)) && e.contains("restore_version"), "{e}");
        assert!(!e.contains(&format!("#{}", b.0)), "only the conflicting layer is named: {e}");
        assert_eq!((opacity(&s, a), opacity(&s, b)), (0.2, 0.5), "nothing changed");
        // Reverting the later one first frees the earlier one.
        s.execute("changeset.revert", &json!({"id": later})).unwrap();
        assert_eq!(opacity(&s, a), 0.5);
        s.execute("changeset.revert", &json!({"id": first})).unwrap();
        assert_eq!((opacity(&s, a), opacity(&s, b)), (1.0, 1.0));
        // Nothing left to revert.
        let again = s.execute("changeset.revert", &json!({"id": first})).unwrap();
        assert_eq!(again["noop"], json!(true), "{again}");
    }

    #[test]
    fn created_and_deleted_layers_are_undone_in_place() {
        let mut s = Session::new();
        let (a, b, c) = (rect(&mut s, 0), rect(&mut s, 50), rect(&mut s, 100));
        let page = s.page().unwrap();
        s.execute("changeset.begin", &json!({"label": "tidy"})).unwrap();
        s.execute("edit.delete", &json!({"ids": [b]})).unwrap();
        let made = rect(&mut s, 200);
        let r = s.execute("changeset.end", &json!({})).unwrap();
        assert_eq!((r["created"].clone(), r["deleted"].clone()), (json!(1), json!(1)), "{r}");
        // A later change elsewhere does not block it.
        s.execute("node.set", &json!({"ids": [a], "props": {"opacity": 0.5}})).unwrap();
        s.execute("changeset.revert", &json!({"id": 1})).unwrap();
        let doc = s.doc().unwrap();
        assert!(!doc.contains(made));
        assert_eq!(doc.node(page).unwrap().children, vec![a, b, c], "the deleted layer is back between its neighbours");
        assert_eq!(opacity(&s, a), 0.5);
    }

    #[test]
    fn work_made_later_inside_a_created_layer_blocks_the_revert() {
        let mut s = Session::new();
        s.execute("changeset.begin", &json!({"label": "frame"})).unwrap();
        let f = id(&s.execute("frame.create", &json!({"x": 0, "y": 0, "width": 100, "height": 100})).unwrap());
        s.execute("changeset.end", &json!({})).unwrap();
        let kid = id(&s.execute("shape.rectangle", &json!({"parent": f, "x": 1, "y": 1, "width": 9, "height": 9})).unwrap());
        let e = s.execute("changeset.revert", &json!({"id": 1})).unwrap_err().to_string();
        assert!(e.contains("refused") && e.contains(&format!("#{}", kid.0)), "{e}");
        assert!(s.doc().unwrap().contains(f) && s.doc().unwrap().contains(kid));
    }

    #[test]
    fn mutating_results_carry_the_open_label() {
        let mut s = Session::new();
        let a = rect(&mut s, 0);
        s.execute("changeset.begin", &json!({"label": "dim things"})).unwrap();
        let r = s.execute("node.set", &json!({"ids": [a], "props": {"opacity": 0.5}, "withChanged": true})).unwrap();
        assert_eq!(r["$changed"]["changeset"], json!("dim things"), "{r}");
        s.execute("changeset.end", &json!({})).unwrap();
        let r = s.execute("node.set", &json!({"ids": [a], "props": {"opacity": 0.6}, "withChanged": true})).unwrap();
        assert!(r["$changed"].get("changeset").is_none(), "{r}");
    }

    #[test]
    fn one_changeset_is_open_at_a_time_and_end_needs_one() {
        let mut s = Session::new();
        assert!(s.execute("changeset.end", &json!({})).is_err());
        s.execute("changeset.begin", &json!({"label": "one"})).unwrap();
        let e = s.execute("changeset.begin", &json!({"label": "two"})).unwrap_err().to_string();
        assert!(e.contains("still open"), "{e}");
        assert!(s.execute("changeset.revert", &json!({"id": 1})).is_err(), "an open changeset cannot be reverted");
        let list = s.execute("changeset.list", &json!({})).unwrap();
        assert_eq!(list["open"]["label"], json!("one"), "{list}");
    }

    #[test]
    fn junk_parameters_are_errors_not_panics() {
        let mut s = Session::new();
        for junk in [json!({}), json!({"label": ""}), json!({"label": "   "}), json!({"label": [1]}), json!({"label": "x".repeat(500)})] {
            assert!(s.execute("changeset.begin", &junk).is_err(), "{junk}");
        }
        assert_eq!(s.execute("changeset.list", &json!({})).unwrap()["open"], Value::Null, "a refused begin opens nothing");
        for junk in
            [json!({}), json!({"id": "x"}), json!({"id": -1}), json!({"id": 999}), json!({"id": [1]}), json!({"id": null}), json!({"id": 1.5})]
        {
            assert!(s.execute("changeset.revert", &junk).is_err(), "{junk}");
        }
        for junk in [json!({"limit": 0}), json!({"limit": -1}), json!({"limit": "x"}), json!({"limit": 1000}), json!({"limit": 2.5})] {
            assert!(s.execute("changeset.list", &junk).is_err(), "{junk}");
        }
        assert!(s.execute("changeset.list", &json!({"limit": 5})).is_ok());
    }
}
