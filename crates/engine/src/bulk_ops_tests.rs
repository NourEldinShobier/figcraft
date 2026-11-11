//! Bulk operations: `command.each` (per-layer results, partial failure, a resume cursor), the window
//! and `continueOnError` of `command.batch`, `instances.fill` (a data merge from JSON or CSV rows),
//! `component.swapAll` and the variable, effect and style reach of `edit.replaceColor`.

use figcraft_doc::{NodeId, NodeKind};
use serde_json::{Value, json};

use crate::Session;

fn ex(s: &mut Session, cmd: &str, p: Value) -> Value {
    s.execute(cmd, &p).unwrap_or_else(|e| panic!("{cmd} {p}: {e}"))
}

fn err(s: &mut Session, cmd: &str, p: Value) -> String {
    s.execute(cmd, &p).expect_err(&format!("{cmd} {p} should fail")).to_string()
}

fn id(v: &Value) -> NodeId {
    NodeId(v["id"].as_u64().unwrap())
}

fn rects(s: &mut Session, n: u32) -> Vec<NodeId> {
    (0..n).map(|i| id(&ex(s, "shape.rectangle", json!({"x": i * 50, "y": 0, "width": 40, "height": 40})))).collect()
}

fn opacity(s: &Session, i: NodeId) -> f32 {
    s.doc().unwrap().node(i).unwrap().opacity
}

fn named(s: &Session, root: NodeId, name: &str) -> Option<NodeId> {
    let d = s.doc().unwrap();
    d.subtree(root).into_iter().skip(1).find(|i| d.node(*i).is_some_and(|n| n.name == name))
}

fn text(s: &Session, root: NodeId, name: &str) -> String {
    let d = s.doc().unwrap();
    named(s, root, name).and_then(|i| d.node(i)).and_then(|n| n.kind.text()).map(|t| t.characters.clone()).unwrap_or_default()
}

fn instances(s: &Session, comp: NodeId) -> Vec<NodeId> {
    crate::components::instances_of(s.doc().unwrap(), comp)
}

fn undo_count(s: &Session) -> usize {
    s.ds().unwrap().history.undo.len()
}

// ---------- command.each ----------

#[test]
fn each_lists_a_result_per_layer_and_a_failing_layer_is_rolled_back_alone() {
    let mut s = Session::new();
    let r = rects(&mut s, 3);
    let t = id(&ex(&mut s, "text.create", json!({"text": "Hi", "x": 0, "y": 100})));
    let ids = [r[0], t, r[1], r[2]];
    let steps = undo_count(&s);
    let run = ex(&mut s, "command.each", json!({"command": "text.set", "params": {"characters": "Changed"}, "ids": ids, "continueOnError": true}));
    assert_eq!(run["processed"], 4, "{run}");
    assert_eq!((run["ok"].clone(), run["failed"].clone(), run["done"].clone()), (json!(1), json!(3), json!(true)), "{run}");
    assert_eq!(run["items"][1], json!({"id": t.0, "ok": true}), "{run}");
    assert_eq!(run["items"][0]["ok"], false);
    assert!(run["items"][0]["error"].as_str().is_some_and(|e| !e.is_empty()), "{run}");
    assert!(run.get("nextCursor").is_none());
    assert_eq!(s.doc().unwrap().node(t).unwrap().kind.text().unwrap().characters, "Changed");
    assert_eq!(undo_count(&s), steps + 1, "one undo step for the whole call");
    s.undo().unwrap();
    assert_eq!(s.doc().unwrap().node(t).unwrap().kind.text().unwrap().characters, "Hi");
}

#[test]
fn each_without_continue_on_error_stops_at_the_first_failure_and_changes_nothing() {
    let mut s = Session::new();
    let t = id(&ex(&mut s, "text.create", json!({"text": "Hi", "x": 0, "y": 100})));
    let r = rects(&mut s, 1);
    let e = err(&mut s, "command.each", json!({"command": "text.set", "params": {"characters": "Changed"}, "ids": [t, r[0]]}));
    assert!(e.contains(&format!("layer {}", r[0].0)) && e.contains("nothing was changed"), "{e}");
    assert_eq!(s.doc().unwrap().node(t).unwrap().kind.text().unwrap().characters, "Hi", "the first layer was put back");
}

