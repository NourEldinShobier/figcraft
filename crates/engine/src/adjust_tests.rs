//! `node.adjust`: each number is added to every layer's own current value.

use figcraft_doc::*;
use serde_json::json;

use crate::Session;
use crate::components::tests::ex;

fn add(s: &mut Session, parent: NodeId, kind: NodeKind, name: &str, [x, y, w, h]: [f64; 4]) -> NodeId {
    s.doc_mut().unwrap().create(parent, kind, name, x, y, w, h).unwrap()
}

fn node(s: &Session, id: NodeId) -> Node {
    s.doc().unwrap().node(id).unwrap().clone()
}

/// x, y, width, height.
fn geometry(s: &Session, id: NodeId) -> [f64; 4] {
    let n = node(s, id);
    let c = n.transform.as_coeffs();
    [c[4], c[5], n.width, n.height]
}

/// Three rectangles with different positions and sizes.
fn three() -> (Session, [NodeId; 3]) {
    let mut s = Session::new();
    let page = s.page().unwrap();
    let a = add(&mut s, page, NodeKind::Rectangle, "A", [0.0, 0.0, 100.0, 50.0]);
    let b = add(&mut s, page, NodeKind::Rectangle, "B", [30.0, 10.0, 80.0, 20.0]);
    let c = add(&mut s, page, NodeKind::Rectangle, "C", [5.0, 5.0, 10.0, 10.0]);
    (s, [a, b, c])
}

#[test]
fn every_layer_changes_from_its_own_value() {
    let (mut s, [a, b, c]) = three();
    let r = ex(&mut s, "node.adjust", json!({"ids": [a, b, c], "dx": 10, "dy": -5, "dw": 20, "dh": -5}));
    assert_eq!(r, json!({"changed": [a, b, c], "skipped": []}));
    assert_eq!(geometry(&s, a), [10.0, -5.0, 120.0, 45.0]);
    assert_eq!(geometry(&s, b), [40.0, 5.0, 100.0, 15.0]);
    assert_eq!(geometry(&s, c), [15.0, 0.0, 30.0, 5.0]);
}

#[test]
fn opacity_corners_and_rotation_add_to_each_layers_value() {
    let (mut s, [a, b, _]) = three();
    ex(&mut s, "node.set", json!({"ids": [a], "props": {"opacity": 0.75, "cornerRadius": 4}}));
    ex(&mut s, "node.set", json!({"ids": [b], "props": {"opacity": 0.5, "cornerRadius": [1, 2, 3, 4]}}));
    ex(&mut s, "node.adjust", json!({"ids": [a, b], "opacity": -0.25, "cornerRadius": 2}));
    assert_eq!((node(&s, a).opacity, node(&s, a).corners.radii), (0.5, [6.0; 4]));
    assert_eq!((node(&s, b).opacity, node(&s, b).corners.radii), (0.25, [3.0, 4.0, 5.0, 6.0]));
    // More than 1 is a percentage; the result stays between 0 and 1.
    ex(&mut s, "node.adjust", json!({"ids": [a, b], "opacity": 100}));
    assert_eq!((node(&s, a).opacity, node(&s, b).opacity), (1.0, 1.0));

    ex(&mut s, "node.adjust", json!({"ids": [a], "rotation": 90}));
    ex(&mut s, "node.adjust", json!({"ids": [a], "rotation": 15}));
    let degrees = figcraft_geom::rotation_degrees(node(&s, a).transform);
    assert!((degrees - 105.0).abs() < 1e-6, "{degrees}");
}

#[test]
fn layers_a_property_does_not_apply_to_are_listed_with_why() {
    let mut s = Session::new();
    let page = s.page().unwrap();
    let frame = add(&mut s, page, NodeKind::Frame, "Frame", [0.0, 0.0, 200.0, 200.0]);
    let text = add(&mut s, page, NodeKind::Text(Box::new(TextData::new("Hi"))), "Text", [0.0, 0.0, 50.0, 20.0]);
    let rect = add(&mut s, page, NodeKind::Rectangle, "Rect", [0.0, 0.0, 10.0, 10.0]);
    ex(&mut s, "node.set", json!({"ids": [text], "props": {"fontSize": 20}}));
    let r = ex(&mut s, "node.adjust", json!({"ids": [text, rect, frame], "fontSize": 4, "gap": 2}));
    let not_text = "`fontSize`: only text layers have a font size; `gap`: only auto layout frames have one";
    assert_eq!(
        r,
        json!({"changed": [text], "skipped": [
            {"id": text, "why": "`gap`: only auto layout frames have one"},
            {"id": rect, "why": not_text},
            {"id": frame, "why": not_text},
        ]})
    );
    let size = |s: &Session| node(s, text).kind.text().map(|t| t.style.font_size);
    assert_eq!(size(&s), Some(24.0));
    // A negative result stays at the smallest size.
    ex(&mut s, "node.adjust", json!({"ids": [text], "fontSize": -100}));
    assert_eq!(size(&s), Some(1.0));
}

