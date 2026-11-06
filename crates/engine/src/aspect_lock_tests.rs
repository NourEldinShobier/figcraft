//! Locked aspect ratio through commands: the lock captures the ratio, a size typed on one side
//! moves the other, and an auto layout fill derives the unresolved axis from it.

use serde_json::{Value, json};

use crate::Session;
use crate::params::id_of;
use figcraft_doc::NodeId;

fn id(v: Value) -> NodeId {
    id_of(&v["id"]).unwrap()
}

fn rect(s: &mut Session, parent: Option<NodeId>, w: f64, h: f64) -> NodeId {
    let mut p = json!({"x": 0, "y": 0, "width": w, "height": h});
    if let Some(parent) = parent {
        p["parent"] = json!(parent);
    }
    id(s.execute("shape.rectangle", &p).unwrap())
}

fn set(s: &mut Session, node: NodeId, props: Value) {
    s.execute("node.set", &json!({"id": node, "props": props})).unwrap();
}

fn size(s: &Session, node: NodeId) -> (f64, f64) {
    let n = s.doc().unwrap().node(node).unwrap();
    (n.width, n.height)
}

#[test]
fn a_width_typed_on_a_locked_node_scales_the_height_once() {
    let mut s = Session::new();
    let r = rect(&mut s, None, 60.0, 30.0);
    set(&mut s, r, json!({"constrainProportions": true}));
    assert_eq!(s.doc().unwrap().node(r).unwrap().aspect_ratio, Some(2.0));
    set(&mut s, r, json!({"width": 120}));
    assert_eq!(size(&s, r), (120.0, 60.0));
    set(&mut s, r, json!({"height": 15}));
    assert_eq!(size(&s, r), (30.0, 15.0));
    // Unlocking forgets the ratio and frees the sides.
    set(&mut s, r, json!({"constrainProportions": false}));
    assert_eq!(s.doc().unwrap().node(r).unwrap().aspect_ratio, None);
    set(&mut s, r, json!({"width": 90}));
    assert_eq!(size(&s, r), (90.0, 15.0));
}

#[test]
fn a_locked_fill_child_in_an_auto_layout_frame_follows_the_ratio() {
    let mut s = Session::new();
    let f = id(s.execute("frame.create", &json!({"x": 0, "y": 0, "width": 300, "height": 100})).unwrap());
    set(&mut s, f, json!({"layout": "horizontal", "padding": 0, "gap": 0, "fills": [], "sizingH": "fixed", "sizingV": "hug"}));
    let c = rect(&mut s, Some(f), 200.0, 100.0);
    set(&mut s, c, json!({"constrainProportions": true}));
    set(&mut s, c, json!({"sizingH": "fill"}));
    assert_eq!(size(&s, c), (300.0, 150.0));
    assert_eq!(size(&s, f), (300.0, 150.0));
    set(&mut s, f, json!({"width": 450}));
    assert_eq!(size(&s, c), (450.0, 225.0));
    // Unlocked, the cross size stays as it was.
    set(&mut s, c, json!({"constrainProportions": false}));
    set(&mut s, f, json!({"width": 300}));
    assert_eq!(size(&s, c), (300.0, 225.0));
}
