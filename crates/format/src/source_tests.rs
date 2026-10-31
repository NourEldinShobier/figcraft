//! The plain-text design source: byte-identical saves, one changed fill = one changed line, and
//! source -> import -> source is the identity.

use figcraft_doc::color::{Color, Paint};
use figcraft_doc::{Document, NodeKind};

use crate::source::{export_source, import_source};

fn sample() -> Document {
    let mut d = Document::new("Sample");
    let page = d.first_page().unwrap();
    let card = d.create(page, NodeKind::Frame, "Card", 10.0, 20.0, 200.0, 100.0).unwrap();
    d.node_mut(card).unwrap().set_key(Some("card"));
    let bg = d.create(card, NodeKind::Rectangle, "Bg", 0.0, 0.0, 200.0, 100.0).unwrap();
    d.node_mut(bg).unwrap().fills = vec![Paint::solid(Color::new(1.0, 0.0, 0.0, 1.0))];
    d.create(card, NodeKind::Ellipse { start: 0.0, sweep: std::f64::consts::TAU, inner: 0.0 }, "Dot", 5.0, 5.0, 10.0, 10.0).unwrap();
    d.add_page("Second");
    d
}

#[test]
fn saving_twice_gives_identical_text_with_sorted_keys_and_no_children_lists() {
    let d = sample();
    let a = export_source(&d).unwrap();
    assert_eq!(a, export_source(&d.clone()).unwrap());
    assert!(a.lines().next().unwrap().starts_with(r#"{"sourceVersion":1,"t":"doc","v":{"#), "{a}");
    let node_lines: Vec<&str> = a.lines().filter(|l| l.contains(r#""t":"node""#)).collect();
    // Two pages, the frame and its two children, in tree order.
    assert_eq!(node_lines.len(), 5, "{a}");
    assert!(node_lines[1].contains(r#""name":"Card""#) && node_lines[2].contains(r#""name":"Bg""#) && node_lines[3].contains(r#""name":"Dot""#));
    assert!(node_lines[1].contains(r#""figcraft.key":"card""#));
    assert!(!a.contains("children"));
}

#[test]
fn changing_one_fill_changes_exactly_one_line() {
    let mut d = sample();
    let before = export_source(&d).unwrap();
    let bg = d.nodes().find(|n| n.name == "Bg").unwrap().id;
    d.node_mut(bg).unwrap().fills = vec![Paint::solid(Color::new(0.0, 1.0, 0.0, 1.0))];
    let after = export_source(&d).unwrap();
    let (a, b): (Vec<&str>, Vec<&str>) = (before.lines().collect(), after.lines().collect());
    assert_eq!(a.len(), b.len());
    let changed: Vec<usize> = (0..a.len()).filter(|i| a[*i] != b[*i]).collect();
    assert_eq!(changed.len(), 1, "{changed:?}");
    assert!(a[changed[0]].contains(r#""name":"Bg""#));
}

#[test]
fn source_import_source_is_the_identity_and_keeps_the_tree() {
    let d = sample();
    let text = export_source(&d).unwrap();
    let back = import_source(&text).unwrap();
    assert!(back.issues.is_empty(), "{:?}", back.issues);
    assert_eq!(export_source(&back.doc).unwrap(), text);
    assert_eq!(back.doc.pages.len(), 2);
    let card = back.doc.node_by_key("card").unwrap();
    let names: Vec<&str> = back.doc.get(card).unwrap().children.iter().filter_map(|c| back.doc.node(*c)).map(|n| n.name.as_str()).collect();
    assert_eq!(names, ["Bg", "Dot"]);
}

#[test]
fn bad_source_is_refused_with_the_line_number() {
    let e = import_source("{\"t\":\"doc\",\"sourceVersion\":1,\"v\":{}}\nnot json\n").err().unwrap().to_string();
    assert!(e.contains("source line 2"), "{e}");
    assert!(import_source("").is_err());
    assert!(import_source("{\"t\":\"doc\",\"sourceVersion\":99,\"v\":{}}\n").is_err());
}