#[test]
fn each_goes_on_from_a_cursor_in_chunks_until_it_is_done() {
    let mut s = Session::new();
    let r = rects(&mut s, 5);
    let p = json!({"command": "node.set", "params": {"props": {"opacity": 0.5}}, "ids": r, "limit": 2});
    let first = ex(&mut s, "command.each", p);
    assert_eq!((first["processed"].clone(), first["remaining"].clone(), first["done"].clone()), (json!(2), json!(3), json!(false)), "{first}");
    let cursor = first["nextCursor"].as_str().unwrap().to_string();
    assert_eq!(r.iter().filter(|i| opacity(&s, **i) == 0.5).count(), 2);
    let second = ex(&mut s, "command.each", json!({"command": "node.set", "params": {"props": {"opacity": 0.5}}, "limit": 2, "cursor": cursor}));
    assert_eq!(
        (second["processed"].clone(), second["remaining"].clone(), second["nextCursor"].clone()),
        (json!(2), json!(1), json!(cursor)),
        "{second}"
    );
    let last = ex(&mut s, "command.each", json!({"command": "node.set", "params": {"props": {"opacity": 0.5}}, "cursor": cursor}));
    assert_eq!((last["processed"].clone(), last["done"].clone()), (json!(1), json!(true)), "{last}");
    assert!(last.get("nextCursor").is_none());
    assert!(r.iter().all(|i| opacity(&s, *i) == 0.5));
    // The run is over: its cursor is gone.
    let e = err(&mut s, "command.each", json!({"command": "node.set", "params": {"props": {"opacity": 1}}, "cursor": cursor}));
    assert!(e.contains("no run to continue"), "{e}");
}

#[test]
fn each_takes_where_and_dry_run_and_the_generic_guard() {
    let mut s = Session::new();
    let r = rects(&mut s, 4);
    let dry = ex(
        &mut s,
        "command.each",
        json!({"command": "node.set", "params": {"props": {"opacity": 0.5}}, "where": {"type": "RECTANGLE"}, "dryRun": true}),
    );
    assert_eq!(dry["dryRun"], true, "{dry}");
    assert_eq!(dry["count"], 4, "{dry}");
    assert!(r.iter().all(|i| opacity(&s, *i) == 1.0), "a dry run changes nothing");
    let e = err(&mut s, "command.each", json!({"command": "node.set", "params": {"props": {"opacity": 0.5}}, "ids": r, "maxAffected": 2}));
    assert!(e.contains("maxAffected 2"), "{e}");
    assert!(r.iter().all(|i| opacity(&s, *i) == 1.0));
    // A command that cannot be rolled back cannot be dry-run.
    let e = err(&mut s, "command.each", json!({"command": "file.save", "ids": r, "dryRun": true}));
    assert!(e.contains("dryRun is not available"), "{e}");
}

#[test]
fn each_refuses_junk_parameters() {
    let mut s = Session::new();
    let r = rects(&mut s, 2);
    for junk in [
        json!({}),
        json!({"command": 5, "ids": r}),
        json!({"command": "no.such", "ids": r}),
        json!({"command": "command.each", "ids": r}),
        json!({"command": "node.set", "params": 4, "ids": r}),
        json!({"command": "node.set", "params": {"ids": [1]}, "ids": r}),
        json!({"command": "node.set", "params": {"where": {}}, "ids": r}),
        json!({"command": "node.set", "ids": r, "limit": 0}),
        json!({"command": "node.set", "ids": r, "limit": "x"}),
        json!({"command": "node.set", "ids": r, "limit": 99999}),
        json!({"command": "node.set", "ids": r, "limit": 1.5}),
        json!({"command": "node.set", "ids": r, "continueOnError": "maybe"}),
        json!({"command": "node.set", "ids": r, "cursor": 7}),
        json!({"command": "node.set", "ids": r, "cursor": "each-9"}),
        json!({"command": "node.set", "ids": r, "cursor": "other"}),
        json!({"command": "node.set", "ids": "x"}),
        json!({"command": "node.set", "ids": [999999]}),
        json!({"command": "node.set", "ids": [], "continueOnError": true}),
    ] {
        assert!(s.execute("command.each", &junk).is_err(), "{junk}");
    }
    assert!(r.iter().all(|i| opacity(&s, *i) == 1.0));
}

// ---------- command.batch window ----------

#[test]
fn a_batch_runs_a_window_of_its_steps_and_names_the_next_one() {
    let mut s = Session::new();
    let r = rects(&mut s, 5);
    let steps: Vec<Value> = r.iter().map(|i| json!({"command": "node.set", "params": {"ids": [i], "props": {"opacity": 0.5}}})).collect();
    let a = ex(&mut s, "command.batch", json!({"commands": steps, "limit": 2, "continueOnError": true}));
    assert_eq!(
        (a["processed"].clone(), a["total"].clone(), a["next"].clone(), a["done"].clone()),
        (json!(2), json!(5), json!(2), json!(false)),
        "{a}"
    );
    assert_eq!(a["results"][1]["step"], 1, "{a}");
    let b = ex(&mut s, "command.batch", json!({"commands": steps, "from": 2, "limit": 2, "continueOnError": true}));
    assert_eq!((b["from"].clone(), b["next"].clone()), (json!(2), json!(4)), "{b}");
    assert_eq!(b["results"][0]["step"], 2, "steps count from the whole batch: {b}");
    let c = ex(&mut s, "command.batch", json!({"commands": steps, "from": 4}));
    assert_eq!((c["processed"].clone(), c["done"].clone()), (json!(1), json!(true)), "{c}");
    assert!(c.get("next").is_none());
    assert!(r.iter().all(|i| opacity(&s, *i) == 0.5));
    // Without a window the result is the array it always was.
    let plain = ex(&mut s, "command.batch", json!({"commands": steps[..1].to_vec()}));
    assert!(plain.is_array(), "{plain}");
}

