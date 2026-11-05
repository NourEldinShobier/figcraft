//! The parameters every command takes (`where`, `list`, `dryRun`, `keepSelection`) and the named
//! layer lists (`list.save`, `list.get`, `list.delete`, `list.all`).

use figcraft_doc::*;
use serde_json::{Value, json};

use crate::Session;
use crate::components::tests::{ex, idv};

fn rect(s: &mut Session, name: &str, x: f64) -> NodeId {
    let page = s.page().unwrap();
    s.doc_mut().unwrap().create(page, NodeKind::Rectangle, name, x, 0.0, 10.0, 10.0).unwrap()
}

fn opacity(s: &Session, id: NodeId) -> f32 {
    s.doc().unwrap().node(id).unwrap().opacity
}

fn name(s: &Session, id: NodeId) -> String {
    s.doc().unwrap().node(id).unwrap().name.clone()
}

fn error(s: &mut Session, cmd: &str, p: Value) -> String {
    s.execute(cmd, &p).unwrap_err().to_string()
}

/// Two buttons and a card on the page.
fn scene() -> (Session, NodeId, NodeId, NodeId) {
    let mut s = Session::new();
    let (a, b, c) = (rect(&mut s, "Button A", 0.0), rect(&mut s, "Button B", 20.0), rect(&mut s, "Card", 40.0));
    (s, a, b, c)
}

#[test]
fn where_finds_and_applies_in_one_call() {
    let (mut s, a, b, c) = scene();
    let r = ex(&mut s, "node.set", json!({"where": {"name": "button"}, "props": {"opacity": 0.5}}));
    assert_eq!(r, json!({"ok": true, "matched": 2, "ids": [a, b]}));
    assert_eq!((opacity(&s, a), opacity(&s, b), opacity(&s, c)), (0.5, 0.5, 1.0));
    assert_eq!(s.undo_depth(), (1, 0), "one undo step");
}

#[test]
fn where_with_no_match_skips_the_command() {
    let (mut s, a, _, _) = scene();
    let r = ex(&mut s, "node.set", json!({"where": {"name": "nothing"}, "props": {"opacity": 0.5}}));
    assert_eq!(r, json!({"matched": 0, "ids": [], "skipped": true}));
    assert_eq!(s.undo_depth(), (0, 0));
    assert_eq!(opacity(&s, a), 1.0);
}

#[test]
fn where_over_the_limit_is_refused_instead_of_truncated() {
    let (mut s, a, b, c) = scene();
    let e = error(&mut s, "node.set", json!({"where": {"limit": 2}, "props": {"opacity": 0.5}}));
    assert!(e.contains("`where` matches 3 layers but only 2 came back"), "{e}");
    assert!(e.contains("raise `where.limit`"), "{e}");
    assert_eq!((opacity(&s, a), opacity(&s, b), opacity(&s, c)), (1.0, 1.0, 1.0));
    assert_eq!(s.undo_depth(), (0, 0));
    let r = ex(&mut s, "node.set", json!({"where": {"limit": 3}, "props": {"opacity": 0.5}}));
    assert_eq!(r["matched"], json!(3));
}

#[test]
fn where_unknown_filters_and_bad_values_are_refused() {
    let (mut s, a, _, _) = scene();
    let e = error(&mut s, "node.set", json!({"where": {"nme": "x"}, "props": {"opacity": 0.5}}));
    assert!(e.contains("unknown parameter `nme`"), "{e}");
    let e = error(&mut s, "node.set", json!({"where": "Button", "props": {"opacity": 0.5}}));
    assert!(e.contains("`where` must be an object"), "{e}");
    let e = error(&mut s, "node.set", json!({"ids": [a], "props": {"opacity": 0.5}, "dryRun": "maybe"}));
    assert!(e.contains("`dryRun` must be a boolean"), "{e}");
    let e = error(&mut s, "node.set", json!({"ids": [a], "props": {"opacity": 0.5}, "keepSelection": "yes"}));
    assert!(e.contains("`keepSelection` must be a boolean"), "{e}");
    assert_eq!(opacity(&s, a), 1.0);
}