#[test]
fn gap_and_padding_of_auto_layout_frames_and_children_the_layout_places() {
    let mut s = Session::new();
    let page = s.page().unwrap();
    let frame = add(&mut s, page, NodeKind::Frame, "Stack", [0.0, 0.0, 200.0, 200.0]);
    let child = add(&mut s, frame, NodeKind::Rectangle, "Child", [0.0, 0.0, 20.0, 20.0]);
    ex(&mut s, "node.set", json!({"ids": [frame], "props": {"layout": "vertical", "gap": 8, "padding": 4}}));
    let r = ex(&mut s, "node.adjust", json!({"ids": [frame], "gap": 2, "padding": 1}));
    assert_eq!(r, json!({"changed": [frame], "skipped": []}));
    let layout = node(&s, frame).layout.unwrap();
    assert_eq!((layout.gap, layout.padding), (10.0, [5.0; 4]));
    ex(&mut s, "node.adjust", json!({"ids": [frame], "padding": -10}));
    assert_eq!(node(&s, frame).layout.unwrap().padding, [0.0; 4], "padding stops at zero");

    let r = ex(&mut s, "node.adjust", json!({"ids": [child], "dx": 5}));
    assert_eq!(r, json!({"changed": [], "skipped": [{"id": child, "why": "`dx`: the auto layout of its parent positions it"}]}));
}

#[test]
fn resizing_applies_constraints_like_node_set() {
    let build = || {
        let mut s = Session::new();
        let page = s.page().unwrap();
        let frame = add(&mut s, page, NodeKind::Frame, "Frame", [0.0, 0.0, 200.0, 100.0]);
        let child = add(&mut s, frame, NodeKind::Rectangle, "Pinned right", [150.0, 10.0, 40.0, 20.0]);
        ex(&mut s, "node.set", json!({"ids": [child], "props": {"constraints": {"horizontal": "right"}}}));
        (s, frame, child)
    };
    let (mut by_set, frame, child) = build();
    ex(&mut by_set, "node.set", json!({"ids": [frame], "props": {"width": 250}}));
    let (mut by_adjust, frame, child_b) = build();
    assert_eq!(child, child_b);
    ex(&mut by_adjust, "node.adjust", json!({"ids": [frame], "dw": 50}));
    assert_eq!(geometry(&by_adjust, child), geometry(&by_set, child));
    assert_eq!(geometry(&by_adjust, child), [200.0, 10.0, 40.0, 20.0], "the right-pinned child followed the edge");
}

#[test]
fn one_undo_step_for_the_whole_call_and_where_finds_the_layers() {
    let (mut s, [a, b, c]) = three();
    let r = ex(&mut s, "node.adjust", json!({"where": {"type": "RECTANGLE"}, "dx": 1, "dw": 2, "opacity": -0.5}));
    assert_eq!((r["changed"].clone(), r["matched"].clone()), (json!([a, b, c]), json!(3)));
    assert_eq!(s.undo_depth(), (1, 0));
    assert_eq!(geometry(&s, b), [31.0, 10.0, 82.0, 20.0]);
    ex(&mut s, "edit.undo", json!({}));
    assert_eq!(geometry(&s, a), [0.0, 0.0, 100.0, 50.0]);
    assert_eq!(geometry(&s, b), [30.0, 10.0, 80.0, 20.0]);
    assert_eq!(geometry(&s, c), [5.0, 5.0, 10.0, 10.0]);
    assert_eq!(node(&s, a).opacity, 1.0);
}

#[test]
fn bad_values_and_a_call_without_adjustments_are_refused_and_change_nothing() {
    let (mut s, [a, ..]) = three();
    for p in [json!({"ids": [a]}), json!({"ids": [a], "dx": "far"}), json!({"ids": [a], "dw": null}), json!({"ids": [a], "dx": 1, "dz": 1})] {
        assert!(s.execute("node.adjust", &p).is_err(), "{p}");
    }
    // A zero adjustment is accepted and changes nothing.
    let r = ex(&mut s, "node.adjust", json!({"ids": [a], "dx": 0, "dw": 0}));
    assert_eq!(r, json!({"changed": [], "skipped": []}));
    assert_eq!(s.undo_depth(), (0, 0));
    assert_eq!(geometry(&s, a), [0.0, 0.0, 100.0, 50.0]);
}