#[test]
fn a_batch_takes_continue_on_error_as_on_error_continue_and_refuses_a_contradiction() {
    let mut s = Session::new();
    let steps = json!([{"command": "no.such.command"}, {"command": "shape.rectangle", "params": {"x": 0, "y": 0, "width": 5, "height": 5}}]);
    let r = ex(&mut s, "command.batch", json!({"commands": steps, "continueOnError": true}));
    assert_eq!((r[0]["ok"].clone(), r[1]["ok"].clone()), (json!(false), json!(true)), "{r}");
    for (junk, why) in [
        (json!({"commands": steps, "onError": "abort", "continueOnError": true}), "disagree"),
        (json!({"commands": steps, "continueOnError": "maybe"}), "boolean"),
        (json!({"commands": steps, "from": 9}), "past the last"),
        (json!({"commands": steps, "from": -1}), "`from`"),
        (json!({"commands": steps, "limit": 0}), "`limit`"),
        (json!({"commands": steps, "limit": "x"}), "`limit`"),
    ] {
        let e = err(&mut s, "command.batch", junk.clone());
        assert!(e.contains(why), "{junk}: {e}");
    }
}

// ---------- instances.fill ----------

/// A component "Card" (a `Title` text bound to the text property `label`, a `Caption` text and a
/// `Badge` shape bound to the boolean property `showBadge`).
fn card(s: &mut Session, name: &str, x: f64, with_props: bool) -> NodeId {
    let f = id(&ex(s, "frame.create", json!({"x": x, "y": 0, "width": 200, "height": 80, "name": name})));
    let title = id(&ex(s, "text.create", json!({"text": "Title", "x": 10, "y": 10, "parent": f, "props": {"name": "Title"}})));
    ex(s, "text.create", json!({"text": "Caption", "x": 10, "y": 40, "parent": f, "props": {"name": "Caption"}}));
    let badge = id(&ex(s, "shape.ellipse", json!({"x": 170, "y": 10, "width": 20, "height": 20, "parent": f, "props": {"name": "Badge"}})));
    let c = id(&ex(s, "component.create", json!({"ids": [f]})));
    if with_props {
        ex(s, "component.addProperty", json!({"id": c, "name": "label", "type": "text", "layer": title}));
        ex(s, "component.addProperty", json!({"id": c, "name": "showBadge", "type": "boolean", "layer": badge}));
    }
    c
}

#[test]
fn five_hundred_rows_become_five_hundred_instances_in_one_undo_step() {
    let mut s = Session::new();
    let c = card(&mut s, "Card", 0.0, true);
    let rows: Vec<Value> = (0..500)
        .map(|i| json!({"key": format!("Card {i}"), "label": format!("Item {i}"), "showBadge": i % 2 == 0, "Caption": format!("Note {i}"), "extra": "x"}))
        .collect();
    let steps = undo_count(&s);
    let r = ex(&mut s, "instances.fill", json!({"component": c, "rows": rows, "keyColumn": "key"}));
    assert_eq!((r["rows"].clone(), r["created"].clone(), r["failed"].clone()), (json!(500), json!(500), json!([])), "{r}");
    assert_eq!(r["unmatchedColumns"], json!(["extra"]), "{r}");
    let made = instances(&s, c);
    assert_eq!(made.len(), 500);
    assert_eq!(undo_count(&s), steps + 1, "one undo step");
    let doc = s.doc().unwrap();
    assert!(doc.validate().is_empty(), "{:?}", doc.validate());
    for i in [0usize, 1, 499] {
        let inst = *made.iter().find(|m| doc.node(**m).unwrap().name == format!("Card {i}")).unwrap();
        assert_eq!(text(&s, inst, "Title"), format!("Item {i}"), "the text property reaches its layer");
        assert_eq!(text(&s, inst, "Caption"), format!("Note {i}"), "a layer named by a column");
        let badge = named(&s, inst, "Badge").unwrap();
        assert_eq!(doc.node(badge).unwrap().visible, i % 2 == 0, "the boolean property");
    }
    s.undo().unwrap();
    assert!(instances(&s, c).is_empty());
}

