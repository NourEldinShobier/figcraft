//! `selection.context`: the selection with its family, bindings, siblings and comments.

use serde_json::{Value, json};

use crate::Session;
use crate::board_tests::id;

fn make(s: &mut Session, p: Value) -> u64 {
    id(&s.execute("node.create", &p).unwrap(), "id").0
}

#[test]
fn an_empty_selection_says_so() {
    let mut s = Session::new();
    s.execute("select.set", &json!({"ids": []})).unwrap();
    let r = s.execute("selection.context", &json!({})).unwrap();
    assert_eq!(r["count"], 0, "{r}");
    assert_eq!((r["layers"].clone(), r["containers"].clone(), r["comments"].clone()), (json!([]), json!([]), json!([])));
}

#[test]
fn describes_layers_siblings_twins_bindings_and_comments() {
    let mut s = Session::new();
    let screen = make(&mut s, json!({"type": "frame", "name": "Home", "x": 0, "y": 0, "width": 400, "height": 300}));
    let card = make(&mut s, json!({"type": "frame", "name": "Card", "parent": screen, "x": 10, "y": 10, "width": 120, "height": 80}));
    let other = make(&mut s, json!({"type": "rectangle", "name": "Divider", "parent": screen, "x": 0, "y": 200, "width": 400, "height": 2}));
    let screen2 = make(&mut s, json!({"type": "frame", "name": "Detail", "x": 600, "y": 0, "width": 400, "height": 300}));
    make(&mut s, json!({"type": "frame", "name": "Card", "parent": screen2, "x": 10, "y": 10, "width": 120, "height": 80}));
    s.execute("variable.createCollection", &json!({"name": "Theme"})).unwrap();
    s.execute("variable.create", &json!({"collection": "Theme", "name": "surface", "type": "color", "value": "#112233"})).unwrap();
    s.execute("variable.bind", &json!({"ids": [card], "field": "fills/0/color", "variable": "surface"})).unwrap();
    s.execute("comment.add", &json!({"x": 20, "y": 20, "text": "Make it rounder", "anchor": card})).unwrap();
    let old = id(&s.execute("comment.add", &json!({"x": 30, "y": 30, "text": "Old one", "anchor": card})).unwrap(), "id").0;
    s.execute("comment.resolve", &json!({"id": old, "resolved": true})).unwrap();
    s.execute("select.set", &json!({"ids": [card]})).unwrap();

    let r = s.execute("selection.context", &json!({})).unwrap();
    assert_eq!(r["count"], 1, "{r}");
    let l = &r["layers"][0];
    assert_eq!(
        (l["id"].as_u64(), l["name"].as_str(), l["parent"]["name"].as_str(), l["frame"]["name"].as_str()),
        (Some(card), Some("Card"), Some("Home"), Some("Home")),
        "{l}"
    );
    assert_eq!(l["family"]["twins"], 1, "{l}");
    assert_eq!(l["family"]["sample"][0]["frame"]["name"], "Detail", "{l}");
    assert_eq!(l["variables"][0]["name"], "surface", "{l}");
    let c = &r["containers"][0];
    assert_eq!((c["name"].as_str(), c["total"].as_u64()), (Some("Home"), Some(2)), "{c}");
    assert_eq!((c["siblings"][0]["selected"].clone(), c["siblings"][1]["id"].as_u64()), (json!(true), Some(other)), "{c}");
    assert!(c["siblings"][1].get("selected").is_none());
    assert_eq!(r["comments"].as_array().unwrap().len(), 1, "{r}");
    assert_eq!(r["comments"][0]["text"], "Make it rounder");
}

#[test]
fn an_instance_reports_its_component_and_limits_cut_the_lists() {
    let mut s = Session::new();
    let f = make(&mut s, json!({"type": "frame", "name": "Button", "x": 0, "y": 0, "width": 100, "height": 40}));
    let comp = id(&s.execute("component.create", &json!({"ids": [f]})).unwrap(), "id").0;
    let inst = id(&s.execute("instance.create", &json!({"component": comp, "x": 200, "y": 0})).unwrap(), "id").0;
    let extra = make(&mut s, json!({"type": "rectangle", "x": 400, "y": 0, "width": 10, "height": 10}));
    s.execute("select.set", &json!({"ids": [inst, extra]})).unwrap();
    let r = s.execute("selection.context", &json!({"limit": 1, "siblings": 1})).unwrap();
    assert_eq!(r["layers"].as_array().unwrap().len(), 1, "{r}");
    assert_eq!(r["layers"][0]["component"]["id"].as_u64(), Some(comp), "{r}");
    assert_eq!(r["containers"][0]["siblings"].as_array().unwrap().len(), 1, "{r}");
    assert!(r["truncated"].as_str().unwrap().contains("1 of 2"), "{r}");
}

#[test]
fn junk_limits_are_refused() {
    let mut s = Session::new();
    for junk in [json!({"limit": -1}), json!({"limit": "x"}), json!({"siblings": 501}), json!({"siblings": 1.5})] {
        assert!(s.execute("selection.context", &junk).is_err(), "{junk}");
    }
}
