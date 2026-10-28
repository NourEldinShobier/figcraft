//! Page-first import: the first page is built and handed over before the others are mapped, and
//! the whole import still gives the document the one-shot import gives.

use figcraft_doc::{Document, NodeKind};

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

fn node(id: u64, parent: Option<u64>, ty: &'static str, name: &str, more: Fields) -> Kv<'static> {
    let mut fields = vec![("guid", g(id)), ("phase", Kv::Enum("CREATED")), ("type", Kv::Enum(ty)), ("name", Kv::Str(name.into()))];
    if let Some(parent) = parent {
        fields.push(("parentIndex", o(vec![("guid", g(parent)), ("position", Kv::Str(format!("{id:08}")))])));
    }
    fields.extend(more);
    o(fields)
}

fn instance(id: u64, page: u64, name: &str, main: u64) -> Kv<'static> {
    node(id, Some(page), "INSTANCE", name, vec![("symbolData", o(vec![("symbolID", g(main))]))])
}

/// Pages "First" (2) and "Second" (4) and a hidden library canvas (3). "First" holds a rectangle,
/// an instance of the library main (20) and an instance of "Second"'s main (40); "Second" holds
/// that main and an instance of the library main.
fn message() -> Kv<'static> {
    let nodes = vec![
        node(1, None, "DOCUMENT", "Document", vec![]),
        node(2, Some(1), "CANVAS", "First", vec![]),
        node(3, Some(1), "CANVAS", "Internal", vec![("internalOnly", Kv::Bool(true))]),
        node(4, Some(1), "CANVAS", "Second", vec![]),
        node(20, Some(3), "SYMBOL", "Library main", vec![("size", xy(10.0, 10.0))]),
        node(21, Some(20), "RECTANGLE", "Library rect", vec![("size", xy(1.0, 1.0))]),
        node(30, Some(2), "RECTANGLE", "First rect", vec![("size", xy(5.0, 5.0))]),
        instance(31, 2, "Library use", 20),
        instance(32, 2, "Second use", 40),
        node(40, Some(4), "SYMBOL", "Second main", vec![("size", xy(8.0, 8.0))]),
        node(41, Some(40), "RECTANGLE", "Second rect", vec![("size", xy(1.0, 1.0))]),
        instance(42, 4, "Library use 2", 20),
    ];
    o(vec![("nodeChanges", Kv::Array(nodes))])
}

fn json_of(doc: &Document) -> String {
    let mut out = Vec::new();
    crate::write_json(doc, &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

fn build(json: &serde_json::Value, packed: crate::rest::Packed, networks: Vec<Option<figcraft_doc::VectorNetwork>>) -> Document {
    crate::rest::import_packed(json, packed, &|_: &str| None, networks, 1_000).unwrap().0
}

fn linked(doc: &Document, instance: &str) -> Option<String> {
    let n = doc.nodes().find(|n| n.name == instance)?;
    let NodeKind::Instance { component, .. } = &n.kind else { return None };
    doc.node(*component).map(|c| c.name.clone())
}

#[test]
fn the_first_page_comes_first_and_the_whole_import_is_unchanged() {
    let msg = message();
    let (json, packed, warnings, networks) = map::to_rest_packed(&msg, "Staged", 1_000);
    let whole = build(&json, packed, networks);

    let mut firsts: Vec<Document> = Vec::new();
    let mut hook = |json: serde_json::Value, packed, networks| firsts.push(build(&json, packed, networks));
    let (json2, packed2, warnings2, networks2) = map::to_rest_packed_staged(&msg, "Staged", 1_000, &mut hook);
    let staged = build(&json2, packed2, networks2);

    assert_eq!(warnings, warnings2);
    assert_eq!(json_of(&whole), json_of(&staged));

    assert_eq!(firsts.len(), 1, "the first page is handed over once");
    let first = &firsts[0];
    let page_names: Vec<&str> = first.pages.iter().filter_map(|p| first.node(*p)).map(|p| p.name.as_str()).collect();
    assert_eq!(page_names, ["First"]);
    assert!(first.nodes().any(|n| n.name == "First rect"));
    // "Second use" carries a copy of its main's layers; the main itself is on the page not built yet.
    assert!(!first.nodes().any(|n| n.name == "Second main"));
    // The library main travels with the first page; a main on a page not built yet does not.
    assert_eq!(linked(first, "Library use").as_deref(), Some("Library main"));
    assert_eq!(linked(first, "Second use"), None);
    assert_eq!(linked(&staged, "Second use").as_deref(), Some("Second main"));
}

#[test]
fn a_single_page_file_has_no_early_stage() {
    let nodes = vec![
        node(1, None, "DOCUMENT", "Document", vec![]),
        node(2, Some(1), "CANVAS", "Only", vec![]),
        node(30, Some(2), "RECTANGLE", "R", vec![("size", xy(5.0, 5.0))]),
    ];
    let mut calls = 0;
    let mut hook = |_: serde_json::Value, _, _| calls += 1;
    map::to_rest_packed_staged(&o(vec![("nodeChanges", Kv::Array(nodes))]), "One", 1_000, &mut hook);
    assert_eq!(calls, 0);
}
