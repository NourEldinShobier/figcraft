//! `edit.replaceColor`: the scopes (selection with descendants, the current page, every page), fills
//! and strokes apart, the tolerance, an explicit alpha in `from`, paints bound to a variable left
//! alone, opacity and hidden paints kept, one undo step, and refused parameters.

use figcraft_color::{Paint, PaintKind};
use figcraft_doc::NodeId;
use serde_json::{Value, json};

use crate::Session;

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

fn id(v: &Value) -> NodeId {
    NodeId(v["id"].as_u64().unwrap())
}

fn rect(s: &mut Session, props: Value, parent: Option<NodeId>) -> NodeId {
    let mut p = json!({"x": 0, "y": 0, "width": 10, "height": 10, "props": props});
    if let Some(f) = parent {
        p["parent"] = json!(f);
    }
    id(&s.execute("shape.rectangle", &p).unwrap())
}

fn paint(s: &Session, n: NodeId, fills: bool, i: usize) -> Paint {
    let n = s.doc().unwrap().node(n).unwrap();
    (if fills { &n.fills } else { &n.strokes })[i].clone()
}

/// The colour of a solid paint as bytes (opacity not included).
fn rgba(p: &Paint) -> [u8; 4] {
    match &p.kind {
        PaintKind::Solid { color } => color.to_rgba8(),
        other => panic!("not a solid paint: {other:?}"),
    }
}

fn fill(s: &Session, n: NodeId) -> [u8; 4] {
    rgba(&paint(s, n, true, 0))
}

fn stroke(s: &Session, n: NodeId) -> [u8; 4] {
    rgba(&paint(s, n, false, 0))
}

/// Page 1: `a` (red fill, blue stroke), `b` (red fill and stroke), a frame `f` holding `e` (red
/// fill); page 2: `d` (red fill). Page 1 is current.
struct Fx {
    s: Session,
    a: NodeId,
    b: NodeId,
    f: NodeId,
    e: NodeId,
    d: NodeId,
}

fn setup() -> Fx {
    let mut s = Session::new();
    let a = rect(&mut s, json!({"fill": "#FF0000", "stroke": "#0000FF"}), None);
    let b = rect(&mut s, json!({"fill": "#FF0000", "stroke": "#FF0000"}), None);
    let f = id(&s.execute("frame.create", &json!({"x": 100, "y": 0, "width": 50, "height": 50})).unwrap());
    let e = rect(&mut s, json!({"fill": "#FF0000"}), Some(f));
    s.execute("page.add", &json!({"name": "Second"})).unwrap();
    let d = rect(&mut s, json!({"fill": "#FF0000"}), None);
    s.execute("page.select", &json!({"index": 0})).unwrap();
    Fx { s, a, b, f, e, d }
}

fn replace(s: &mut Session, extra: Value) -> Value {
    let mut p = json!({"from": "#FF0000", "to": "#00FF00"});
    for (k, v) in extra.as_object().unwrap() {
        p[k] = v.clone();
    }
    s.execute("edit.replaceColor", &p).unwrap()
}

fn ids(v: &[NodeId]) -> Value {
    json!(v.iter().map(|i| i.0).collect::<Vec<_>>())
}

#[test]
fn the_current_page_is_the_default_scope() {
    let mut x = setup();
    let r = replace(&mut x.s, json!({}));
    assert_eq!(r["changed"], ids(&[x.a, x.b, x.e]));
    assert_eq!(r["paints"], 4);
    assert_eq!(r["skippedBound"], json!([]));
    assert_eq!((fill(&x.s, x.a), stroke(&x.s, x.a)), (GREEN, BLUE));
    assert_eq!((fill(&x.s, x.b), stroke(&x.s, x.b)), (GREEN, GREEN));
    assert_eq!(fill(&x.s, x.e), GREEN);
    assert_eq!(fill(&x.s, x.d), RED, "another page is left alone");
}

#[test]
fn the_selection_scope_covers_the_selection_and_its_descendants() {
    let mut x = setup();
    x.s.execute("select.set", &json!({"ids": [x.f.0]})).unwrap();
    let r = replace(&mut x.s, json!({"scope": "selection"}));
    assert_eq!(r["changed"], ids(&[x.e]));
    assert_eq!(r["paints"], 1);
    assert_eq!(fill(&x.s, x.e), GREEN);
    assert_eq!(fill(&x.s, x.a), RED);
    x.s.execute("select.set", &json!({"ids": [x.a.0]})).unwrap();
    let r = replace(&mut x.s, json!({"scope": "selection"}));
    assert_eq!(r["changed"], ids(&[x.a]));
    assert_eq!(fill(&x.s, x.a), GREEN);
    assert_eq!(fill(&x.s, x.b), RED);
}

