//! Breakpoints through variable modes (docs/layout-gap-analysis.md gap 1): a frame whose width and
//! padding are bound to number variables with a Mobile and a Desktop mode lays out again, with its
//! auto layout descendants, when the mode changes (on the frame or on the page), in one undo step.

use serde_json::{Value, json};

use crate::Session;
use crate::params::id_of;
use figcraft_doc::NodeId;

fn id(v: Value) -> NodeId {
    id_of(&v["id"]).unwrap()
}

fn run(s: &mut Session, cmd: &str, p: Value) -> Value {
    s.execute(cmd, &p).unwrap_or_else(|e| panic!("{cmd} {p}: {e}"))
}

fn width(s: &Session, n: NodeId) -> f64 {
    s.doc().unwrap().node(n).unwrap().width
}

fn x(s: &Session, n: NodeId) -> f64 {
    s.doc().unwrap().node(n).unwrap().transform.as_coeffs()[4]
}

/// Collection `Screen` (Mobile first = the default, Desktop) with number variables `w` (375 / 1440)
/// and `pad` (8 / 24).
fn screen(s: &mut Session) {
    run(s, "variable.createCollection", json!({"name": "Screen", "modes": ["Mobile", "Desktop"]}));
    run(s, "variable.create", json!({"collection": "Screen", "name": "w", "type": "number", "values": {"Mobile": 375, "Desktop": 1440}}));
    run(s, "variable.create", json!({"collection": "Screen", "name": "pad", "type": "number", "values": {"Mobile": 8, "Desktop": 24}}));
}

/// Outer frame (horizontal auto layout, padding 16, width bound to `w`) holding an inner frame
/// (Fill width, padding bound to `pad`) with one Fill rectangle; the outer frame also holds a Fill
/// rectangle of its own, so each width is its parent's less padding.
fn page(s: &mut Session) -> (NodeId, NodeId, NodeId, NodeId) {
    screen(s);
    let outer = id(run(s, "frame.create", json!({"x": 0, "y": 0, "width": 375, "height": 400})));
    run(s, "node.set", json!({"id": outer, "props": {"layout": "vertical", "padding": 16, "gap": 0, "fills": []}}));
    let direct = id(run(s, "shape.rectangle", json!({"parent": outer, "x": 0, "y": 0, "width": 50, "height": 20})));
    run(s, "node.set", json!({"id": direct, "props": {"sizingH": "fill"}}));
    let inner = id(run(s, "frame.create", json!({"parent": outer, "x": 0, "y": 0, "width": 100, "height": 60})));
    run(s, "node.set", json!({"id": inner, "props": {"layout": "vertical", "padding": 0, "gap": 0, "fills": [], "sizingH": "fill"}}));
    let leaf = id(run(s, "shape.rectangle", json!({"parent": inner, "x": 0, "y": 0, "width": 10, "height": 10})));
    run(s, "node.set", json!({"id": leaf, "props": {"sizingH": "fill"}}));
    run(s, "variable.bind", json!({"ids": [outer], "field": "width", "variable": "w"}));
    run(s, "variable.bind", json!({"ids": [inner], "field": "paddingHorizontal", "variable": "pad"}));
    (outer, direct, inner, leaf)
}

#[test]
fn the_default_mode_applies_when_the_variable_is_bound() {
    let mut s = Session::new();
    let (outer, direct, inner, leaf) = page(&mut s);
    assert_eq!(width(&s, outer), 375.0);
    assert_eq!(width(&s, direct), 343.0, "375 - 2 x 16");
    assert_eq!(width(&s, inner), 343.0);
    assert_eq!(width(&s, leaf), 327.0, "343 - 2 x 8");
}

#[test]
fn switching_the_frame_mode_lays_out_the_frame_and_its_descendants_in_one_undo_step() {
    let mut s = Session::new();
    let (outer, direct, inner, leaf) = page(&mut s);
    run(&mut s, "variable.setMode", json!({"ids": [outer], "collection": "Screen", "mode": "Desktop"}));
    assert_eq!(width(&s, outer), 1440.0);
    assert_eq!(width(&s, direct), 1408.0, "1440 - 2 x 16");
    assert_eq!(width(&s, inner), 1408.0);
    assert_eq!(width(&s, leaf), 1360.0, "1408 - 2 x 24");
    assert_eq!(x(&s, leaf), 24.0, "the bound padding moved it");
    run(&mut s, "variable.setMode", json!({"ids": [outer], "collection": "Screen", "mode": "Mobile"}));
    assert_eq!((width(&s, direct), width(&s, leaf)), (343.0, 327.0));
    // One undo step per switch: back to Desktop, then to the start.
    s.undo().unwrap();
    assert_eq!((width(&s, outer), width(&s, direct), width(&s, leaf)), (1440.0, 1408.0, 1360.0));
    s.undo().unwrap();
    assert_eq!((width(&s, outer), width(&s, direct), width(&s, leaf)), (375.0, 343.0, 327.0));
}

#[test]
fn a_mode_set_on_the_page_reaches_frames_on_auto_and_an_explicit_frame_mode_wins() {
    let mut s = Session::new();
    let (outer, direct, _, leaf) = page(&mut s);
    let pg = s.page().unwrap();
    run(&mut s, "variable.setMode", json!({"ids": [pg], "collection": "Screen", "mode": "Desktop"}));
    assert_eq!((width(&s, outer), width(&s, direct), width(&s, leaf)), (1440.0, 1408.0, 1360.0), "Auto follows the page");
    // An explicit mode on the frame beats the page's.
    run(&mut s, "variable.setMode", json!({"ids": [outer], "collection": "Screen", "mode": "Mobile"}));
    assert_eq!((width(&s, outer), width(&s, direct), width(&s, leaf)), (375.0, 343.0, 327.0));
    // Back to Auto (null): the page's Desktop again, as one step.
    run(&mut s, "variable.setMode", json!({"ids": [outer], "collection": "Screen", "mode": null}));
    assert_eq!((width(&s, outer), width(&s, direct), width(&s, leaf)), (1440.0, 1408.0, 1360.0));
    s.undo().unwrap();
    assert_eq!((width(&s, outer), width(&s, direct), width(&s, leaf)), (375.0, 343.0, 327.0));
}

#[test]
fn a_mode_set_on_an_inner_frame_only_re_lays_out_that_frame() {
    let mut s = Session::new();
    let (outer, direct, inner, leaf) = page(&mut s);
    run(&mut s, "variable.setMode", json!({"ids": [inner], "collection": "Screen", "mode": "Desktop"}));
    assert_eq!(width(&s, outer), 375.0, "the outer frame stays on Mobile");
    assert_eq!(width(&s, direct), 343.0);
    assert_eq!(width(&s, leaf), 295.0, "343 - 2 x 24 from the inner frame's Desktop padding");
}
