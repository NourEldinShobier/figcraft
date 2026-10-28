//! Bounds kept across renders follow edits: an edited layer's and its ancestors' answers are
//! dropped, the rest stay.

use super::*;
use crate::bounds_cache::refresh;
use crate::draw::{Bounds, Seed};
use std::sync::Arc;

/// Unclipped frames G > F > R and a separate frame S > T; returns their ids.
fn tree() -> (Document, [figcraft_doc::NodeId; 5]) {
    let mut d = Document::new("kept");
    let p = d.first_page().unwrap();
    let frame = |d: &mut Document, parent, name: &str, x, y, w, h| {
        let f = d.create(parent, NodeKind::Frame, name, x, y, w, h).unwrap();
        d.node_mut(f).unwrap().clip = false;
        f
    };
    let g = frame(&mut d, p, "G", 0.0, 0.0, 10.0, 10.0);
    let f = frame(&mut d, g, "F", 0.0, 0.0, 10.0, 10.0);
    let r = d.create(f, NodeKind::Rectangle, "R", 10.0, 20.0, 30.0, 40.0).unwrap();
    let s = frame(&mut d, p, "S", 500.0, 0.0, 10.0, 10.0);
    let t = d.create(s, NodeKind::Rectangle, "T", 5.0, 5.0, 10.0, 10.0).unwrap();
    (d, [g, f, r, s, t])
}

fn bounds_of(d: &Document, id: figcraft_doc::NodeId, seed: &Seed) -> Rect {
    draw::bounds_within(d, d.node(id).unwrap(), 0, None, Some(&Bounds::seeded(seed.clone()))).0
}

/// The answers for the layers of `d` that have children.
fn learn(d: &Document, ids: &[figcraft_doc::NodeId]) -> Seed {
    let memo = Bounds::default();
    for id in ids {
        draw::bounds_within(d, d.node(*id).unwrap(), 0, None, Some(&memo));
    }
    let l = memo.into_learned();
    Seed { paint: Arc::new(l.paint), splat: Arc::new(l.splat), greek: None }
}

#[test]
fn editing_a_layer_drops_its_and_its_ancestors_bounds_only() {
    let (d, [g, f, r, s, _]) = tree();
    let seed = learn(&d, &[g, s]);
    assert_eq!(bounds_of(&d, g, &seed), Rect::new(0.0, 0.0, 40.0, 60.0));
    assert_eq!(bounds_of(&d, s, &seed), Rect::new(0.0, 0.0, 15.0, 15.0));
    let mut edited = d.clone();
    edited.node_mut(r).unwrap().width = 100.0;
    // Stale without the refresh, right with it.
    assert_eq!(bounds_of(&edited, g, &seed), Rect::new(0.0, 0.0, 40.0, 60.0));
    let fresh = refresh(seed, &d, &edited).unwrap();
    assert_eq!(bounds_of(&edited, g, &fresh), Rect::new(0.0, 0.0, 110.0, 60.0));
    assert_eq!(bounds_of(&edited, f, &fresh), Rect::new(0.0, 0.0, 110.0, 60.0));
    // The other frame's answers were kept.
    assert!(fresh.paint.contains_key(&(s, None)));
    assert!(!fresh.paint.contains_key(&(g, None)));
}

#[test]
fn moving_a_parent_changes_the_bounds_of_the_layers_above_it() {
    let (d, [g, f, ..]) = tree();
    let seed = learn(&d, &[g]);
    let mut moved = d.clone();
    moved.node_mut(f).unwrap().transform = figcraft_geom::Affine::translate((100.0, 0.0));
    let fresh = refresh(seed, &d, &moved).unwrap();
    assert_eq!(bounds_of(&moved, g, &fresh), Rect::new(0.0, 0.0, 140.0, 60.0));
}

#[test]
fn a_removed_layer_drops_the_bounds_of_its_old_ancestors() {
    let (d, [g, f, r, ..]) = tree();
    let seed = learn(&d, &[g]);
    let mut removed = d.clone();
    removed.remove(r).unwrap();
    let fresh = refresh(seed, &d, &removed).unwrap();
    assert_eq!(bounds_of(&removed, g, &fresh), Rect::new(0.0, 0.0, 10.0, 10.0));
    assert_eq!(bounds_of(&removed, f, &fresh), Rect::new(0.0, 0.0, 10.0, 10.0));
}