#[test]
fn ids_where_and_list_exclude_each_other() {
    let (mut s, a, _, _) = scene();
    let e = error(&mut s, "node.set", json!({"ids": [a], "where": {"name": "a"}, "props": {"opacity": 0.5}}));
    assert!(e.contains("give ids or where, not both"), "{e}");
    let e = error(&mut s, "node.set", json!({"id": a, "list": "x", "props": {"opacity": 0.5}}));
    assert!(e.contains("give ids or list, not both"), "{e}");
    let e = error(&mut s, "node.set", json!({"where": {}, "list": "x", "props": {"opacity": 0.5}}));
    assert!(e.contains("give where or list, not both"), "{e}");
    assert_eq!(opacity(&s, a), 1.0);
}

#[test]
fn a_result_with_its_own_ids_keeps_them() {
    let (mut s, a, _, _) = scene();
    let r = ex(&mut s, "edit.duplicate", json!({"where": {"name": "Button A"}}));
    assert_eq!(r["matched"], json!(1));
    assert_eq!(r["matchedIds"], json!([a]));
    assert_ne!(r["ids"], json!([a]), "`ids` stays the copies");
}

#[test]
fn saved_lists_name_layers_for_later_commands() {
    let (mut s, a, b, c) = scene();
    let r = ex(&mut s, "list.save", json!({"name": "buttons", "where": {"name": "button"}}));
    assert_eq!(r, json!({"name": "buttons", "count": 2, "matched": 2, "ids": [a, b]}));
    assert_eq!(ex(&mut s, "list.all", json!({})), json!({"lists": [{"name": "buttons", "count": 2}]}));
    assert_eq!(ex(&mut s, "list.get", json!({"name": "buttons"})), json!({"name": "buttons", "ids": [a, b]}));
    assert_eq!(s.undo_depth(), (0, 0), "lists are no undo steps");

    let r = ex(&mut s, "node.set", json!({"list": "buttons", "props": {"opacity": 0.5}}));
    assert_eq!(r, json!({"ok": true, "matched": 2, "ids": [a, b]}));
    assert_eq!((opacity(&s, a), opacity(&s, b), opacity(&s, c)), (0.5, 0.5, 1.0));

    // Adding keeps the layers there, saving again replaces them.
    assert_eq!(ex(&mut s, "list.save", json!({"name": "buttons", "ids": [c], "add": true})), json!({"name": "buttons", "count": 3}));
    assert_eq!(ex(&mut s, "list.save", json!({"name": "buttons", "ids": [c, c], "add": true})), json!({"name": "buttons", "count": 3}));
    assert_eq!(ex(&mut s, "list.save", json!({"name": "buttons", "ids": [c]})), json!({"name": "buttons", "count": 1}));

    // Without ids or a filter the selection is saved.
    s.set_selection(vec![a, c]);
    assert_eq!(ex(&mut s, "list.save", json!({"name": "picked"})), json!({"name": "picked", "count": 2}));
}

#[test]
fn layers_deleted_since_drop_out_of_a_list() {
    let (mut s, a, b, _) = scene();
    ex(&mut s, "list.save", json!({"name": "two", "ids": [a, b]}));
    ex(&mut s, "edit.delete", json!({"ids": [a]}));
    assert_eq!(ex(&mut s, "list.get", json!({"name": "two"})), json!({"name": "two", "ids": [b]}));
    assert_eq!(ex(&mut s, "list.all", json!({})), json!({"lists": [{"name": "two", "count": 1}]}));
    assert_eq!(ex(&mut s, "node.set", json!({"list": "two", "props": {"opacity": 0.5}}))["matched"], json!(1));
    ex(&mut s, "edit.delete", json!({"ids": [b]}));
    let r = ex(&mut s, "node.set", json!({"list": "two", "props": {"opacity": 0.5}}));
    assert_eq!(r, json!({"matched": 0, "ids": [], "skipped": true}));
}

