//! Instances of icon-like mains: resizing scales the content, creating one in auto layout keeps
//! the main's size, inner layers take geometry and constraint overrides, `instance.swapAt` can
//! keep overrides, and frames made in auto layout default to no fill.

use serde_json::json;

use crate::Session;
use crate::components::tests::{copy_of, ex, healthy, idv, node};
use figcraft_doc::*;

/// A 24x24 icon component "Icon" holding one 24x24 vector "Glyph" (a stroked diagonal). Returns
/// (component, glyph).
fn icon(s: &mut Session, size: f64) -> (NodeId, NodeId) {
    let f = idv(&ex(s, "frame.create", json!({"x": 0, "y": 0, "width": size, "height": size, "name": "Icon"})));
    let g = idv(&ex(
        s,
        "vector.create",
        json!({"points": [[0, 0], [size, size]], "parent": f, "x": 0, "y": 0, "width": size, "height": size, "props": {"name": "Glyph", "stroke": "#FFFFFF", "strokeWidth": 2}}),
    ));
    let c = idv(&ex(s, "component.create", json!({"ids": [f]})));
    (c, g)
}

fn size(s: &Session, id: NodeId) -> (f64, f64) {
    let n = node(s, id);
    (n.width, n.height)
}

fn near(a: (f64, f64), b: (f64, f64)) -> bool {
    (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6
}

#[test]
fn resizing_an_icon_instance_scales_its_vectors() {
    let mut s = Session::new();
    let (c, g) = icon(&mut s, 24.0);
    let i = idv(&ex(&mut s, "instance.create", json!({"component": c, "x": 100, "y": 0})));
    ex(&mut s, "node.set", json!({"id": i, "props": {"width": 12, "height": 12}}));
    let ig = copy_of(&s, i, g);
    assert!(near(size(&s, i), (12.0, 12.0)), "{:?}", size(&s, i));
    assert!(near(size(&s, ig), (12.0, 12.0)), "glyph scaled with the icon: {:?}", size(&s, ig));
    healthy(&s);
    // The main and a non-resized instance keep the main's size.
    let j = idv(&ex(&mut s, "instance.create", json!({"component": c, "x": 200, "y": 0})));
    assert!(near(size(&s, copy_of(&s, j, g)), (24.0, 24.0)));
    assert!(near(size(&s, g), (24.0, 24.0)));
}

fn row(s: &mut Session) -> NodeId {
    let r = idv(&ex(s, "frame.create", json!({"x": 500, "y": 0, "width": 200, "height": 40, "name": "Row"})));
    ex(s, "node.set", json!({"id": r, "props": {"layout": "horizontal", "gap": 4, "padding": 0}}));
    r
}

fn stroke_hex(s: &Session, id: NodeId) -> String {
    node(s, id).strokes.first().map(|p| format!("{:?}", p)).unwrap_or_default()
}

#[test]
fn an_instance_made_in_auto_layout_has_the_mains_size() {
    let mut s = Session::new();
    let (c, g) = icon(&mut s, 17.33);
    let r = row(&mut s);
    let i = idv(&ex(&mut s, "instance.create", json!({"component": c, "parent": r})));
    assert!(near(size(&s, i), (17.33, 17.33)), "{:?}", size(&s, i));
    assert!(near(size(&s, copy_of(&s, i, g)), (17.33, 17.33)));
    let p = idv(&ex(&mut s, "instance.place", json!({"component": c, "x": 520, "y": 10})));
    assert!(near(size(&s, p), (17.33, 17.33)), "placed: {:?}", size(&s, p));
    ex(&mut s, "node.set", json!({"id": i, "props": {"width": 10, "height": 10}}));
    assert!(near(size(&s, copy_of(&s, i, g)), (10.0, 10.0)));
    healthy(&s);
}

#[test]
fn layers_inside_an_instance_take_geometry_and_constraint_overrides() {
    let mut s = Session::new();
    let (c, g) = icon(&mut s, 24.0);
    let i = idv(&ex(&mut s, "instance.create", json!({"component": c, "x": 100, "y": 0})));
    let ig = copy_of(&s, i, g);
    ex(&mut s, "node.set", json!({"id": ig, "props": {"x": 4, "y": 5, "width": 10, "height": 8}}));
    ex(&mut s, "node.set", json!({"id": ig, "props": {"constraints": {"horizontal": "right", "vertical": "bottom"}}}));
    let n = node(&s, ig);
    assert!(near((n.width, n.height), (10.0, 8.0)));
    assert_eq!((n.constraints.horizontal, n.constraints.vertical), (HConstraint::Right, VConstraint::Bottom));
    assert!(n.overrides.contains("size") && n.overrides.contains("constraints"), "{:?}", n.overrides);
    // They survive an edit of the main.
    ex(&mut s, "node.set", json!({"id": g, "props": {"stroke": "#00FF00"}}));
    let n = node(&s, ig);
    assert!(near((n.width, n.height), (10.0, 8.0)), "{:?}", (n.width, n.height));
    assert_eq!(n.constraints.horizontal, HConstraint::Right);
    // The main is untouched; resetOverrides brings the instance back, size included.
    assert_eq!(node(&s, g).constraints.horizontal, HConstraint::Left);
    ex(&mut s, "node.set", json!({"id": i, "props": {"width": 48, "height": 48}}));
    ex(&mut s, "instance.resetOverrides", json!({"ids": [i]}));
    let n = node(&s, copy_of(&s, i, g));
    assert!(near((n.width, n.height), (24.0, 24.0)), "{:?}", (n.width, n.height));
    assert_eq!(n.constraints.horizontal, HConstraint::Left);
    assert!(near(size(&s, i), (24.0, 24.0)), "{:?}", size(&s, i));
    healthy(&s);
}

#[test]
fn swap_at_keeps_size_and_with_keep_overrides_the_strokes() {
    for keep in [false, true] {
        let mut s = Session::new();
        let (c, g) = icon(&mut s, 24.0);
        let (c2, _) = icon(&mut s, 24.0);
        let i = idv(&ex(&mut s, "instance.create", json!({"component": c, "x": 300, "y": 300})));
        ex(&mut s, "node.set", json!({"id": i, "props": {"width": 15, "height": 15}}));
        let ig = copy_of(&s, i, g);
        ex(&mut s, "node.set", json!({"id": ig, "props": {"stroke": "#8C8C8C"}}));
        let before = stroke_hex(&s, ig);
        let mut p = json!({"component": c2, "x": 307, "y": 307});
        if keep {
            p["keepOverrides"] = json!(true);
        }
        let r = s.execute("instance.swapAt", &p);
        r.unwrap_or_else(|e| panic!("{e}"));
        assert!(near(size(&s, i), (15.0, 15.0)), "keep={keep}: {:?}", size(&s, i));
        let new_glyph = node(&s, i).children[0];
        assert!(near(size(&s, new_glyph), (15.0, 15.0)), "keep={keep}");
        assert_eq!(stroke_hex(&s, new_glyph) == before, keep, "keep={keep}");
        healthy(&s);
    }
}

#[test]
fn frames_made_in_auto_layout_default_to_no_fill() {
    let mut s = Session::new();
    let top = idv(&ex(&mut s, "node.create", json!({"type": "frame", "width": 50, "height": 50})));
    assert_eq!(node(&s, top).fills.len(), 1, "top-level frames keep the white default");
    let r = row(&mut s);
    let a = idv(&ex(&mut s, "node.create", json!({"type": "frame", "parent": r, "width": 20, "height": 20})));
    assert!(node(&s, a).fills.is_empty());
    let b = idv(&ex(&mut s, "node.create", json!({"type": "frame", "parent": r, "width": 20, "height": 20, "props": {"fill": "#112233"}})));
    assert_eq!(node(&s, b).fills.len(), 1);
}
