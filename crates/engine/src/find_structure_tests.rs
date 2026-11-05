//! `node.find`'s structural filters: `page`, `widthNot` / `heightNot`, `outsideParent`, `edgeGap`
//! and `details`. None of them select, switch pages or add history.

use figcraft_doc::*;
use serde_json::{Value, json};

use crate::Session;
use crate::components::tests::ex;

fn add(s: &mut Session, parent: NodeId, kind: NodeKind, name: &str, rect: (f64, f64, f64, f64)) -> NodeId {
    s.doc_mut().unwrap().create(parent, kind, name, rect.0, rect.1, rect.2, rect.3).unwrap()
}

fn rect(s: &mut Session, parent: NodeId, name: &str, r: (f64, f64, f64, f64)) -> NodeId {
    add(s, parent, NodeKind::Rectangle, name, r)
}

fn found(s: &mut Session, params: Value, ids: &[NodeId]) {
    assert_eq!(ex(s, "node.find", params), json!({"ids": ids, "total": ids.len()}));
}

#[test]
fn page_searches_that_page_by_id_name_or_all_and_leaves_the_open_page_and_selection() {
    let mut s = Session::new();
    let page = s.page().unwrap();
    let here = rect(&mut s, page, "Here", (0.0, 0.0, 10.0, 10.0));
    let other = s.doc_mut().unwrap().add_page("Other");
    let there = rect(&mut s, other, "There", (0.0, 0.0, 10.0, 10.0));
    s.set_selection(vec![here]);
    found(&mut s, json!({}), &[here]);
    found(&mut s, json!({"page": "other"}), &[there]);
    found(&mut s, json!({"page": other}), &[there]);
    found(&mut s, json!({"page": other.0.to_string()}), &[there]);
    found(&mut s, json!({"page": "all"}), &[here, there]);
    found(&mut s, json!({"page": "ALL", "name": "there"}), &[there]);
    found(&mut s, json!({"page": "other", "root": page}), &[here]);
    assert!(s.execute("node.find", &json!({"page": "No such page"})).is_err());
    assert!(s.execute("node.find", &json!({"page": here})).is_err());
    assert_eq!(s.page().unwrap(), page);
    assert_eq!(s.selection(), vec![here]);
}

#[test]
fn width_not_and_height_not_drop_every_listed_size_within_a_hundredth() {
    let mut s = Session::new();
    let page = s.page().unwrap();
    let a = rect(&mut s, page, "A", (0.0, 0.0, 100.0, 40.0));
    let b = rect(&mut s, page, "B", (0.0, 0.0, 100.0, 44.0));
    let c = rect(&mut s, page, "C", (0.0, 0.0, 100.0, 44.004));
    let d = rect(&mut s, page, "D", (0.0, 0.0, 80.0, 40.0));
    found(&mut s, json!({"heightNot": 44}), &[a, d]);
    found(&mut s, json!({"heightNot": 44.5}), &[a, b, c, d]);
    found(&mut s, json!({"heightNot": [40, 44]}), &[]);
    found(&mut s, json!({"widthNot": 100}), &[d]);
    found(&mut s, json!({"widthNot": [100], "heightNot": 40}), &[]);
    found(&mut s, json!({"heightNot": 40, "widthNot": 80}), &[b, c]);
    found(&mut s, json!({"heightNot": []}), &[a, b, c, d]);
    assert!(s.execute("node.find", &json!({"heightNot": "tall"})).is_err());
    assert!(s.execute("node.find", &json!({"widthNot": [1, {}]})).is_err());
}

#[test]
fn outside_parent_compares_the_box_with_the_parent_frame_through_groups() {
    let mut s = Session::new();
    let page = s.page().unwrap();
    let f = add(&mut s, page, NodeKind::Frame, "F", (0.0, 0.0, 200.0, 100.0));
    let inside = rect(&mut s, f, "Inside", (10.0, 10.0, 50.0, 50.0));
    let past_right = rect(&mut s, f, "PastRight", (180.0, 10.0, 50.0, 50.0));
    let flush = rect(&mut s, f, "Flush", (150.0, 10.0, 50.0, 50.0));
    let tiny = rect(&mut s, f, "Tiny", (150.3, 10.0, 50.0, 50.0));
    let g = add(&mut s, f, NodeKind::Group, "G", (100.0, 0.0, 100.0, 50.0));
    let in_group = rect(&mut s, g, "InGroup", (50.0, 0.0, 60.0, 30.0));
    let top = rect(&mut s, page, "Top", (500.0, 500.0, 10.0, 10.0));
    found(&mut s, json!({"outsideParent": true}), &[past_right, in_group]);
    found(&mut s, json!({"outsideParent": false}), &[f, inside, flush, tiny, g, top]);
    found(&mut s, json!({"outsideParent": true, "name": "right"}), &[past_right]);
    assert!(s.execute("node.find", &json!({"outsideParent": "maybe"})).is_err());
}

#[test]
fn outside_parent_sees_layers_past_the_left_and_top_edges_but_not_a_full_size_child() {
    let mut s = Session::new();
    let page = s.page().unwrap();
    let f = add(&mut s, page, NodeKind::Frame, "F", (300.0, 300.0, 100.0, 100.0));
    let past_left = rect(&mut s, f, "PastLeft", (-20.0, 10.0, 30.0, 30.0));
    let past_top = rect(&mut s, f, "PastTop", (10.0, -1.0, 30.0, 30.0));
    let _fits = rect(&mut s, f, "Fits", (0.0, 0.0, 100.0, 100.0));
    found(&mut s, json!({"outsideParent": true}), &[past_left, past_top]);
}

