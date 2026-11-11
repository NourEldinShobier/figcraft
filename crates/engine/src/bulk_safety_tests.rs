//! Bulk-edit safety: the `maxAffected` guard, the `changed` summary, the scope fence and the owner
//! locks (`cmd/safety.rs`, `session/generic.rs`).

use figcraft_doc::NodeId;
use serde_json::{Value, json};

use crate::Session;

const AGENT: Option<&str> = Some("Claude");

fn id(v: &Value) -> NodeId {
    NodeId(v["id"].as_u64().unwrap())
}

fn rect(s: &mut Session, x: i32) -> NodeId {
    id(&s.execute("shape.rectangle", &json!({"x": x, "y": 0, "width": 40, "height": 40})).unwrap())
}

fn rects(s: &mut Session, n: i32) -> Vec<NodeId> {
    (0..n).map(|i| rect(s, i * 50)).collect()
}

fn opacity(s: &Session, id: NodeId) -> f32 {
    s.doc().unwrap().node(id).unwrap().opacity
}

fn set(s: &mut Session, who: Option<&str>, ids: &[NodeId], extra: Value) -> Result<Value, String> {
    let mut p = json!({"ids": ids, "props": {"opacity": 0.5}});
    p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    s.execute_as(who, "node.set", &p).map_err(|e| e.to_string())
}

#[test]
fn max_affected_refuses_a_call_that_would_change_more_layers() {
    let mut s = Session::new();
    let r = rects(&mut s, 5);
    let e = set(&mut s, AGENT, &r, json!({"maxAffected": 3})).unwrap_err();
    assert!(e.contains("5 layers") && e.contains("maxAffected 3") && e.contains("confirm: true"), "{e}");
    assert!(e.contains(&r[0].0.to_string()), "a sample of ids: {e}");
    assert!(r.iter().all(|i| opacity(&s, *i) == 1.0), "nothing changed");
    // At the limit is fine; `confirm` lifts it.
    set(&mut s, AGENT, &r, json!({"maxAffected": 5})).unwrap();
    assert!(r.iter().all(|i| opacity(&s, *i) == 0.5));
    let more = rects(&mut s, 4);
    set(&mut s, AGENT, &more, json!({"maxAffected": 1, "confirm": true})).unwrap();
    assert!(more.iter().all(|i| opacity(&s, *i) == 0.5));
}

#[test]
fn the_guard_counts_a_whole_batch_and_a_script() {
    let mut s = Session::new();
    let r = rects(&mut s, 4);
    let steps: Vec<Value> = r.iter().map(|i| json!({"command": "node.set", "params": {"ids": [i], "props": {"opacity": 0.5}}})).collect();
    let e = s.execute_as(AGENT, "command.batch", &json!({"commands": steps, "maxAffected": 2})).unwrap_err().to_string();
    assert!(e.contains("command.batch would change 4 layers"), "{e}");
    assert!(r.iter().all(|i| opacity(&s, *i) == 1.0));
    let code = "for n in find(#{}) { cmd(\"node.set\", #{ids: [n], props: #{opacity: 0.5}}); }";
    let e = s.execute_as(AGENT, "script.run", &json!({"code": code, "maxAffected": 2})).unwrap_err().to_string();
    assert!(e.contains("script.run would change 4 layers"), "{e}");
    assert!(r.iter().all(|i| opacity(&s, *i) == 1.0));
    s.execute_as(AGENT, "command.batch", &json!({"commands": steps, "maxAffected": 4})).unwrap();
    assert!(r.iter().all(|i| opacity(&s, *i) == 0.5));
}

#[test]
fn creating_a_layer_counts_it_not_its_parent() {
    let mut s = Session::new();
    let f = id(&s.execute("frame.create", &json!({"x": 0, "y": 0, "width": 300, "height": 200})).unwrap());
    let r = s
        .execute_as(AGENT, "shape.rectangle", &json!({"parent": f, "x": 1, "y": 1, "width": 9, "height": 9, "maxAffected": 1, "withChanged": true}))
        .unwrap();
    assert_eq!(r["$changed"]["count"], 1, "{r}");
    assert_eq!(r["$changed"]["created"], 1);
}