#[test]
fn a_bad_row_is_reported_and_put_back_while_the_others_apply() {
    let mut s = Session::new();
    let c = card(&mut s, "Card", 0.0, true);
    let rows = json!([
        {"key": "A", "label": "one"},
        {"key": "B", "showBadge": "maybe"},
        {"key": "C", "label": "three", "Caption": "ok"},
    ]);
    let r = ex(&mut s, "instances.fill", json!({"component": c, "rows": rows, "keyColumn": "key"}));
    assert_eq!(r["created"], 2, "{r}");
    assert_eq!(r["failed"][0]["row"], 1, "{r}");
    assert!(r["failed"][0]["error"].as_str().unwrap().contains("showBadge"), "{r}");
    let names: Vec<String> = instances(&s, c).iter().map(|i| s.doc().unwrap().node(*i).unwrap().name.clone()).collect();
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(!names.contains(&"B".to_string()), "the failed row left no instance: {names:?}");
    // Stopping at the first failure undoes everything.
    let steps = undo_count(&s);
    let e = err(&mut s, "instances.fill", json!({"component": c, "rows": rows, "keyColumn": "key", "continueOnError": false}));
    assert!(e.contains("row 1") && e.contains("nothing was changed"), "{e}");
    assert_eq!((instances(&s, c).len(), undo_count(&s)), (2, steps));
}

#[test]
fn update_mode_finds_instances_by_a_key_column_and_reports_unmatched_rows_and_columns() {
    let mut s = Session::new();
    let c = card(&mut s, "Card", 0.0, true);
    let rows: Vec<Value> = ["a", "b", "c"].iter().map(|k| json!({"key": k, "label": format!("first {k}")})).collect();
    ex(&mut s, "instances.fill", json!({"component": c, "rows": rows, "keyColumn": "key"}));
    let again = json!([
        {"key": "b", "label": "second b", "Caption": "cap", "stray": 1},
        {"key": "zzz", "label": "lost"},
        {"key": "a", "showBadge": false},
        {"label": "no key"},
    ]);
    let steps = undo_count(&s);
    let r = ex(&mut s, "instances.fill", json!({"component": c, "mode": "update", "keyColumn": "key", "rows": again}));
    assert_eq!(r["updated"], 2, "{r}");
    assert_eq!(r["unmatchedRowCount"], 2, "{r}");
    assert_eq!(r["unmatchedRows"][0], json!({"row": 1, "key": "zzz"}), "{r}");
    assert_eq!(r["unmatchedRows"][1], json!({"row": 3, "key": null}), "{r}");
    assert_eq!(r["unmatchedColumns"], json!(["stray"]), "{r}");
    assert_eq!(undo_count(&s), steps + 1);
    let by_name = |s: &Session, n: &str| *instances(s, c).iter().find(|i| s.doc().unwrap().node(**i).unwrap().name == n).unwrap();
    let (a, b, cc) = (by_name(&s, "a"), by_name(&s, "b"), by_name(&s, "c"));
    assert_eq!((text(&s, a, "Title"), text(&s, b, "Title"), text(&s, cc, "Title")), ("first a".into(), "second b".into(), "first c".into()));
    assert_eq!(text(&s, b, "Caption"), "cap");
    assert!(!s.doc().unwrap().node(named(&s, a, "Badge").unwrap()).unwrap().visible);
    // `ids` name the instances instead of a component.
    let r = ex(&mut s, "instances.fill", json!({"ids": [cc], "mode": "update", "keyColumn": "key", "rows": [{"key": "c", "label": "by id"}]}));
    assert_eq!(r["updated"], 1, "{r}");
    assert_eq!(text(&s, cc, "Title"), "by id");
}

#[test]
fn csv_rows_are_read_with_quotes_commas_and_line_breaks() {
    let mut s = Session::new();
    let c = card(&mut s, "Card", 0.0, true);
    let csv = "\u{feff}name,label,showBadge,Caption\r\nOne,\"Hello, \"\"world\"\"\",true,\"two\nlines\"\r\n\r\nTwo,plain,no,\r\nShort,only label\r\n";
    let r = ex(&mut s, "instances.fill", json!({"component": c, "csv": csv, "keyColumn": "name"}));
    assert_eq!((r["created"].clone(), r["failed"].clone()), (json!(3), json!([])), "{r}");
    let by_name = |s: &Session, n: &str| *instances(s, c).iter().find(|i| s.doc().unwrap().node(**i).unwrap().name == n).unwrap();
    let one = by_name(&s, "One");
    assert_eq!(text(&s, one, "Title"), "Hello, \"world\"");
    assert_eq!(text(&s, one, "Caption"), "two\nlines");
    let two = by_name(&s, "Two");
    assert!(!s.doc().unwrap().node(named(&s, two, "Badge").unwrap()).unwrap().visible, "\"no\" is false");
    assert_eq!(text(&s, two, "Caption"), "", "an empty text cell clears the text");
    assert_eq!(text(&s, by_name(&s, "Short"), "Title"), "only label");
    // Tab separated data works too.
    let tsv = "name\tlabel\nTabbed\tfrom tab";
    ex(&mut s, "instances.fill", json!({"component": c, "csv": tsv, "keyColumn": "name"}));
    assert_eq!(text(&s, by_name(&s, "Tabbed"), "Title"), "from tab");
}

