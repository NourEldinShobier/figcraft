//! Documented auto layout rules that had no test (docs/layout-gap-analysis.md gaps 5, 6, 7): fixed
//! and sticky scroll positions need a layer that ignores auto layout, a child frame's stroke
//! setting is its own, and an instance takes padding overrides but no reordering.

use serde_json::{Value, json};

use crate::Session;
use crate::params::id_of;
use figcraft_doc::NodeId;
use figcraft_doc::proto::ScrollPosition;

fn id(v: Value) -> NodeId {
    id_of(&v["id"]).unwrap()
}

fn run(s: &mut Session, cmd: &str, p: Value) -> Value {
    s.execute(cmd, &p).unwrap_or_else(|e| panic!("{cmd} {p}: {e}"))
}

fn set(s: &mut Session, node: NodeId, props: Value) {
    run(s, "node.set", json!({"id": node, "props": props}));
}

fn size(s: &Session, node: NodeId) -> (f64, f64) {
    let n = s.doc().unwrap().node(node).unwrap();
    (n.width, n.height)
}

fn pos(s: &Session, node: NodeId) -> (f64, f64) {
    let c = s.doc().unwrap().node(node).unwrap().transform.as_coeffs();
    (c[4], c[5])
}

fn scroll_position(s: &Session, node: NodeId) -> ScrollPosition {
    s.doc().unwrap().node(node).unwrap().scroll_position
}

fn rect(s: &mut Session, parent: NodeId, w: f64, h: f64) -> NodeId {
    id(run(s, "shape.rectangle", json!({"parent": parent, "x": 0, "y": 0, "width": w, "height": h})))
}

#[test]
fn fixed_and_sticky_need_a_layer_that_ignores_auto_layout() {
    // Gap 5: a flow child of an auto layout frame can't be fixed or sticky.
    let mut s = Session::new();
    let f = id(run(&mut s, "frame.create", json!({"x": 0, "y": 0, "width": 200, "height": 100})));
    set(&mut s, f, json!({"layout": "horizontal", "padding": 0, "gap": 0}));
    let r = rect(&mut s, f, 20.0, 20.0);
    for v in ["fixed", "sticky"] {
        let e = s.execute("node.set", &json!({"id": r, "props": {"scrollPosition": v}})).unwrap_err().to_string();
        assert!(e.contains("ignores auto layout") && e.contains("absolute"), "{v}: {e}");
        assert_eq!(scroll_position(&s, r), ScrollPosition::Scrolls, "{v}: refused, nothing changed");
    }
    // Scrolling with the parent is always allowed.
    set(&mut s, r, json!({"scrollPosition": "scrolls"}));
    // Ignoring auto layout first makes it succeed; so does asking for both in one call.
    set(&mut s, r, json!({"absolute": true}));
    set(&mut s, r, json!({"scrollPosition": "fixed"}));
    assert_eq!(scroll_position(&s, r), ScrollPosition::Fixed);
    set(&mut s, r, json!({"scrollPosition": "sticky"}));
    assert_eq!(scroll_position(&s, r), ScrollPosition::Sticky);
    let q = rect(&mut s, f, 20.0, 20.0);
    set(&mut s, q, json!({"absolute": true, "scrollPosition": "fixed"}));
    assert_eq!(scroll_position(&s, q), ScrollPosition::Fixed);
}

#[test]
fn fixed_is_free_in_a_frame_without_auto_layout() {
    let mut s = Session::new();
    let f = id(run(&mut s, "frame.create", json!({"x": 0, "y": 0, "width": 200, "height": 100})));
    let r = rect(&mut s, f, 20.0, 20.0);
    set(&mut s, r, json!({"scrollPosition": "fixed"}));
    assert_eq!(scroll_position(&s, r), ScrollPosition::Fixed);
}

#[test]
fn a_child_frames_stroke_setting_is_its_own() {
    // Gap 6 (D): "strokes included" belongs to each frame, not to its parent.
    let mut s = Session::new();
    let p = id(run(&mut s, "frame.create", json!({"x": 0, "y": 0, "width": 200, "height": 100})));
    set(
        &mut s,
        p,
        json!({"layout": "horizontal", "padding": 0, "gap": 0, "fills": [], "strokesIncluded": true, "stroke": "#000000", "strokeWeight": 4, "strokeAlign": "inside"}),
    );
    let c = id(run(&mut s, "frame.create", json!({"parent": p, "x": 0, "y": 0, "width": 50, "height": 20})));
    rect(&mut s, c, 50.0, 20.0);
    set(
        &mut s,
        c,
        json!({"layout": "horizontal", "padding": 0, "gap": 0, "fills": [], "sizingH": "hug", "sizingV": "hug",
               "strokesIncluded": false, "stroke": "#000000", "strokeWeight": 10, "strokeAlign": "inside"}),
    );
    assert_eq!(pos(&s, c), (4.0, 4.0), "the parent's included stroke takes 4 on each side");
    assert_eq!(size(&s, c), (50.0, 20.0), "the child's own stroke is excluded");
    set(&mut s, c, json!({"strokesIncluded": true}));
    assert_eq!(size(&s, c), (70.0, 40.0), "included: 10 more on each side");
    assert_eq!(pos(&s, c), (4.0, 4.0));
    assert_eq!(size(&s, p), (200.0, 100.0));
    // Excluding the parent's stroke leaves the child's alone.
    set(&mut s, p, json!({"strokesIncluded": false}));
    assert_eq!(pos(&s, c), (0.0, 0.0));
    assert_eq!(size(&s, c), (70.0, 40.0));
}

#[test]
fn an_instance_takes_a_padding_override_but_no_reordering() {
    // Gap 7: padding and gap of an instance's auto layout are overridable; the order of its layers is the main's.
    let mut s = Session::new();
    let f = id(run(&mut s, "frame.create", json!({"x": 0, "y": 0, "width": 100, "height": 40, "name": "Chip"})));
    rect(&mut s, f, 50.0, 20.0);
    rect(&mut s, f, 10.0, 20.0);
    set(&mut s, f, json!({"layout": "horizontal", "padding": 10, "gap": 0, "sizingH": "hug", "sizingV": "hug"}));
    let main = id(run(&mut s, "component.create", json!({"ids": [f]})));
    let i = id(run(&mut s, "instance.create", json!({"component": main, "x": 0, "y": 100})));
    assert_eq!(size(&s, i), (80.0, 40.0), "10 + 50 + 10 + 10 by 10 + 20 + 10");
    set(&mut s, i, json!({"padding": 24}));
    assert_eq!(size(&s, i), (108.0, 68.0), "28 wider and taller: 2 x 14");
    // Reordering the layers inside the instance is refused, and nothing changes.
    let first = s.doc().unwrap().node(i).unwrap().children[0];
    let before = s.doc().unwrap().clone();
    assert!(s.execute("node.reorder", &json!({"id": first, "index": 1})).is_err());
    assert!(s.execute("arrange.front", &json!({"ids": [first]})).is_err());
    assert_eq!(*s.doc().unwrap(), before);
}