#[test]
fn the_selection_scope_needs_a_selection() {
    let mut x = setup();
    x.s.execute("select.none", &json!({})).unwrap();
    assert!(x.s.execute("edit.replaceColor", &json!({"from": "#FF0000", "to": "#00FF00", "scope": "selection"})).is_err());
    assert_eq!(fill(&x.s, x.a), RED);
}

#[test]
fn the_file_scope_reaches_every_page() {
    let mut x = setup();
    let r = replace(&mut x.s, json!({"scope": "file"}));
    assert_eq!(r["changed"], ids(&[x.a, x.b, x.e, x.d]));
    assert_eq!(r["paints"], 5);
    assert_eq!(fill(&x.s, x.d), GREEN);
}

#[test]
fn fills_only_and_strokes_only() {
    let mut x = setup();
    let r = replace(&mut x.s, json!({"fills": false}));
    assert_eq!(r["changed"], ids(&[x.b]));
    assert_eq!(r["paints"], 1);
    assert_eq!((fill(&x.s, x.b), stroke(&x.s, x.b)), (RED, GREEN));
    assert_eq!(fill(&x.s, x.a), RED);

    let mut x = setup();
    let r = replace(&mut x.s, json!({"strokes": false}));
    assert_eq!(r["changed"], ids(&[x.a, x.b, x.e]));
    assert_eq!(r["paints"], 3);
    assert_eq!((fill(&x.s, x.b), stroke(&x.s, x.b)), (GREEN, RED));
    assert_eq!((fill(&x.s, x.a), stroke(&x.s, x.a)), (GREEN, BLUE));
}

#[test]
fn the_tolerance_is_a_per_channel_distance() {
    let mut s = Session::new();
    let exact = rect(&mut s, json!({"fill": "#FF0000"}), None);
    let off_by_one = rect(&mut s, json!({"fill": "#FE0000"}), None);
    let far = rect(&mut s, json!({"fill": "#FF8080"}), None);
    // 1/255 is 0.0039: out of reach of 0.001, within 0.01.
    for tolerance in [json!(0), json!(0.001)] {
        let r = replace(&mut s, json!({"tolerance": tolerance, "to": "#00FF00"}));
        assert_eq!(r["changed"], ids(&[exact]), "tolerance {tolerance}");
        s.execute("edit.undo", &json!({})).unwrap();
    }
    let r = replace(&mut s, json!({"tolerance": 0.01}));
    assert_eq!(r["changed"], ids(&[exact, off_by_one]));
    assert_eq!(fill(&s, off_by_one), GREEN);
    assert_eq!(fill(&s, far), [255, 128, 128, 255]);
}

#[test]
fn a_paint_bound_to_a_variable_is_skipped_and_reported() {
    let mut x = setup();
    let ex = |s: &mut Session, cmd: &str, p: Value| {
        s.execute(cmd, &p).unwrap_or_else(|e| panic!("{cmd}: {e}"));
    };
    ex(&mut x.s, "variable.createCollection", json!({"name": "Tokens", "modes": ["Light"]}));
    ex(&mut x.s, "variable.create", json!({"collection": "Tokens", "name": "danger", "type": "color", "value": "#FF0000"}));
    // `b` is bound on its fill; its stroke is a plain red.
    ex(&mut x.s, "variable.bind", json!({"ids": [x.b.0], "field": "fill", "variable": "danger"}));
    let r = replace(&mut x.s, json!({}));
    assert_eq!(r["changed"], ids(&[x.a, x.b, x.e]));
    assert_eq!(r["paints"], 3);
    assert_eq!(r["skippedBound"], ids(&[x.b]));
    assert_eq!(fill(&x.s, x.b), RED, "the bound fill keeps the variable's colour");
    assert!(!x.s.doc().unwrap().node(x.b).unwrap().bound.is_empty());
    assert_eq!(stroke(&x.s, x.b), GREEN, "the plain stroke of the same layer changes");
    assert_eq!(fill(&x.s, x.a), GREEN);

    // A layer whose only matching paint is bound is reported but not listed as changed.
    let mut x = setup();
    ex(&mut x.s, "variable.createCollection", json!({"name": "Tokens", "modes": ["Light"]}));
    ex(&mut x.s, "variable.create", json!({"collection": "Tokens", "name": "danger", "type": "color", "value": "#FF0000"}));
    ex(&mut x.s, "variable.bind", json!({"ids": [x.e.0], "field": "fill", "variable": "danger"}));
    let r = replace(&mut x.s, json!({}));
    assert_eq!(r["changed"], ids(&[x.a, x.b]));
    assert_eq!(r["skippedBound"], ids(&[x.e]));
    assert_eq!(fill(&x.s, x.e), RED);
}