#[test]
fn a_read_command_ignores_the_guard_parameters() {
    let mut s = Session::new();
    rects(&mut s, 3);
    let r = s.execute_as(AGENT, "node.find", &json!({"maxAffected": 1, "withChanged": true, "as": "x"})).unwrap();
    assert!(r.get("$changed").is_none(), "{r}");
}

#[test]
fn the_changed_summary_names_layers_props_and_no_ops() {
    let mut s = Session::new();
    let r = rects(&mut s, 12);
    let v = set(&mut s, AGENT, &r, json!({"withChanged": true})).unwrap();
    let c = &v["$changed"];
    assert_eq!(c["count"], 12, "{v}");
    assert_eq!(c["ids"].as_array().map(Vec::len), Some(10), "up to ten ids");
    assert_eq!(c["more"], 2);
    assert_eq!(c["props"], json!(["opacity"]));
    assert!(c.get("noop").is_none());
    // The same write again changes nothing.
    let v = set(&mut s, AGENT, &r[..3], json!({"withChanged": true})).unwrap();
    assert_eq!(v["$changed"]["count"], 0, "{v}");
    assert_eq!(v["$changed"]["noop"], 3);
    assert_eq!(v["result"]["ok"], true, "the command's own result stays under `result`: {v}");
    // Without the option the result is the command's own.
    let v = set(&mut s, AGENT, &r[..3], json!({"maxAffected": 50})).unwrap();
    assert!(v.get("$changed").is_none() && v.get("result").is_none(), "{v}");
}

#[test]
fn junk_guard_parameters_are_refused_and_change_nothing() {
    let mut s = Session::new();
    let r = rects(&mut s, 2);
    for extra in [
        json!({"maxAffected": 0}),
        json!({"maxAffected": -3}),
        json!({"maxAffected": 1.5}),
        json!({"maxAffected": "many"}),
        json!({"maxAffected": [1]}),
        json!({"confirm": "yes"}),
        json!({"as": ""}),
        json!({"as": 7}),
        json!({"as": "x".repeat(101)}),
        json!({"withChanged": 3}),
    ] {
        let e = set(&mut s, AGENT, &r, extra.clone()).unwrap_err();
        assert!(e.contains("must be"), "{extra}: {e}");
        assert!(r.iter().all(|i| opacity(&s, *i) == 1.0), "{extra}");
    }
}

#[test]
fn a_fence_refuses_agent_edits_outside_it_and_names_it() {
    let mut s = Session::new();
    let r = rects(&mut s, 3);
    assert_eq!(s.execute("scope.get", &json!({})).unwrap()["active"], false);
    let v = s.execute("scope.set", &json!({"ids": [r[0]]})).unwrap();
    assert_eq!(v["ids"], json!([r[0]]));
    set(&mut s, AGENT, &r[..1], json!({})).unwrap();
    let e = set(&mut s, AGENT, &r[1..], json!({})).unwrap_err();
    assert!(e.contains("scope fence") && e.contains(&format!("layers {}", r[0].0)) && e.contains(&format!("layer {}", r[1].0)), "{e}");
    assert_eq!(opacity(&s, r[1]), 1.0);
    // A batch with one step outside is rolled back whole.
    let steps = vec![
        json!({"command": "node.set", "params": {"ids": [r[0]], "props": {"opacity": 0.2}}}),
        json!({"command": "node.set", "params": {"ids": [r[2]], "props": {"opacity": 0.2}}}),
    ];
    s.execute_as(AGENT, "command.batch", &json!({"commands": steps})).unwrap_err();
    assert_eq!(opacity(&s, r[0]), 0.5, "the first step was undone with the batch");
    // The person is not fenced.
    s.execute("node.set", &json!({"ids": [r[1]], "props": {"opacity": 0.5}})).unwrap();
    assert_eq!(s.execute("scope.get", &json!({})).unwrap()["ids"], json!([r[0]]));
    assert_eq!(s.execute("scope.clear", &json!({})).unwrap()["cleared"], true);
    assert_eq!(s.execute("scope.clear", &json!({})).unwrap()["cleared"], false);
    set(&mut s, AGENT, &r[2..], json!({})).unwrap();
}

