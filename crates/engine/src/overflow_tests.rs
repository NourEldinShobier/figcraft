//! `inspect.overflow`: layers cut off or sticking out of their parent, and text that doesn't fit.
//! Read-only: no selection, page or history change.

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

fn shadow(s: &mut Session, id: NodeId, offset: (f64, f64), radius: f64) {
    let kind = EffectKind::DropShadow(Shadow { offset, radius, ..Shadow::default() });
    s.doc_mut().unwrap().get_mut(id).unwrap().effects.push(Effect { kind, visible: true });
}

fn item(id: NodeId, name: &str, parent: NodeId, parent_name: &str, kind: &str, detail: &str) -> Value {
    json!({"id": id, "name": name, "parent": parent, "parentName": parent_name, "kind": kind, "detail": detail})
}

fn overflow(s: &mut Session, params: Value) -> Value {
    ex(s, "inspect.overflow", params)
}

#[test]
fn a_child_past_a_clipping_frame_is_clipped_and_past_a_plain_one_is_just_outside() {
    let mut s = Session::new();
    let page = s.page().unwrap();
    let f = add(&mut s, page, NodeKind::Frame, "F", (0.0, 0.0, 200.0, 100.0));
    let out = rect(&mut s, f, "Out", (150.0, 10.0, 100.0, 50.0));
    let _inside = rect(&mut s, f, "Inside", (10.0, 10.0, 50.0, 50.0));
    s.set_selection(vec![out]);
    assert_eq!(
        overflow(&mut s, json!({})),
        json!({"items": [item(out, "Out", f, "F", "outside-parent-clipped", "50 past right")], "total": 1, "truncated": false})
    );
    s.doc_mut().unwrap().get_mut(f).unwrap().clip = false;
    assert_eq!(
        overflow(&mut s, json!({})),
        json!({"items": [item(out, "Out", f, "F", "outside-parent", "50 past right")], "total": 1, "truncated": false})
    );
    assert_eq!(s.selection(), vec![out]);
}

#[test]
fn every_side_it_passes_is_named_in_the_order_left_top_right_bottom() {
    let mut s = Session::new();
    let page = s.page().unwrap();
    let f = add(&mut s, page, NodeKind::Frame, "F", (0.0, 0.0, 200.0, 100.0));
    let corner = rect(&mut s, f, "Corner", (-10.0, 80.0, 30.0, 30.0));
    // Within the half-pixel tolerance, and flush: not reported.
    let _near = rect(&mut s, f, "Near", (170.4, 0.0, 30.0, 30.0));
    let _flush = rect(&mut s, f, "Flush", (170.0, 70.0, 30.0, 30.0));
    assert_eq!(
        overflow(&mut s, json!({})),
        json!({"items": [item(corner, "Corner", f, "F", "outside-parent-clipped", "10 past left, 10 past bottom")], "total": 1, "truncated": false})
    );
}

#[test]
fn a_shadow_cut_by_a_clipping_parent_is_effect_clipped_though_the_box_fits() {
    let mut s = Session::new();
    let page = s.page().unwrap();
    let f = add(&mut s, page, NodeKind::Frame, "F", (0.0, 0.0, 200.0, 100.0));
    let card = rect(&mut s, f, "Card", (0.0, 10.0, 50.0, 50.0));
    let roomy = rect(&mut s, f, "Roomy", (100.0, 30.0, 50.0, 50.0));
    shadow(&mut s, card, (0.0, 0.0), 10.0);
    shadow(&mut s, roomy, (0.0, 0.0), 10.0);
    assert_eq!(
        overflow(&mut s, json!({})),
        json!({"items": [item(card, "Card", f, "F", "effect-clipped", "10 past left")], "total": 1, "truncated": false})
    );
    // Nothing clips it in a frame that doesn't clip, and a hidden layer paints nothing.
    s.doc_mut().unwrap().get_mut(f).unwrap().clip = false;
    assert_eq!(overflow(&mut s, json!({})), json!({"items": [], "total": 0, "truncated": false}));
    s.doc_mut().unwrap().get_mut(f).unwrap().clip = true;
    s.doc_mut().unwrap().get_mut(card).unwrap().visible = false;
    assert_eq!(overflow(&mut s, json!({})), json!({"items": [], "total": 0, "truncated": false}));
}

#[test]
fn a_shadow_offset_towards_the_edge_reaches_past_it_by_offset_plus_blur() {
    let mut s = Session::new();
    let page = s.page().unwrap();
    let f = add(&mut s, page, NodeKind::Frame, "F", (0.0, 0.0, 200.0, 100.0));
    let card = rect(&mut s, f, "Card", (100.0, 30.0, 100.0, 64.0));
    shadow(&mut s, card, (0.0, 4.0), 6.0);
    assert_eq!(
        overflow(&mut s, json!({})),
        json!({"items": [item(card, "Card", f, "F", "effect-clipped", "6 past right, 4 past bottom")], "total": 1, "truncated": false})
    );
}