#[test]
fn a_map_renames_columns_and_skips_some_and_variant_and_swap_properties_are_set() {
    let mut s = Session::new();
    // A set with a `Size` variant property.
    let small = card(&mut s, "Size=S", 0.0, false);
    let large = card(&mut s, "Size=L", 300.0, false);
    let set = id(&ex(&mut s, "component.combineVariants", json!({"ids": [small, large]})));
    let r = ex(
        &mut s,
        "instances.fill",
        json!({
            "component": set,
            "rows": [{"Heading": "Hi", "Dimension": "L", "internal": "x", "hide": "false", "Pic": "x"}],
            "map": {"Heading": {"layer": "Title"}, "Dimension": "Size", "internal": null, "hide": {"layer": "Badge", "field": "visible"}},
        }),
    );
    assert_eq!((r["created"].clone(), r["failed"].clone()), (json!(1), json!([])), "{r}");
    assert_eq!(r["unmatchedColumns"], json!(["Pic"]), "no such property: {r}");
    let inst = NodeId(r["ids"][0].as_u64().unwrap());
    let NodeKind::Instance { component, .. } = &s.doc().unwrap().node(inst).unwrap().kind else { panic!("not an instance") };
    assert_eq!(*component, large, "the variant property switched the variant");
    assert_eq!(text(&s, inst, "Title"), "Hi");
    assert!(!s.doc().unwrap().node(named(&s, inst, "Badge").unwrap()).unwrap().visible);
}

#[test]
fn fill_refuses_junk_parameters() {
    let mut s = Session::new();
    let c = card(&mut s, "Card", 0.0, true);
    let rows = json!([{"label": "x"}]);
    let before = instances(&s, c).len();
    for junk in [
        json!({}),
        json!({"component": c}),
        json!({"component": c, "rows": []}),
        json!({"component": c, "rows": "x"}),
        json!({"component": c, "rows": [1]}),
        json!({"component": c, "rows": rows, "csv": "a\n1"}),
        json!({"component": c, "csv": 5}),
        json!({"component": c, "csv": ""}),
        json!({"component": c, "csv": "a,a\n1,2"}),
        json!({"component": c, "csv": "a,\n1,2"}),
        json!({"component": c, "csv": "a\n1,2"}),
        json!({"component": c, "csv": "a\n\"open"}),
        json!({"component": 99999, "rows": rows}),
        json!({"component": "x", "rows": rows}),
        json!({"component": c, "rows": rows, "mode": "merge"}),
        json!({"component": c, "rows": rows, "mode": "update"}),
        json!({"mode": "update", "keyColumn": "label", "rows": rows, "ids": [c]}),
        json!({"component": c, "rows": rows, "map": 3}),
        json!({"component": c, "rows": rows, "map": {"label": {"field": "text"}}}),
        json!({"component": c, "rows": rows, "map": {"label": {"layer": "Title", "field": "fill"}}}),
        json!({"component": c, "rows": rows, "map": {"label": 5}}),
        json!({"component": c, "rows": rows, "keyColumn": 5}),
        json!({"component": c, "rows": rows, "matchBy": "id"}),
        json!({"component": c, "rows": rows, "gap": -3}),
        json!({"component": c, "rows": rows, "gap": "x"}),
        json!({"component": c, "rows": rows, "direction": "diagonal"}),
        json!({"component": c, "rows": rows, "parent": 99999}),
        json!({"component": c, "rows": rows, "continueOnError": "maybe"}),
        json!({"component": c, "rows": vec![json!({"label": "x"}); 2001]}),
    ] {
        assert!(s.execute("instances.fill", &junk).is_err(), "{junk}");
    }
    assert_eq!(instances(&s, c).len(), before, "nothing was made");
    // Cells that are no text: the row fails, the call does not.
    let r = ex(&mut s, "instances.fill", json!({"component": c, "rows": [{"label": [1]}, {"label": {"a": 1}}, {"label": null}]}));
    assert_eq!(r["failed"].as_array().unwrap().len(), 2, "{r}");
    assert_eq!(r["created"], 1, "a null cell leaves the row with nothing to set: {r}");
}