#[test]
fn unknown_lists_name_the_known_ones_and_delete_removes() {
    let (mut s, a, _, _) = scene();
    let e = error(&mut s, "node.set", json!({"list": "nope", "props": {"opacity": 0.5}}));
    assert!(e.contains("no list named \"nope\" (none saved yet"), "{e}");
    ex(&mut s, "list.save", json!({"name": "one", "ids": [a]}));
    ex(&mut s, "list.save", json!({"name": "two", "ids": [a]}));
    let e = error(&mut s, "node.set", json!({"list": "nope", "props": {"opacity": 0.5}}));
    assert!(e.contains("no list named \"nope\" (known lists: one, two)"), "{e}");
    assert_eq!(ex(&mut s, "list.delete", json!({"name": "one"})), json!({"name": "one", "deleted": true}));
    let e = error(&mut s, "list.get", json!({"name": "one"}));
    assert!(e.contains("known lists: two"), "{e}");
    let e = error(&mut s, "list.save", json!({"name": "  ", "ids": [a]}));
    assert!(e.contains("`name` must be a non-empty string"), "{e}");
    let e = error(&mut s, "list.save", json!({"name": "x", "ids": [a], "extra": 1}));
    assert!(e.contains("unknown parameter `extra`"), "{e}");
}

#[test]
fn lists_belong_to_their_tab() {
    let (mut s, a, _, _) = scene();
    ex(&mut s, "list.save", json!({"name": "mine", "ids": [a]}));
    s.docs.push(crate::doc_state::DocState::new(Document::new("Other"), None));
    s.active = 1;
    let e = error(&mut s, "list.get", json!({"name": "mine"}));
    assert!(e.contains("no list named \"mine\""), "{e}");
    s.active = 0;
    assert_eq!(ex(&mut s, "list.get", json!({"name": "mine"})), json!({"name": "mine", "ids": [a]}));
}

#[test]
fn dry_run_reports_the_changes_and_rolls_everything_back() {
    let (mut s, a, b, _) = scene();
    let page = s.page().unwrap();
    // One undone step, so there is a redo step a dry run must not drop.
    ex(&mut s, "node.set", json!({"ids": [a], "props": {"name": "First"}}));
    ex(&mut s, "edit.undo", json!({}));
    s.set_selection(vec![b]);
    let doc = s.ds().unwrap().doc.clone();
    let journal = s.journal.len();
    assert_eq!(s.undo_depth(), (0, 1));

    let r = ex(&mut s, "node.set", json!({"ids": [a], "props": {"name": "Renamed"}, "dryRun": true}));
    assert_eq!(r, json!({"dryRun": true, "wouldChange": [a], "created": [], "deleted": [], "count": 1, "truncated": false, "result": {"ok": true}}));
    assert_eq!(name(&s, a), "Button A");
    assert!(*s.ds().unwrap().doc == *doc, "the document is as it was");
    assert_eq!(s.undo_depth(), (0, 1), "undo and redo steps are as they were");
    assert_eq!(s.selection(), vec![b]);
    assert_eq!(s.journal.len(), journal);
    assert_eq!(s.last_command.as_ref().map(|(id, _)| id.as_str()), Some("node.set"), "Repeat still repeats the last real command");

    // A new layer is `created` (and its page changes); nothing is added.
    let r = ex(&mut s, "shape.rectangle", json!({"x": 0, "y": 0, "width": 5, "height": 5, "dryRun": true}));
    assert_eq!(r["created"].as_array().map(Vec::len), Some(1), "{r}");
    assert_eq!(r["wouldChange"], json!([page]));
    assert_eq!(r["count"], json!(2));
    assert!(*s.ds().unwrap().doc == *doc);
    assert_eq!(s.selection(), vec![b]);

    // A deleted layer is `deleted` (and its page changes).
    let r = ex(&mut s, "edit.delete", json!({"ids": [b], "dryRun": true}));
    assert_eq!((r["deleted"].clone(), r["wouldChange"].clone(), r["count"].clone()), (json!([b]), json!([page]), json!(2)));
    assert!(s.doc().unwrap().contains(b));
    assert_eq!(s.undo_depth(), (0, 1));
}

#[test]
fn dry_run_with_where_names_the_layers_it_found() {
    let (mut s, a, b, _) = scene();
    let r = ex(&mut s, "node.set", json!({"where": {"name": "button"}, "props": {"opacity": 0.5}, "dryRun": true}));
    assert_eq!(r["wouldChange"], json!([a, b]));
    assert_eq!((r["matched"].clone(), r["ids"].clone(), r["count"].clone()), (json!(2), json!([a, b]), json!(2)));
    assert_eq!((opacity(&s, a), opacity(&s, b)), (1.0, 1.0));
    assert_eq!(s.undo_depth(), (0, 0));
}