#[test]
fn a_section_does_not_clip_and_a_group_is_judged_by_its_children_against_the_frame() {
    let mut s = Session::new();
    let page = s.page().unwrap();
    let sec = add(&mut s, page, NodeKind::Section { contents_hidden: false }, "Sec", (0.0, 0.0, 100.0, 100.0));
    let spill = rect(&mut s, sec, "Spill", (80.0, 0.0, 40.0, 40.0));
    let f = add(&mut s, page, NodeKind::Frame, "F", (500.0, 0.0, 100.0, 100.0));
    let g = add(&mut s, f, NodeKind::Group, "G", (50.0, 0.0, 50.0, 50.0));
    let kid = rect(&mut s, g, "Kid", (30.0, 0.0, 40.0, 40.0));
    rect(&mut s, page, "Top", (900.0, 900.0, 40.0, 40.0));
    assert_eq!(
        overflow(&mut s, json!({})),
        json!({"items": [
            item(spill, "Spill", sec, "Sec", "outside-parent", "20 past right"),
            item(kid, "Kid", f, "F", "outside-parent-clipped", "20 past right"),
        ], "total": 2, "truncated": false})
    );
}

#[test]
fn root_page_and_limit_scope_the_search_and_limit_reports_truncation() {
    let mut s = Session::new();
    let page = s.page().unwrap();
    let f1 = add(&mut s, page, NodeKind::Frame, "F1", (0.0, 0.0, 100.0, 100.0));
    let a = rect(&mut s, f1, "A", (90.0, 0.0, 20.0, 20.0));
    let b = rect(&mut s, f1, "B", (95.0, 40.0, 20.0, 20.0));
    let other = s.doc_mut().unwrap().add_page("Other");
    let f2 = add(&mut s, other, NodeKind::Frame, "F2", (0.0, 0.0, 100.0, 100.0));
    let c = rect(&mut s, f2, "C", (0.0, 90.0, 20.0, 20.0));
    let limited = overflow(&mut s, json!({"limit": 1}));
    assert_eq!(limited["items"], json!([item(a, "A", f1, "F1", "outside-parent-clipped", "10 past right")]));
    assert_eq!((limited["total"].clone(), limited["truncated"].clone()), (json!(2), json!(true)));
    assert_eq!(overflow(&mut s, json!({"limit": 0}))["items"], json!([]));
    assert_eq!(
        overflow(&mut s, json!({"page": "other"})),
        json!({"items": [item(c, "C", f2, "F2", "outside-parent-clipped", "10 past bottom")], "total": 1, "truncated": false})
    );
    assert_eq!(overflow(&mut s, json!({"page": "all"}))["total"], json!(3));
    assert_eq!(
        overflow(&mut s, json!({"root": f1, "page": "other"})),
        json!({"items": [
            item(a, "A", f1, "F1", "outside-parent-clipped", "10 past right"),
            item(b, "B", f1, "F1", "outside-parent-clipped", "15 past right"),
        ], "total": 2, "truncated": false})
    );
    // The root's descendants only: its own overflow belongs to its parent's search.
    assert_eq!(overflow(&mut s, json!({"root": a}))["total"], json!(0));
    assert_eq!(s.page().unwrap(), page);
    assert!(s.execute("inspect.overflow", &json!({"limit": 10001})).is_err());
    assert!(s.execute("inspect.overflow", &json!({"page": "Nowhere"})).is_err());
}

#[test]
fn fixed_size_text_taller_than_its_box_is_text_overflow_and_auto_height_text_is_not() {
    if figcraft_text::CRAFT_FONTS.is_empty() {
        eprintln!("skipped: no craft-fonts embedded");
        return;
    }
    let mut s = Session::new();
    let page = s.page().unwrap();
    let long = "The quick brown fox jumps over the lazy dog and keeps running across the field";
    let fixed = add(&mut s, page, NodeKind::Text(Box::new(TextData::new(long))), "Fixed", (0.0, 0.0, 60.0, 10.0));
    let auto = add(&mut s, page, NodeKind::Text(Box::new(TextData::new(long))), "Auto", (0.0, 100.0, 60.0, 10.0));
    for (id, mode) in [(fixed, TextAutoResize::None), (auto, TextAutoResize::Height)] {
        if let NodeKind::Text(t) = &mut s.doc_mut().unwrap().get_mut(id).unwrap().kind {
            t.auto_resize = mode;
        }
    }
    let r = overflow(&mut s, json!({}));
    assert_eq!(r["total"], json!(1));
    assert_eq!(r["items"][0]["id"], json!(fixed));
    assert_eq!(r["items"][0]["kind"], json!("text-overflow"));
    assert_eq!(r["items"][0]["parent"], json!(page));
    let detail = r["items"][0]["detail"].as_str().unwrap();
    assert!(detail.starts_with("text is ") && detail.contains(" tall, the box 10"), "{detail}");
}