#[test]
fn opacity_is_kept_and_an_alpha_in_from_must_match() {
    let mut s = Session::new();
    let full = rect(&mut s, json!({"fill": "#FF0000"}), None);
    let half = rect(&mut s, json!({"fill": "#FF000080"}), None);
    // Without an alpha, `from` matches the colour whatever the paint's opacity.
    let r = replace(&mut s, json!({}));
    assert_eq!(r["changed"], ids(&[full, half]));
    let p = paint(&s, half, true, 0);
    assert_eq!(rgba(&p), GREEN);
    assert_eq!((p.opacity * 255.0).round(), 128.0, "the 50% opacity stays");
    assert_eq!(paint(&s, full, true, 0).opacity, 1.0);
    s.execute("edit.undo", &json!({})).unwrap();
    // With one, only the paint of that effective alpha matches.
    let r = replace(&mut s, json!({"from": "#FF000080"}));
    assert_eq!(r["changed"], ids(&[half]));
    assert_eq!(fill(&s, full), RED);
}

#[test]
fn hidden_paints_and_the_other_paints_of_a_layer_are_left_alone() {
    let mut s = Session::new();
    let h = rect(&mut s, json!({"fill": "#FF0000"}), None);
    s.execute("fill.add", &json!({"ids": [h.0], "color": "#FF0000"})).unwrap();
    s.execute("fill.set", &json!({"ids": [h.0], "index": 1, "visible": false})).unwrap();
    s.execute("fill.add", &json!({"ids": [h.0], "color": "#0000FF"})).unwrap();
    let r = replace(&mut s, json!({}));
    assert_eq!(r["paints"], 1);
    assert_eq!(rgba(&paint(&s, h, true, 0)), GREEN);
    let hidden = paint(&s, h, true, 1);
    assert_eq!((rgba(&hidden), hidden.visible), (RED, false));
    assert_eq!(rgba(&paint(&s, h, true, 2)), BLUE);
}

#[test]
fn a_colour_nobody_uses_changes_nothing() {
    let mut x = setup();
    let r = replace(&mut x.s, json!({"from": "#123456"}));
    assert_eq!((r["changed"].clone(), r["paints"].clone(), r["skippedBound"].clone()), (json!([]), json!(0), json!([])));
    assert_eq!(fill(&x.s, x.a), RED);
}

#[test]
fn one_undo_restores_every_page() {
    let mut x = setup();
    replace(&mut x.s, json!({"scope": "file"}));
    assert_eq!(fill(&x.s, x.d), GREEN);
    x.s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!((fill(&x.s, x.a), stroke(&x.s, x.a)), (RED, BLUE));
    assert_eq!((fill(&x.s, x.b), stroke(&x.s, x.b)), (RED, RED));
    assert_eq!((fill(&x.s, x.e), fill(&x.s, x.d)), (RED, RED));
}

#[test]
fn junk_parameters_are_refused() {
    let mut x = setup();
    for p in [
        json!({"to": "#00FF00"}),
        json!({"from": "#FF0000"}),
        json!({"from": "nope", "to": "#00FF00"}),
        json!({"from": "#FF0000", "to": 5}),
        json!({"from": "#FF0000", "to": "#00FF00", "scope": "everywhere"}),
        json!({"from": "#FF0000", "to": "#00FF00", "tolerance": 2}),
        json!({"from": "#FF0000", "to": "#00FF00", "tolerance": -0.5}),
        json!({"from": "#FF0000", "to": "#00FF00", "tolerance": "wide"}),
        json!({"from": "#FF0000", "to": "#00FF00", "text": true}),
    ] {
        assert!(x.s.execute("edit.replaceColor", &p).is_err(), "{p}");
    }
    assert_eq!(fill(&x.s, x.a), RED);
}