#[test]
fn fill_dry_run_changes_nothing() {
    let mut s = Session::new();
    let c = card(&mut s, "Card", 0.0, true);
    let r = ex(&mut s, "instances.fill", json!({"component": c, "rows": [{"label": "a"}, {"label": "b"}], "dryRun": true}));
    assert_eq!(r["dryRun"], true, "{r}");
    assert!(r["count"].as_u64().unwrap() >= 2, "{r}");
    assert!(instances(&s, c).is_empty());
}

// ---------- component.swapAll ----------

#[test]
fn swap_all_moves_every_instance_keeping_overrides_by_name_and_reports_the_rest() {
    let mut s = Session::new();
    let old = card(&mut s, "Old", 0.0, true);
    // The new card names its title layer "Heading" and has no Caption.
    let f = id(&ex(&mut s, "frame.create", json!({"x": 400, "y": 0, "width": 200, "height": 80, "name": "New"})));
    ex(&mut s, "text.create", json!({"text": "Heading", "x": 10, "y": 10, "parent": f, "props": {"name": "Heading"}}));
    ex(&mut s, "shape.ellipse", json!({"x": 170, "y": 10, "width": 20, "height": 20, "parent": f, "props": {"name": "Badge"}}));
    let new = id(&ex(&mut s, "component.create", json!({"ids": [f]})));
    let heading = named(&s, new, "Heading").unwrap();
    ex(&mut s, "component.addProperty", json!({"id": new, "name": "headline", "type": "text", "layer": heading}));
    let rows: Vec<Value> = (0..3).map(|i| json!({"label": format!("T{i}"), "Caption": format!("C{i}"), "showBadge": i == 1})).collect();
    ex(&mut s, "instances.fill", json!({"component": old, "rows": rows}));
    let steps = undo_count(&s);
    let dry = ex(&mut s, "component.swapAll", json!({"from": old, "to": new, "dryRun": true}));
    assert_eq!(dry["dryRun"], true, "{dry}");
    assert_eq!(instances(&s, old).len(), 3, "a dry run swaps nothing");
    let r = ex(&mut s, "component.swapAll", json!({"from": old, "to": new, "mapOverrides": {"Title": "Heading", "label": "headline"}}));
    assert_eq!((r["swapped"].clone(), r["failed"].clone()), (json!(3), json!([])), "{r}");
    assert!(instances(&s, old).is_empty());
    assert_eq!(instances(&s, new).len(), 3);
    assert_eq!(undo_count(&s), steps + 1, "one undo step");
    let layers = r["unmatched"]["layers"].as_array().unwrap();
    assert!(layers.iter().any(|l| l["name"] == "Caption" && l["count"] == 3), "the Caption overrides have no layer to go to: {r}");
    assert!(layers.iter().all(|l| l["name"] != "Heading"), "{r}");
    let doc = s.doc().unwrap();
    assert!(doc.validate().is_empty(), "{:?}", doc.validate());
    for (i, inst) in instances(&s, new).iter().enumerate() {
        let want = (0..3).map(|k| format!("T{k}")).collect::<Vec<_>>();
        assert!(want.contains(&text(&s, *inst, "Heading")), "instance {i} kept its text through the rename: {}", text(&s, *inst, "Heading"));
    }
    s.undo().unwrap();
    assert_eq!(instances(&s, old).len(), 3);
}

#[test]
fn swap_all_without_a_rename_reports_the_unmatched_and_keeps_matching_names() {
    let mut s = Session::new();
    let old = card(&mut s, "Old", 0.0, false);
    let new = card(&mut s, "Newer", 400.0, false);
    let a = id(&ex(&mut s, "instance.create", json!({"component": old, "x": 0, "y": 500})));
    let caption = named(&s, a, "Caption").unwrap();
    ex(&mut s, "text.set", json!({"id": caption, "characters": "Mine"}));
    let r = ex(&mut s, "component.swapAll", json!({"from": old, "to": new}));
    assert_eq!(r["swapped"], 1, "{r}");
    assert_eq!(r["unmatched"], json!({"layers": [], "props": []}), "every name matches: {r}");
    assert_eq!(text(&s, a, "Caption"), "Mine", "the override follows the layer name");
}