#[test]
fn a_fenced_frame_may_gain_children_and_a_fenced_page_holds_its_layers() {
    let mut s = Session::new();
    let f = id(&s.execute("frame.create", &json!({"x": 0, "y": 0, "width": 300, "height": 200})).unwrap());
    let other = rect(&mut s, 400);
    s.execute("scope.set", &json!({"ids": [f]})).unwrap();
    let child = s.execute_as(AGENT, "shape.rectangle", &json!({"parent": f, "x": 1, "y": 1, "width": 9, "height": 9})).unwrap();
    s.execute_as(AGENT, "node.set", &json!({"ids": [id(&child)], "props": {"opacity": 0.5}})).unwrap();
    let e = s.execute_as(AGENT, "shape.rectangle", &json!({"x": 1, "y": 1, "width": 9, "height": 9})).unwrap_err().to_string();
    assert!(e.contains("scope fence"), "a new layer at the page is outside the frame: {e}");
    s.execute_as(AGENT, "edit.delete", &json!({"ids": [id(&child)]})).unwrap();
    // Pages by name and by id; an unknown one is refused.
    let page = s.page().unwrap();
    let name = s.doc().unwrap().node(page).unwrap().name.clone();
    s.execute("scope.set", &json!({"pages": [name.to_uppercase()]})).unwrap();
    assert_eq!(s.execute("scope.get", &json!({})).unwrap()["pages"][0]["id"], json!(page));
    s.execute_as(AGENT, "node.set", &json!({"ids": [other], "props": {"opacity": 0.5}})).unwrap();
    s.execute("scope.set", &json!({"pages": [page]})).unwrap();
    s.execute("scope.set", &json!({"pages": ["no such page"]})).unwrap_err();
    assert_eq!(s.execute("scope.get", &json!({})).unwrap()["active"], true, "a failed set leaves the fence as it was");
}

#[test]
fn junk_scope_and_lock_parameters_are_refused() {
    let mut s = Session::new();
    let r = rects(&mut s, 1);
    for p in [
        json!({}),
        json!({"ids": "a"}),
        json!({"ids": [1.5]}),
        json!({"ids": [999999]}),
        json!({"pages": 5}),
        json!({"pages": [null]}),
        json!({"pages": {}}),
    ] {
        s.execute("scope.set", &p).unwrap_err();
    }
    for (cmd, p) in [
        ("lock.set", json!({"ids": r})),
        ("lock.set", json!({"ids": r, "owner": ""})),
        ("lock.set", json!({"ids": r, "owner": "x".repeat(101)})),
        ("lock.set", json!({"ids": [999999], "owner": "A"})),
        ("lock.release", json!({"ids": r, "force": "yes"})),
        ("lock.release", json!({"ids": [999999], "owner": "A"})),
        ("lock.list", json!({"limit": 0})),
        ("lock.list", json!({"limit": "x"})),
    ] {
        s.execute(cmd, &p).unwrap_err();
    }
    assert_eq!(s.execute("scope.get", &json!({})).unwrap()["active"], false);
    assert!(!s.doc().unwrap().node(r[0]).unwrap().locked);
}

