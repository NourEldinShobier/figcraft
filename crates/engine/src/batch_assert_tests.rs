//! `assert` steps and `preview: true` of `command.batch`.

use figcraft_doc::{NodeId, NodeKind};
use serde_json::{Value, json};

use crate::Session;

/// A 200x100 frame `F` holding `A` (10,10 50x50) and `B` (100,10 50x50), apart and inside.
fn scene() -> (Session, NodeId, NodeId) {
    let mut s = Session::new();
    let page = s.page().unwrap();
    let doc = s.doc_mut().unwrap();
    let f = doc.create(page, NodeKind::Frame, "F", 0.0, 0.0, 200.0, 100.0).unwrap();
    let a = doc.create(f, NodeKind::Rectangle, "A", 10.0, 10.0, 50.0, 50.0).unwrap();
    let b = doc.create(f, NodeKind::Rectangle, "B", 100.0, 10.0, 50.0, 50.0).unwrap();
    (s, a, b)
}

fn x_of(s: &Session, id: NodeId) -> f64 {
    s.doc().unwrap().get(id).unwrap().transform.translation().x
}

fn mv(id: NodeId, x: f64) -> Value {
    json!({"command": "node.set", "params": {"id": id, "props": {"x": x}}})
}

fn run(s: &mut Session, params: Value) -> crate::Result<Value> {
    s.execute("command.batch", &params)
}

fn msg(r: crate::Result<Value>) -> String {
    r.unwrap_err().to_string()
}

#[test]
fn passing_assertions_run_and_report() {
    let (mut s, a, _) = scene();
    let steps = json!([
        mv(a, 20.0),
        {"assert": {"noOverflow": {}, "noOverlap": true}},
        {"assert": {"count": {"where": {"type": "rectangle"}, "equals": 2}}},
        {"assert": {"exists": {"where": {"name": "A"}}}},
        {"assert": {"none": {"where": {"name": "Nope"}}}},
        {"assert": {"contrast": {}}},
        {"assert": {"count": {"where": {"type": "rectangle"}, "min": 1, "max": 5}}}
    ]);
    let r = run(&mut s, json!({"commands": steps})).unwrap();
    assert_eq!(r[1], json!({"ok": true, "checked": ["noOverflow", "noOverlap"]}));
    assert_eq!(r.as_array().unwrap().len(), 7);
    assert_eq!(x_of(&s, a), 20.0);
}

#[test]
fn a_failed_assertion_names_the_step_and_rolls_everything_back() {
    let (mut s, a, b) = scene();
    let before = s.ds().unwrap().history.undo.len();
    for (assertion, why) in [
        (json!({"noOverflow": {}}), "assertion `noOverflow` failed: 1 overflow problem(s)"),
        (json!({"noOverlap": {}}), "assertion `noOverlap` failed: 1 overlap(s)"),
        (json!({"count": {"where": {"type": "rectangle"}, "equals": 5}}), "2 layer(s) match, expected exactly 5"),
        (json!({"exists": {"where": {"name": "Nope"}}}), "0 layer(s) match, expected at least 1"),
        (json!({"none": {"where": {"name": "A"}}}), "1 layer(s) match, expected exactly 0"),
        (json!({"count": {"where": {"type": "rectangle"}, "min": 3}}), "expected at least 3"),
    ] {
        // Step 0 moves A past B's edge and past the frame's, so every assertion above sees its trouble.
        let steps = json!([
            {"command": "node.set", "params": {"id": a, "props": {"x": 120.0, "width": 120.0}}},
            {"assert": assertion}
        ]);
        let e = msg(run(&mut s, json!({"commands": steps})));
        assert!(e.contains("step 1 (batch.assert)"), "{e}");
        assert!(e.contains(why), "{e}");
        assert!(e.contains("rolled back"), "{e}");
        assert_eq!(x_of(&s, a), 10.0);
        assert_eq!(x_of(&s, b), 100.0);
        assert_eq!(s.ds().unwrap().history.undo.len(), before);
    }
}