#[test]
fn swap_all_leaves_nested_instances_and_limits_to_ids_and_refuses_junk() {
    let mut s = Session::new();
    let old = card(&mut s, "Old", 0.0, false);
    let new = card(&mut s, "Newer", 400.0, false);
    let outer = {
        let f = id(&ex(&mut s, "frame.create", json!({"x": 0, "y": 600, "width": 300, "height": 200, "name": "Panel"})));
        ex(&mut s, "instance.create", json!({"component": old, "x": 0, "y": 0, "parent": f}));
        id(&ex(&mut s, "component.create", json!({"ids": [f]})))
    };
    let panel = id(&ex(&mut s, "instance.create", json!({"component": outer, "x": 0, "y": 1000})));
    let loose = id(&ex(&mut s, "instance.create", json!({"component": old, "x": 0, "y": 1300})));
    let other = id(&ex(&mut s, "instance.create", json!({"component": old, "x": 0, "y": 1600})));
    let r = ex(&mut s, "component.swapAll", json!({"from": old, "to": new, "ids": [loose, panel]}));
    assert_eq!(r["swapped"], 1, "{r}");
    assert_eq!(r["skippedNested"], 1, "the one inside the panel instance is left alone: {r}");
    let comp = |i| match &s.doc().unwrap().node(i).unwrap().kind {
        NodeKind::Instance { component, .. } => *component,
        k => panic!("{k:?}"),
    };
    assert_eq!((comp(loose), comp(other)), (new, old), "only the instance inside `ids` moved");
    for junk in [
        json!({}),
        json!({"from": old}),
        json!({"from": old, "to": old}),
        json!({"from": loose, "to": new}),
        json!({"from": old, "to": 999_999}),
        json!({"from": old, "to": "x"}),
        json!({"from": old, "to": new, "ids": 4}),
        json!({"from": old, "to": new, "ids": [999_999]}),
        json!({"from": old, "to": new, "mapOverrides": [1]}),
        json!({"from": old, "to": new, "mapOverrides": {"a": 3}}),
        json!({"from": old, "to": new, "mapOverrides": {"a": ""}}),
    ] {
        assert!(s.execute("component.swapAll", &junk).is_err(), "{junk}");
    }
}

// ---------- edit.replaceColor reach ----------

fn colour_variable(s: &mut Session) {
    ex(s, "variable.createCollection", json!({"name": "Tokens", "modes": ["Light"]}));
    ex(s, "variable.create", json!({"collection": "Tokens", "name": "danger", "type": "color", "value": "#00FF00"}));
}

#[test]
fn replace_color_can_bind_to_a_variable_and_reach_shadows() {
    let mut s = Session::new();
    colour_variable(&mut s);
    let a = id(&ex(&mut s, "shape.rectangle", json!({"x": 0, "y": 0, "width": 10, "height": 10, "props": {"fill": "#FF0000", "stroke": "#FF0000"}})));
    let b = id(&ex(&mut s, "shape.rectangle", json!({"x": 20, "y": 0, "width": 10, "height": 10, "props": {"fill": "#0000FF"}})));
    ex(&mut s, "node.set", json!({"ids": [b], "props": {"effects": [{"type": "dropShadow", "color": "#FF0000", "offset": [0, 2], "radius": 4}]}}));
    let dry = ex(&mut s, "edit.replaceColor", json!({"from": "#FF0000", "variable": "danger", "dryRun": true}));
    assert_eq!(dry["dryRun"], true, "{dry}");
    assert_eq!(dry["wouldChange"], json!([a.0, b.0]), "{dry}");
    let r = ex(&mut s, "edit.replaceColor", json!({"from": "#FF0000", "variable": "danger"}));
    assert_eq!((r["paints"].clone(), r["effects"].clone()), (json!(2), json!(1)), "{r}");
    let n = s.doc().unwrap().node(a).unwrap();
    assert!(n.bound.contains_key("fills/0/color") && n.bound.contains_key("strokes/0/color"), "{:?}", n.bound);
    assert!(s.doc().unwrap().node(b).unwrap().bound.contains_key("effects/0/color"));
    // The shadow no longer matches: nothing left to do.
    let again = ex(&mut s, "edit.replaceColor", json!({"from": "#FF0000", "to": "#00FF00"}));
    assert_eq!((again["paints"].clone(), again["effects"].clone()), (json!(0), json!(0)), "{again}");
}

#[test]
fn replace_color_recolours_a_shadow_keeping_its_alpha_and_effects_can_be_left_out() {
    let mut s = Session::new();
    let r = id(&ex(&mut s, "shape.rectangle", json!({"x": 0, "y": 0, "width": 10, "height": 10})));
    ex(&mut s, "node.set", json!({"ids": [r], "props": {"effects": [{"type": "dropShadow", "color": "#FF000040", "offset": [0, 2], "radius": 4}]}}));
    let skipped = ex(&mut s, "edit.replaceColor", json!({"from": "#FF0000", "to": "#0000FF", "effects": false}));
    assert_eq!(skipped["effects"], 0, "{skipped}");
    let done = ex(&mut s, "edit.replaceColor", json!({"from": "#FF0000", "to": "#0000FF"}));
    assert_eq!(done["effects"], 1, "{done}");
    let n = s.doc().unwrap().node(r).unwrap();
    let figcraft_doc::EffectKind::DropShadow(sh) = &n.effects[0].kind else { panic!("not a shadow") };
    assert_eq!(sh.color.to_rgba8(), [0, 0, 255, 64]);
}