#[test]
fn a_locked_layer_refuses_other_owners_and_names_the_owner() {
    let mut s = Session::new();
    let r = rects(&mut s, 2);
    let v = s.execute_as(AGENT, "lock.set", &json!({"ids": [r[0]], "owner": "alice"})).unwrap();
    assert_eq!(v["owner"], "alice");
    assert!(s.doc().unwrap().node(r[0]).unwrap().locked);
    let l = s.execute("lock.list", &json!({})).unwrap();
    assert_eq!((l["total"].clone(), l["owners"]["alice"].clone(), l["locks"][0]["id"].clone()), (json!(1), json!(1), json!(r[0])));
    // No label, and another owner's label: refused, naming alice.
    for extra in [json!({}), json!({"as": "bob"})] {
        let e = set(&mut s, AGENT, &r[..1], extra).unwrap_err();
        assert!(e.contains("locked by \"alice\"") && e.contains(&format!("layer {}", r[0].0)), "{e}");
    }
    assert_eq!(opacity(&s, r[0]), 1.0);
    // Unlocked layers, the owner and the person are not stopped.
    set(&mut s, AGENT, &r[1..], json!({"as": "bob"})).unwrap();
    set(&mut s, AGENT, &r[..1], json!({"as": "alice"})).unwrap();
    s.execute("node.set", &json!({"ids": [r[0]], "props": {"opacity": 0.9}})).unwrap();
    // Locks of other owners can be neither taken nor released by an agent.
    let e = s.execute_as(AGENT, "lock.set", &json!({"ids": [r[0]], "owner": "bob"})).unwrap_err().to_string();
    assert!(e.contains("locked by \"alice\""), "{e}");
    let v = s.execute_as(AGENT, "lock.release", &json!({"ids": [r[0]], "as": "bob"})).unwrap();
    assert_eq!((v["released"].clone(), v["skipped"][0]["owner"].clone()), (json!([]), json!("alice")));
    let e = s.execute_as(AGENT, "lock.release", &json!({"ids": [r[0]], "force": true})).unwrap_err().to_string();
    assert!(e.contains("for the person"), "{e}");
    // Unlocking through node.set is an edit like any other.
    set(&mut s, AGENT, &r[..1], json!({"props": {"locked": false}, "as": "bob"})).unwrap_err();
    let v = s.execute_as(AGENT, "lock.release", &json!({"as": "alice", "ids": [r[0]]})).unwrap();
    assert_eq!(v["released"], json!([r[0]]));
    assert!(!s.doc().unwrap().node(r[0]).unwrap().locked);
    set(&mut s, AGENT, &r[..1], json!({"as": "bob"})).unwrap();
    assert_eq!(s.execute("lock.list", &json!({})).unwrap()["total"], 0);
}

#[test]
fn locks_by_owner_release_together_and_the_person_can_force() {
    let mut s = Session::new();
    let r = rects(&mut s, 3);
    s.execute_as(AGENT, "lock.set", &json!({"ids": [r[0], r[1]], "as": "alice"})).unwrap();
    s.execute_as(AGENT, "lock.set", &json!({"ids": [r[2]], "as": "bob"})).unwrap();
    assert_eq!(s.execute("lock.list", &json!({"owner": "bob"})).unwrap()["total"], 1);
    let v = s.execute_as(AGENT, "lock.release", &json!({"owner": "alice"})).unwrap();
    assert_eq!(v["released"], json!([r[0], r[1]]), "no ids: everything alice holds");
    assert!(s.doc().unwrap().node(r[2]).unwrap().locked);
    let v = s.execute("lock.release", &json!({"ids": [r[2]], "force": true})).unwrap();
    assert_eq!(v["released"], json!([r[2]]));
    // A layer locked without an owner (the lock flag alone) stays as it is.
    s.execute("node.set", &json!({"ids": [r[0]], "props": {"locked": true}})).unwrap();
    set(&mut s, AGENT, &r[..1], json!({})).unwrap();
    let v = s.execute_as(AGENT, "lock.release", &json!({"ids": [r[0]], "as": "alice"})).unwrap();
    assert_eq!(v["notLocked"], 1);
}

#[test]
fn normal_calls_are_unchanged_by_the_safety_checks() {
    let mut s = Session::new();
    let r = rects(&mut s, 3);
    set(&mut s, AGENT, &r, json!({})).unwrap();
    assert!(r.iter().all(|i| opacity(&s, *i) == 0.5));
    let v = s.execute_as(AGENT, "node.set", &json!({"ids": [r[0]], "props": {"opacity": 0.25}, "dryRun": true})).unwrap();
    assert_eq!(v["dryRun"], true);
    assert_eq!(opacity(&s, r[0]), 0.5);
    s.execute_as(AGENT, "edit.undo", &json!({})).unwrap();
}
