//! An instance the editor scaled as a whole (`symbolData.uniformScaleFactor`, set by the scale
//! tool): its layers scale with it, whatever their constraints say, along with their strokes.

use figcraft_doc::NodeKind;

use super::kiwi::Kv;
use super::map;

type Fields = Vec<(&'static str, Kv<'static>)>;

fn o(fields: Fields) -> Kv<'static> {
    Kv::Object(fields)
}

fn g(id: u64) -> Kv<'static> {
    o(vec![("sessionID", Kv::Uint(1)), ("localID", Kv::Uint(id))])
}

fn xy(x: f64, y: f64) -> Kv<'static> {
    o(vec![("x", Kv::Float(x as f32)), ("y", Kv::Float(y as f32))])
}

fn at(x: f64, y: f64) -> Kv<'static> {
    let m = |k: &'static str, v: f32| (k, Kv::Float(v));
    o(vec![m("m00", 1.0), m("m01", 0.0), m("m02", x as f32), m("m10", 0.0), m("m11", 1.0), m("m12", y as f32)])
}

fn node(id: u64, parent: u64, ty: &'static str, name: &str, more: Fields) -> Kv<'static> {
    let mut fields = vec![
        ("guid", g(id)),
        ("phase", Kv::Enum("CREATED")),
        ("type", Kv::Enum(ty)),
        ("name", Kv::Str(name.into())),
        ("parentIndex", o(vec![("guid", g(parent)), ("position", Kv::Str(format!("{id:08}")))])),
    ];
    fields.extend(more);
    o(fields)
}

fn solid() -> Kv<'static> {
    let rgb = o(vec![("r", Kv::Float(0.0)), ("g", Kv::Float(0.0)), ("b", Kv::Float(1.0)), ("a", Kv::Float(1.0))]);
    o(vec![("type", Kv::Enum("SOLID")), ("color", rgb), ("opacity", Kv::Float(1.0)), ("visible", Kv::Bool(true))])
}

/// A 40 × 40 main with a 20 × 20 square at (10, 10) with a 4 px stroke, and an instance of it at
/// `size` (20 × 20) scaled by `factor`.
fn message(factor: f32) -> Kv<'static> {
    let nodes = vec![
        node(1, 0, "DOCUMENT", "Document", vec![]),
        node(2, 1, "CANVAS", "Page", vec![]),
        node(20, 2, "SYMBOL", "Main", vec![("size", xy(40.0, 40.0)), ("transform", at(0.0, 0.0))]),
        node(
            21,
            20,
            "RECTANGLE",
            "Square",
            vec![
                ("size", xy(20.0, 20.0)),
                ("transform", at(10.0, 10.0)),
                ("strokeWeight", Kv::Float(4.0)),
                ("strokePaints", Kv::Array(vec![solid()])),
                ("cornerRadius", Kv::Float(6.0)),
            ],
        ),
        node(
            30,
            2,
            "INSTANCE",
            "Small",
            vec![
                ("size", xy(20.0, 20.0)),
                ("transform", at(100.0, 0.0)),
                ("symbolData", o(vec![("symbolID", g(20)), ("uniformScaleFactor", Kv::Float(factor))])),
            ],
        ),
    ];
    o(vec![("nodeChanges", Kv::Array(nodes))])
}

fn import(factor: f32) -> figcraft_doc::Document {
    let (json, _, networks) = map::to_rest(&message(factor), "Scaled", 1000);
    crate::rest::import_with_networks(&json, &|_: &str| None, networks, 1000).unwrap().0
}

#[test]
fn uniformly_scaled_instances_scale_their_layers() {
    let doc = import(0.5);
    let small = doc.nodes().find(|n| n.name == "Small").unwrap();
    assert!(matches!(small.kind, NodeKind::Instance { .. }));
    let square = doc.node(small.children[0]).unwrap();
    assert_eq!((square.transform.as_coeffs()[4], square.transform.as_coeffs()[5], square.width, square.height), (5.0, 5.0, 10.0, 10.0));
    assert_eq!(square.stroke.weight, 2.0);
    assert_eq!(square.corners.radii[0], 3.0);
}

#[test]
fn an_instance_of_factor_one_keeps_its_layers_as_they_are() {
    let doc = import(1.0);
    let small = doc.nodes().find(|n| n.name == "Small").unwrap();
    let square = doc.node(small.children[0]).unwrap();
    assert_eq!((square.transform.as_coeffs()[4], square.transform.as_coeffs()[5], square.width, square.stroke.weight), (10.0, 10.0, 20.0, 4.0));
}