#[test]
fn a_failed_assertion_in_continue_mode_fails_only_its_step() {
    let (mut s, a, _) = scene();
    let steps = json!([mv(a, 20.0), {"assert": {"none": {"where": {"name": "A"}}}}, mv(a, 30.0)]);
    let r = run(&mut s, json!({"onError": "continue", "commands": steps})).unwrap();
    assert_eq!(r[1]["ok"], json!(false));
    assert!(r[1]["error"].as_str().unwrap().contains("assertion `none` failed"), "{r}");
    assert_eq!(r[2]["ok"], json!(true));
    assert_eq!(x_of(&s, a), 30.0);
}

#[test]
fn an_assertion_can_use_step_references() {
    let (mut s, _, _) = scene();
    let steps = json!([
        {"command": "shape.rectangle", "as": "r", "params": {"x": 500, "y": 0, "width": 10, "height": 10}},
        {"assert": {"exists": {"where": {"root": {"$ref": "r"}, "name": "Nope"}}}}
    ]);
    assert!(msg(run(&mut s, json!({"commands": steps}))).contains("step 1"));
}

#[test]
fn preview_returns_a_diff_and_changes_nothing() {
    let (mut s, a, b) = scene();
    let history = s.ds().unwrap().history.undo.len();
    let hash = s.execute("history.hash", &json!({})).unwrap();
    let steps = json!([mv(a, 20.0), {"assert": {"noOverlap": {}}}, {"command": "node.set", "params": {"id": b, "props": {"name": "Renamed"}}},
        {"command": "shape.rectangle", "params": {"x": 0, "y": 0, "width": 5, "height": 5}}]);
    let r = run(&mut s, json!({"preview": true, "commands": steps})).unwrap();
    assert_eq!(r["preview"], json!(true));
    assert_eq!(r["rolledBack"], json!(true));
    assert_eq!(r["results"].as_array().unwrap().len(), 4);
    assert_eq!(r["diff"]["counts"], json!({"added": 1, "removed": 0, "moved": 0, "changed": 2}));
    assert!(r["diff"]["changed"].to_string().contains("Renamed"), "{r}");
    assert_eq!(x_of(&s, a), 10.0);
    assert_eq!(s.doc().unwrap().get(b).unwrap().name, "B");
    assert_eq!(s.ds().unwrap().history.undo.len(), history);
    assert_eq!(s.execute("history.hash", &json!({})).unwrap(), hash);
}

#[test]
fn preview_of_a_failing_batch_is_the_error_and_leaves_the_document_alone() {
    let (mut s, a, _) = scene();
    let steps = json!([mv(a, 120.0), {"assert": {"noOverlap": {}}}]);
    let e = msg(run(&mut s, json!({"preview": true, "commands": steps})));
    assert!(e.contains("assertion `noOverlap` failed"), "{e}");
    assert_eq!(x_of(&s, a), 10.0);
}

#[test]
fn junk_assertions_and_previews_are_errors_not_panics() {
    let (mut s, _, _) = scene();
    let junk = [
        json!({}),
        json!({"bogus": {}}),
        json!({"noOverflow": 5}),
        json!({"noOverflow": {"zzz": 1}}),
        json!({"noOverflow": {"root": "x"}}),
        json!({"contrast": {"root": []}}),
        json!({"count": {}}),
        json!({"count": {"where": {}}}),
        json!({"count": {"where": 5, "min": 1}}),
        json!({"count": {"where": {}, "min": -1}}),
        json!({"count": {"where": {}, "min": "a"}}),
        json!({"count": {"where": {"type": "nope"}, "min": 1}}),
        json!({"exists": {}}),
        json!({"exists": {"where": {}, "min": 1}}),
        json!(null),
        json!([1]),
    ];
    for j in junk {
        assert!(s.execute("batch.assert", &j).is_err(), "{j}");
        assert!(run(&mut s, json!({"commands": [{"assert": j}]})).is_err(), "{j}");
    }
    for steps in [
        json!([{"assert": 5}]),
        json!([{"assert": {"noOverflow": {}}, "command": "node.set"}]),
        json!([{"assert": {"noOverflow": {}}, "params": {}}]),
        json!([{"assert": null}]),
        json!([{"assert": {}, "extra": 1}]),
    ] {
        assert!(run(&mut s, json!({"commands": steps})).is_err(), "{steps}");
    }
    for preview in [json!("yes"), json!(1), json!([])] {
        assert!(run(&mut s, json!({"preview": preview, "commands": [{"command": "edit.selectAll"}]})).is_err());
    }
}