#[test]
fn dry_run_is_refused_for_commands_that_are_not_plain_edits() {
    let (mut s, a, _, _) = scene();
    let e = error(&mut s, "edit.copy", json!({"ids": [a], "dryRun": true}));
    assert!(e.contains("dryRun is not available for edit.copy"), "{e}");
    let e = error(&mut s, "file.save", json!({"dryRun": true}));
    assert!(e.contains("dryRun is not available for file.save"), "{e}");
    // A batch can be dry-run when every step can.
    let batch = |command: &str| json!({"commands": [{"command": command, "params": {"ids": [a]}}], "dryRun": true});
    let e = error(&mut s, "command.batch", batch("edit.copy"));
    assert!(e.contains("dryRun is not available for edit.copy"), "{e}");
    let step = json!({"command": "node.set", "params": {"ids": [a], "props": {"name": "X"}}});
    let r = ex(&mut s, "command.batch", json!({"commands": [step], "dryRun": true}));
    assert_eq!((r["wouldChange"].clone(), r["result"].clone()), (json!([a]), json!([{"ok": true}])));
    assert_eq!(name(&s, a), "Button A");
    assert_eq!(s.undo_depth(), (0, 0));
}

#[test]
fn a_command_with_a_dry_run_of_its_own_keeps_it() {
    let mut s = Session::new();
    let page = s.page().unwrap();
    let r = s.doc_mut().unwrap().create(page, NodeKind::Rectangle, "Rectangle 3", 0.0, 0.0, 10.0, 10.0).unwrap();
    let out = ex(&mut s, "layer.autoName", json!({"ids": [r], "dryRun": true}));
    assert!(out.get("renames").is_some() && out.get("wouldChange").is_none(), "{out}");
    assert_eq!(out["dryRun"], json!(true));
    assert_eq!(name(&s, r), "Rectangle 3");
}

#[test]
fn keep_selection_restores_the_selection_and_the_page() {
    let (mut s, a, b, _) = scene();
    let page1 = s.page().unwrap();
    s.set_selection(vec![a]);
    // A new layer is normally selected.
    ex(&mut s, "shape.rectangle", json!({"x": 0, "y": 0, "width": 5, "height": 5}));
    assert_ne!(s.selection(), vec![a]);
    s.set_selection(vec![a]);
    ex(&mut s, "shape.rectangle", json!({"x": 0, "y": 0, "width": 5, "height": 5, "keepSelection": true}));
    assert_eq!(s.selection(), vec![a]);

    // A new page is shown and cleared of selection; not with keepSelection.
    let page2 = idv(&ex(&mut s, "page.add", json!({"keepSelection": true})));
    assert_eq!((s.page().unwrap(), s.selection()), (page1, vec![a]));
    assert!(s.doc().unwrap().pages.contains(&page2));

    // Go to the other page, then select a layer of the first one and keep what was shown.
    ex(&mut s, "page.select", json!({"id": page2}));
    assert_eq!((s.page().unwrap(), s.selection()), (page2, vec![]));
    ex(&mut s, "select.set", json!({"ids": [b], "keepSelection": true}));
    assert_eq!((s.page().unwrap(), s.selection()), (page2, vec![]));
    ex(&mut s, "select.set", json!({"ids": [b]}));
    assert_eq!((s.page().unwrap(), s.selection()), (page1, vec![b]));

    // Layers deleted by the command drop out of the restored selection.
    s.set_selection(vec![a, b]);
    ex(&mut s, "edit.delete", json!({"ids": [a], "keepSelection": true}));
    assert_eq!(s.selection(), vec![b]);
}

#[test]
fn batch_steps_take_the_generic_parameters_and_the_batch_itself_not_where() {
    let (mut s, a, b, _) = scene();
    let steps = json!([
        {"command": "node.set", "params": {"where": {"name": "button"}, "props": {"opacity": 0.5}}},
        {"command": "node.set", "params": {"where": {"name": "nothing"}, "props": {"opacity": 0.1}}}
    ]);
    let r = ex(&mut s, "command.batch", json!({"commands": steps}));
    assert_eq!(r, json!([{"ok": true, "matched": 2, "ids": [a, b]}, {"matched": 0, "ids": [], "skipped": true}]));
    assert_eq!(s.undo_depth(), (1, 0), "the batch is one step");
    let e = error(&mut s, "command.batch", json!({"commands": [], "where": {"name": "button"}}));
    assert!(e.contains("`where` can't be used on a batch"), "{e}");
}