#[test]
fn replace_color_recolours_styles_so_their_layers_follow() {
    let mut s = Session::new();
    let a = id(&ex(&mut s, "shape.rectangle", json!({"x": 0, "y": 0, "width": 10, "height": 10, "props": {"fill": "#FF0000"}})));
    let st = ex(&mut s, "style.create", json!({"type": "paint", "name": "Brand/Red", "from": a}))["id"].as_str().unwrap().to_string();
    ex(&mut s, "page.add", json!({"name": "Second"}));
    let b = id(&ex(&mut s, "shape.rectangle", json!({"x": 0, "y": 0, "width": 10, "height": 10})));
    ex(&mut s, "style.apply", json!({"ids": [b], "style": st}));
    s.execute("page.select", &json!({"index": 0})).unwrap();
    // Page scope leaves the style, and the layer on the other page, alone.
    let page = ex(&mut s, "edit.replaceColor", json!({"from": "#FF0000", "to": "#00FF00"}));
    assert_eq!(page["styles"], json!([]), "{page}");
    // Undo that, then recolour the file.
    s.undo().unwrap();
    let file = ex(&mut s, "edit.replaceColor", json!({"from": "#FF0000", "to": "#00FF00", "scope": "file"}));
    assert_eq!(file["styles"], json!([st]), "{file}");
    let fill = |s: &Session, i: NodeId| match &s.doc().unwrap().node(i).unwrap().fills[0].kind {
        figcraft_color::PaintKind::Solid { color } => color.to_rgba8(),
        k => panic!("{k:?}"),
    };
    assert_eq!((fill(&s, a), fill(&s, b)), ([0, 255, 0, 255], [0, 255, 0, 255]), "the layer using the style follows it");
    assert_eq!(s.doc().unwrap().node(b).unwrap().styles.fill.as_deref(), Some(st.as_str()), "and stays linked");
}

#[test]
fn replace_color_refuses_a_missing_or_doubled_target_and_a_wrong_variable() {
    let mut s = Session::new();
    colour_variable(&mut s);
    ex(&mut s, "variable.create", json!({"collection": "Tokens", "name": "size", "type": "number", "value": 4}));
    let r = id(&ex(&mut s, "shape.rectangle", json!({"x": 0, "y": 0, "width": 10, "height": 10, "props": {"fill": "#FF0000"}})));
    for junk in [
        json!({"from": "#FF0000"}),
        json!({"from": "#FF0000", "to": "#00FF00", "variable": "danger"}),
        json!({"from": "#FF0000", "variable": "nope"}),
        json!({"from": "#FF0000", "variable": "size"}),
        json!({"from": "#FF0000", "variable": 5}),
        json!({"from": "#FF0000", "variable": ["danger"]}),
        json!({"from": "#FF0000", "to": "nonsense"}),
        json!({"from": "#FF0000", "to": "#00FF00", "scope": "everywhere"}),
    ] {
        assert!(s.execute("edit.replaceColor", &junk).is_err(), "{junk}");
    }
    assert!(s.doc().unwrap().node(r).unwrap().bound.is_empty());
}

#[test]
fn a_guard_dry_run_does_not_use_up_a_cursor() {
    let mut s = Session::new();
    let r = rects(&mut s, 5);
    let p = json!({"command": "node.set", "params": {"props": {"opacity": 0.5}}, "ids": r, "limit": 2, "maxAffected": 100});
    let first = ex(&mut s, "command.each", p);
    // Only the real run keeps a cursor: the guard's dry run before it left none behind.
    assert_eq!(first["nextCursor"], "each-1", "{first}");
    let second = ex(
        &mut s,
        "command.each",
        json!({"command": "node.set", "params": {"props": {"opacity": 0.5}}, "cursor": "each-1", "limit": 2, "maxAffected": 100}),
    );
    assert_eq!((second["processed"].clone(), second["nextCursor"].clone()), (json!(2), json!("each-1")), "{second}");
    let last =
        ex(&mut s, "command.each", json!({"command": "node.set", "params": {"props": {"opacity": 0.5}}, "cursor": "each-1", "maxAffected": 100}));
    assert_eq!(last["done"], true, "{last}");
    assert!(r.iter().all(|i| opacity(&s, *i) == 0.5));
}
