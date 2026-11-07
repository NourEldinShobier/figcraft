//! `find_similar`, `propagate_change` and the `similarOn` hint on a `set_properties` result.

use crate::common::Client;
use serde_json::{Value, json};

/// A "Header" frame with a "Logo" rectangle in it; returns (frame, logo).
fn header(c: &mut Client, name: &str, x: i64) -> (Value, Value) {
    let frame = c.create(json!({"type": "frame", "name": name, "x": x, "y": 0, "width": 300, "height": 60}));
    let logo = c.create(json!({"type": "rectangle", "name": "Logo", "parent": frame, "x": 10, "y": 10, "width": 40, "height": 40}));
    (frame, logo)
}

fn fill_of(c: &mut Client, id: &Value) -> Value {
    c.ok("get_node", json!({"id": id, "depth": 0}))["fills"][0]["color"].clone()
}

#[test]
fn an_edit_names_its_twins_and_propagate_change_previews_then_applies() {
    let mut c = Client::new();
    let (a, logo_a) = header(&mut c, "Header", 0);
    let (b, logo_b) = header(&mut c, "Header 2", 400);

    let r = c.ok("find_similar", json!({"id": a}));
    assert_eq!(r["twins"][0]["id"], b, "{r}");

    let r = c.ok("set_properties", json!({"ids": [a], "props": {"fills": [{"type": "solid", "color": "#00AA00"}]}}));
    assert_eq!(r["similarOn"]["twins"][0]["id"], b, "the edit names its twin: {r}");
    assert!(r["similarOn"]["hint"].as_str().unwrap().contains("propagate_change"), "{r}");

    c.ok("set_properties", json!({"ids": [logo_a], "props": {"fills": [{"type": "solid", "color": "#FF0000"}]}}));
    let before = fill_of(&mut c, &logo_b);
    assert_ne!(before, fill_of(&mut c, &logo_a), "the logos start different");

    let preview = c.ok("propagate_change", json!({"from": a}));
    assert_eq!(preview["preview"], true, "{preview}");
    assert_eq!(fill_of(&mut c, &logo_b), before, "a preview changes nothing");

    let done = c.ok("propagate_change", json!({"from": a, "confirm": true}));
    assert!(done["changed"].as_array().is_some_and(|l| !l.is_empty()), "{done}");
    assert_eq!(fill_of(&mut c, &logo_b), fill_of(&mut c, &logo_a));
    assert_eq!(c.ok("get_node", json!({"id": b, "depth": 0}))["name"], "Header 2", "names stay");

    c.ok("undo", json!({}));
    assert_eq!(fill_of(&mut c, &logo_b), before, "one undo step");
}

#[test]
fn bad_arguments_are_errors() {
    let mut c = Client::new();
    assert!(c.err("propagate_change", json!({})).contains("from"));
    assert!(c.err("propagate_change", json!({"from": 1, "confirm": "yes"})).contains("boolean"));
    assert!(!c.err("find_similar", json!({"id": 999_999})).is_empty());
    assert!(!c.err("find_similar", json!({"minScore": 3, "id": 1})).is_empty());
}

#[test]
fn code_connect_tools_read_the_file_and_refuse_a_missing_layer() {
    let mut c = Client::new();
    let r = c.ok("get_code_connect_map", json!({}));
    assert_eq!(r["links"], json!([]), "{r}");
    assert!(!c.err("add_code_connect_map", json!({"id": 999_999, "name": "Button"})).is_empty());
    assert!(!c.err("get_code_connect_map", json!({"id": 999_999})).is_empty());
}
