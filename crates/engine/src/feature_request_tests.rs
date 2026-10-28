//! Fixes from the 2026-10 feature-request triage, one test per request.

use serde_json::{Value, json};

use crate::Session;

fn ex(s: &mut Session, cmd: &str, p: Value) -> Value {
    s.execute(cmd, &p).unwrap_or_else(|e| panic!("{cmd}: {e}"))
}

fn id(v: &Value) -> crate::NodeId {
    crate::params::id_of(v).unwrap()
}

#[test]
fn empty_node_set_is_a_no_op_inside_a_batch_only() {
    let mut s = Session::new();
    let r = id(&ex(&mut s, "node.create", json!({"type": "rectangle"}))["id"]);
    assert!(s.execute("node.set", &json!({"ids": [r]})).is_err());
    let out = ex(&mut s, "command.batch", json!({"commands": [{"command": "node.set", "params": {"ids": [r]}}]}));
    assert_eq!(out.as_array().map(Vec::len), Some(1));
}

fn frame(s: &mut Session, extra: Value) -> crate::NodeId {
    let mut p = json!({"type": "frame", "x": 0, "y": 0, "width": 200, "height": 200});
    if let (Some(o), Some(e)) = (p.as_object_mut(), extra.as_object()) {
        o.extend(e.clone());
    }
    id(&ex(s, "node.create", p)["id"])
}

fn rect(s: &mut Session, parent: crate::NodeId, name: &str) -> crate::NodeId {
    id(&ex(s, "node.create", json!({"type": "rectangle", "parent": parent, "name": name, "width": 10, "height": 10}))["id"])
}

#[test]
fn a_width_held_back_by_minwidth_is_warned_about() {
    let mut s = Session::new();
    let f = frame(&mut s, json!({"props": {"layout": "horizontal", "sizingH": "hug", "minWidth": 200}}));
    let r = ex(&mut s, "node.set", json!({"ids": [f], "props": {"width": 100}}));
    let w = r["warnings"][0].as_str().unwrap_or_default();
    assert!(w.contains("`width` 100 is held to 200") && w.contains("minWidth is 200"), "{r}");
}

#[test]
fn frames_with_layout_made_in_auto_layout_hug_vertically() {
    let mut s = Session::new();
    let list = frame(&mut s, json!({"props": {"layout": "vertical"}}));
    let col = id(&ex(&mut s, "node.create", json!({"type": "frame", "parent": list, "props": {"layout": "vertical"}}))["id"]);
    let fixed = id(&ex(&mut s, "node.create", json!({"type": "frame", "parent": list, "height": 40, "props": {"layout": "vertical"}}))["id"]);
    let doc = s.doc().unwrap();
    assert_eq!(doc.get(col).unwrap().child_layout.sizing_v, figcraft_doc::Sizing::Hug);
    assert_eq!(doc.get(fixed).unwrap().child_layout.sizing_v, figcraft_doc::Sizing::Fixed);
}

#[test]
fn character_ranges_style_part_of_a_text() {
    let mut s = Session::new();
    let t = id(&ex(&mut s, "node.create", json!({"type": "text", "text": "Hello world"}))["id"]);
    ex(&mut s, "text.setRange", json!({"id": t, "start": 0, "end": 5, "fill": "#FF0000", "weight": 700}));
    ex(&mut s, "node.set", json!({"id": t, "props": {"characters": "Hello brave world", "ranges": [{"start": 6, "end": 11, "fontSize": 30}]}}));
    let doc = s.doc().unwrap();
    let td = doc.get(t).unwrap().kind.text().unwrap();
    assert_eq!(td.characters, "Hello brave world");
    assert_eq!(td.style_at(6).font_size, 30.0);
    assert_ne!(td.style_at(0).font_size, 30.0);
    // Bad offsets are refused.
    assert!(s.execute("text.setRange", &json!({"id": t, "start": 5, "end": 99, "weight": 700})).is_err());
    assert!(s.execute("text.setRange", &json!({"id": t, "start": 0, "end": 3, "bogus": 1})).is_err());
}

#[test]
fn batch_rename_finds_and_replaces_in_layer_names() {
    let mut s = Session::new();
    let a = frame(&mut s, json!({"name": "row Copy Folder Path"}));
    ex(&mut s, "object.batchRename", json!({"ids": [a], "find": "Copy Folder Path", "replace": "Paste"}));
    assert_eq!(s.doc().unwrap().get(a).unwrap().name, "row Paste");
}

#[test]
fn swapping_a_resized_instance_keeps_its_size() {
    let mut s = Session::new();
    let mut comps = Vec::new();
    for x in [0, 300] {
        let f = id(&ex(&mut s, "frame.create", json!({"x": x, "y": 0, "width": 24, "height": 24}))["id"]);
        ex(&mut s, "shape.rectangle", json!({"x": 0, "y": 0, "width": 24, "height": 24, "parent": f}));
        comps.push(id(&ex(&mut s, "component.create", json!({"ids": [f]}))["id"]));
    }
    let i = id(&ex(&mut s, "instance.create", json!({"component": comps[0], "x": 0, "y": 100}))["id"]);
    // Sized without a recorded override, as a scale of the instance leaves it.
    for dim in [true, false] {
        let n = s.doc_mut().unwrap().get_mut(i).unwrap();
        if dim { n.width = 18.0 } else { n.height = 18.0 }
    }
    ex(&mut s, "instance.swap", json!({"ids": [i], "component": comps[1]}));
    let n = ex(&mut s, "node.get", json!({"id": i}));
    assert_eq!((n["width"].as_f64(), n["height"].as_f64()), (Some(18.0), Some(18.0)));
}