#[test]
fn edge_gap_is_the_distance_to_the_parents_same_edge_and_all_filters_must_match() {
    let mut s = Session::new();
    let page = s.page().unwrap();
    let f = add(&mut s, page, NodeKind::Frame, "F", (50.0, 50.0, 200.0, 100.0));
    let a = rect(&mut s, f, "A", (5.0, 70.0, 100.0, 20.0));
    let b = rect(&mut s, f, "B", (0.0, 80.0, 100.0, 20.0));
    let c = rect(&mut s, f, "C", (0.0, 90.0, 100.0, 20.0));
    let d = rect(&mut s, f, "D", (0.0, 10.0, 100.0, 20.0));
    // Bottom gap: parent height - (y + height): A 10, B 0, C -10, D 70.
    found(&mut s, json!({"edgeGap": {"edge": "bottom", "max": 0}}), &[b, c]);
    found(&mut s, json!({"edgeGap": {"edge": "bottom", "min": 0, "max": 10}}), &[a, b]);
    found(&mut s, json!({"edgeGap": {"edge": "bottom", "min": 70}}), &[d]);
    found(&mut s, json!({"edgeGap": {"edge": "Bottom", "min": 10, "max": 10}}), &[a]);
    // Top gap is y: A 70, B 80, C 90, D 10. Right gap: 200 - (x + width): A 95, the others 100.
    found(&mut s, json!({"edgeGap": {"edge": "top", "min": 70, "max": 70}}), &[a]);
    found(&mut s, json!({"edgeGap": {"edge": "right", "max": 99}}), &[a]);
    found(&mut s, json!({"edgeGap": {"edge": "left", "min": 1}}), &[a]);
    found(&mut s, json!({"edgeGap": [{"edge": "bottom", "min": 0, "max": 10}, {"edge": "left", "min": 5}]}), &[a]);
    found(&mut s, json!({"edgeGap": [{"edge": "bottom", "max": 0}, {"edge": "left", "min": 5}]}), &[]);
    // A layer on the page has no parent frame, so no gaps.
    found(&mut s, json!({"edgeGap": {"edge": "top"}, "root": page, "name": "F"}), &[]);
    assert!(s.execute("node.find", &json!({"edgeGap": {"edge": "middle"}})).is_err());
    assert!(s.execute("node.find", &json!({"edgeGap": {"min": 1}})).is_err());
    assert!(s.execute("node.find", &json!({"edgeGap": {"edge": "top", "min": "far"}})).is_err());
    assert!(s.execute("node.find", &json!({"edgeGap": 5})).is_err());
}

#[test]
fn edge_gap_of_a_layer_in_a_group_is_measured_from_the_frame() {
    let mut s = Session::new();
    let page = s.page().unwrap();
    let f = add(&mut s, page, NodeKind::Frame, "F", (0.0, 0.0, 200.0, 100.0));
    let g = add(&mut s, f, NodeKind::Group, "G", (50.0, 60.0, 40.0, 30.0));
    let inner = rect(&mut s, g, "Inner", (5.0, 6.0, 20.0, 10.0));
    // Frame-relative box: x 55, y 66, bottom 76: gaps left 55, top 66, bottom 24.
    found(
        &mut s,
        json!({"edgeGap": [{"edge": "bottom", "min": 24, "max": 24}, {"edge": "left", "min": 55, "max": 55}], "type": "rectangle"}),
        &[inner],
    );
}

#[test]
fn details_returns_the_hits_with_frame_relative_position_and_size_in_the_order_of_ids() {
    let mut s = Session::new();
    let page = s.page().unwrap();
    let f = add(&mut s, page, NodeKind::Frame, "F", (100.0, 200.0, 300.0, 300.0));
    let r = rect(&mut s, f, "R1", (10.0, 20.0, 30.0, 40.0));
    let g = add(&mut s, f, NodeKind::Group, "G", (50.0, 60.0, 40.0, 30.0));
    let r2 = rect(&mut s, g, "R2", (5.0, 6.0, 7.0, 8.0));
    let result = ex(&mut s, "node.find", json!({"name": "R", "details": true}));
    assert_eq!(
        result,
        json!({
            "ids": [r, r2],
            "total": 2,
            "nodes": [
                {"id": r, "name": "R1", "type": "RECTANGLE", "parent": f, "x": 10.0, "y": 20.0, "width": 30.0, "height": 40.0},
                {"id": r2, "name": "R2", "type": "RECTANGLE", "parent": g, "x": 55.0, "y": 66.0, "width": 7.0, "height": 8.0}
            ]
        })
    );
    let limited = ex(&mut s, "node.find", json!({"name": "R", "details": true, "limit": 1}));
    assert_eq!(limited["ids"], json!([r]));
    assert_eq!(limited["total"], json!(2));
    assert_eq!(limited["nodes"].as_array().map(Vec::len), Some(1));
    let top = ex(&mut s, "node.find", json!({"name": "F", "details": true}));
    assert_eq!(top["nodes"][0]["parent"], json!(page));
    assert_eq!(top["nodes"][0]["x"], json!(100.0));
    assert!(ex(&mut s, "node.find", json!({"name": "R"})).get("nodes").is_none());
    assert!(ex(&mut s, "node.find", json!({"name": "R", "details": false})).get("nodes").is_none());
    assert!(s.execute("node.find", &json!({"details": "sometimes"})).is_err());
}
