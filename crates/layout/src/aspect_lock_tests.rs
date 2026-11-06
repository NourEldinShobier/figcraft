//! Locked aspect ratio in auto layout: the axis a locked child does not fill follows the one it
//! does (width / height = the ratio the lock captured). Every case is laid out twice by `settle`
//! and must not move.

use super::*;
use figcraft_doc::{AutoLayout, Document, NodeKind};

fn frame(d: &mut Document, mode: LayoutMode, (w, h): (f64, f64), (sh, sv): (Sizing, Sizing)) -> NodeId {
    let p = d.first_page().unwrap();
    let f = d.create(p, NodeKind::Frame, "F", 0.0, 0.0, w, h).unwrap();
    let n = d.get_mut(f).unwrap();
    n.layout = Some(Box::new(AutoLayout { mode, gap: 0.0, padding: [0.0; 4], ..Default::default() }));
    n.child_layout.sizing_h = sh;
    n.child_layout.sizing_v = sv;
    f
}

/// A rectangle of `w` x `h`; locked, it keeps the ratio it has now.
fn child(d: &mut Document, parent: NodeId, (w, h): (f64, f64), locked: bool, (sh, sv): (Sizing, Sizing)) -> NodeId {
    let c = d.create(parent, NodeKind::Rectangle, "R", 0.0, 0.0, w, h).unwrap();
    let n = d.get_mut(c).unwrap();
    n.constrain_proportions = locked;
    n.aspect_ratio = locked.then_some(w / h);
    n.child_layout.sizing_h = sh;
    n.child_layout.sizing_v = sv;
    c
}

fn size(d: &Document, id: NodeId) -> (f64, f64) {
    let n = d.node(id).unwrap();
    (n.width, n.height)
}

fn settle(d: &mut Document, f: NodeId) {
    layout_frame(d, f, &NoMeasure);
    layout_frame(d, f, &NoMeasure);
}

#[test]
fn fill_width_and_hug_height_derive_the_height() {
    let mut d = Document::new("t");
    let f = frame(&mut d, LayoutMode::Horizontal, (300.0, 10.0), (Sizing::Fixed, Sizing::Hug));
    let c = child(&mut d, f, (200.0, 100.0), true, (Sizing::Fill, Sizing::Hug));
    settle(&mut d, f);
    assert_eq!(size(&d, c), (300.0, 150.0));
    assert_eq!(size(&d, f), (300.0, 150.0));
    // The frame resized to 450 gives 450 x 225.
    set_size(&mut d, f, 450.0, 150.0);
    settle(&mut d, f);
    assert_eq!(size(&d, c), (450.0, 225.0));
    assert_eq!(size(&d, f), (450.0, 225.0));
}

#[test]
fn an_unlocked_child_is_unchanged() {
    let mut d = Document::new("t");
    let f = frame(&mut d, LayoutMode::Horizontal, (300.0, 10.0), (Sizing::Fixed, Sizing::Hug));
    let c = child(&mut d, f, (200.0, 100.0), false, (Sizing::Fill, Sizing::Hug));
    settle(&mut d, f);
    assert_eq!(size(&d, c), (300.0, 100.0));
}

#[test]
fn fill_height_and_hug_width_derive_the_width() {
    // Horizontal flow, the frame 80 tall: the child fills the height, its width follows.
    let mut d = Document::new("t");
    let f = frame(&mut d, LayoutMode::Horizontal, (10.0, 80.0), (Sizing::Hug, Sizing::Fixed));
    let c = child(&mut d, f, (100.0, 50.0), true, (Sizing::Hug, Sizing::Fill));
    settle(&mut d, f);
    assert_eq!(size(&d, c), (160.0, 80.0));
    assert_eq!(size(&d, f), (160.0, 80.0));
    // Vertical flow, the frame 200 tall: the child fills the height along the flow.
    let g = frame(&mut d, LayoutMode::Vertical, (10.0, 200.0), (Sizing::Hug, Sizing::Fixed));
    let k = child(&mut d, g, (100.0, 50.0), true, (Sizing::Hug, Sizing::Fill));
    settle(&mut d, g);
    assert_eq!(size(&d, k), (400.0, 200.0));
    assert_eq!(size(&d, g), (400.0, 200.0));
}

#[test]
fn both_fill_and_both_fixed_are_left_alone() {
    let mut d = Document::new("t");
    let f = frame(&mut d, LayoutMode::Horizontal, (300.0, 120.0), (Sizing::Fixed, Sizing::Fixed));
    let both_fill = child(&mut d, f, (100.0, 50.0), true, (Sizing::Fill, Sizing::Fill));
    settle(&mut d, f);
    assert_eq!(size(&d, both_fill), (300.0, 120.0));
    let g = frame(&mut d, LayoutMode::Horizontal, (300.0, 120.0), (Sizing::Fixed, Sizing::Fixed));
    let fixed = child(&mut d, g, (100.0, 50.0), true, (Sizing::Fixed, Sizing::Fixed));
    settle(&mut d, g);
    assert_eq!(size(&d, fixed), (100.0, 50.0));
}

#[test]
fn the_derived_size_stays_within_min_and_max() {
    let mut d = Document::new("t");
    let f = frame(&mut d, LayoutMode::Horizontal, (300.0, 10.0), (Sizing::Fixed, Sizing::Hug));
    let c = child(&mut d, f, (200.0, 100.0), true, (Sizing::Fill, Sizing::Hug));
    d.get_mut(c).unwrap().child_layout.max_h = Some(100.0);
    settle(&mut d, f);
    assert_eq!(size(&d, c), (300.0, 100.0));
}

#[test]
fn a_lock_without_a_stored_ratio_keeps_the_current_one() {
    let mut d = Document::new("t");
    let f = frame(&mut d, LayoutMode::Horizontal, (300.0, 10.0), (Sizing::Fixed, Sizing::Hug));
    let c = child(&mut d, f, (200.0, 100.0), true, (Sizing::Fill, Sizing::Hug));
    d.get_mut(c).unwrap().aspect_ratio = None;
    settle(&mut d, f);
    assert_eq!(size(&d, c), (300.0, 150.0));
}