#[test]
fn export_many_refuses_unknown_keys_and_can_flatten_slashes() {
    let mut s = Session::new();
    let f = frame(&mut s, json!({"name": "state/win/dark"}));
    let err = s.execute("file.exportMany", &json!({"ids": [f], "items": []})).unwrap_err().to_string();
    assert!(err.contains("unknown key `items`"), "{err}");
    let nested = ex(&mut s, "file.exportMany", json!({"ids": [f]}));
    assert_eq!(nested["files"][0]["name"], "state/win/dark.png");
    let flat = ex(&mut s, "file.exportMany", json!({"ids": [f], "flatten": true}));
    assert_eq!(flat["files"][0]["name"], "state_win_dark.png");
}

#[test]
fn export_creates_missing_parent_folders() {
    let mut s = Session::new();
    let f = frame(&mut s, json!({}));
    let dir = std::env::temp_dir().join(format!("figcraft-fr-{}", std::process::id()));
    let path = dir.join("a").join("b").join("out.png");
    ex(&mut s, "file.export", json!({"ids": [f], "path": path.to_string_lossy()}));
    assert!(path.is_file());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn duplicate_reports_an_id_map_and_repeated_copies_keep_order_in_auto_layout() {
    let mut s = Session::new();
    let list = frame(&mut s, json!({"props": {"layout": "vertical"}}));
    let card = id(&ex(&mut s, "node.create", json!({"type": "frame", "parent": list, "name": "card", "width": 50, "height": 20}))["id"]);
    let inner = rect(&mut s, card, "inner");
    let d1 = ex(&mut s, "edit.duplicate", json!({"ids": [card]}));
    let c1 = id(&d1["ids"][0]);
    assert_eq!(d1["idMap"].as_object().map(|m| m.len()), Some(2));
    let c1_inner = id(&d1["idMap"][inner.to_string()]);
    assert_eq!(s.doc().unwrap().get(c1_inner).unwrap().parent, Some(c1));
    let c2 = id(&ex(&mut s, "edit.duplicate", json!({"ids": [card]}))["ids"][0]);
    assert_eq!(s.doc().unwrap().get(list).unwrap().children, [card, c1, c2]);
    // `index` places the copy.
    let c3 = id(&ex(&mut s, "edit.duplicate", json!({"ids": [card], "index": 0}))["ids"][0]);
    assert_eq!(s.doc().unwrap().get(list).unwrap().children, [c3, card, c1, c2]);
}

#[test]
fn reparent_can_keep_the_position_relative_to_the_new_parent() {
    let mut s = Session::new();
    let target = frame(&mut s, json!({"x": 300, "y": 300}));
    let r = id(&ex(&mut s, "node.create", json!({"type": "rectangle", "x": 20, "y": 30, "width": 10, "height": 10}))["id"]);
    ex(&mut s, "node.reparent", json!({"ids": [r], "parent": target, "keepRelative": true}));
    let n = ex(&mut s, "node.get", json!({"id": r}));
    assert_eq!((n["x"].as_f64(), n["y"].as_f64()), (Some(20.0), Some(30.0)));
    let r2 = id(&ex(&mut s, "node.create", json!({"type": "rectangle", "x": 20, "y": 30, "width": 10, "height": 10}))["id"]);
    ex(&mut s, "node.reparent", json!({"ids": [r2], "parent": target}));
    let n = ex(&mut s, "node.get", json!({"id": r2}));
    assert_eq!((n["x"].as_f64(), n["y"].as_f64()), (Some(-280.0), Some(-270.0)));
}

#[test]
fn create_can_leave_the_auto_layout_flow_and_instances_can_name_a_parent() {
    let mut s = Session::new();
    let list = frame(&mut s, json!({"props": {"layout": "vertical"}}));
    let badge = id(&ex(
        &mut s,
        "node.create",
        json!({"type": "rectangle", "parent": list, "x": 150, "y": 5, "width": 10, "height": 10, "absolute": true}),
    )["id"]);
    assert!(s.doc().unwrap().get(badge).unwrap().child_layout.absolute);

    let comp = id(&ex(&mut s, "node.create", json!({"type": "component", "x": 500, "y": 500, "width": 20, "height": 20}))["id"]);
    let host = frame(&mut s, json!({"x": 1000, "y": 0}));
    // The point is on the page, outside `host`; `parent` decides.
    let placed = ex(&mut s, "instance.place", json!({"component": comp, "x": 5000, "y": 5000, "parent": host}));
    assert_eq!(id(&placed["parent"]), host);
}
