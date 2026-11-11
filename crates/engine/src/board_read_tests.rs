//! `board.read`: what an agent reads off a board.

use figcraft_doc::NodeId;
use serde_json::{Value, json};

use crate::Session;
use crate::board_tests::{board, id};

fn sticky(s: &mut Session, x: i32, text: &str) -> NodeId {
    id(&s.execute("board.sticky", &json!({"x": x, "y": 0, "text": text, "color": "blue"})).unwrap(), "id")
}

#[test]
fn reads_items_sections_connectors_and_tables() {
    let mut s = board();
    let a = sticky(&mut s, 0, "Ship it");
    let b = sticky(&mut s, 400, "Test it");
    let c = id(&s.execute("board.connector", &json!({"from": a, "to": b, "label": "then"})).unwrap(), "id");
    s.execute("board.shape", &json!({"shape": "diamond", "x": 0, "y": 300, "text": "Ok?"})).unwrap();
    s.execute("board.stamp", &json!({"x": 10, "y": 10, "stamp": "heart"})).unwrap();
    let t = id(&s.execute("board.table", &json!({"cells": [["Who", "What"], ["Ann", "Plan"]], "x": 0, "y": 600})).unwrap(), "id");
    let sec =
        id(&s.execute("node.create", &json!({"type": "section", "name": "Backlog", "x": 900, "y": 0, "width": 500, "height": 400})).unwrap(), "id");
    let inner = id(&s.execute("board.sticky", &json!({"x": 920, "y": 20, "text": "Inside", "parent": sec})).unwrap(), "id");

    let r: Value = s.execute("board.read", &json!({})).unwrap();
    assert_eq!(
        r["summary"],
        json!({"sections": 1, "stickies": 3, "shapes": 1, "texts": 0, "stamps": 1, "connectors": 1, "tables": 1, "ink": 0}),
        "{r}"
    );
    let loose = &r["sections"][0];
    assert!(loose["id"].is_null() && loose["name"].is_null(), "{r}");
    let first = &loose["items"][0];
    assert_eq!(
        (first["id"].as_u64(), first["type"].as_str(), first["text"].as_str(), first["color"].as_str()),
        (Some(a.0), Some("sticky"), Some("Ship it"), Some("blue")),
        "{r}"
    );
    let items = loose["items"].as_array().unwrap();
    assert!(items.iter().any(|i| i["type"] == "shape" && i["shape"] == "diamond" && i["text"] == "Ok?"), "{r}");
    assert!(items.iter().any(|i| i["type"] == "stamp" && i["stamp"] == "heart"), "{r}");
    assert_eq!(r["sections"][1]["name"], "Backlog", "{r}");
    let item = &r["sections"][1]["items"][0];
    assert_eq!((item["id"].as_u64(), item["text"].as_str()), (Some(inner.0), Some("Inside")), "{r}");
    let conn = &r["connectors"][0];
    assert_eq!(
        (conn["id"].as_u64(), conn["from"].as_u64(), conn["to"].as_u64(), conn["label"].as_str()),
        (Some(c.0), Some(a.0), Some(b.0), Some("then")),
        "{r}"
    );
    assert_eq!((conn["fromText"].as_str(), conn["toText"].as_str()), (Some("Ship it"), Some("Test it")), "{r}");
    assert_eq!(r["tables"][0]["id"].as_u64(), Some(t.0));
    assert_eq!(r["tables"][0]["rows"], json!([["Who", "What"], ["Ann", "Plan"]]), "{r}");
    assert!(r.get("truncated").is_none(), "{r}");
}

#[test]
fn limit_truncates_the_lists_not_the_counts() {
    let mut s = board();
    for i in 0..5 {
        sticky(&mut s, i * 300, &format!("n{i}"));
    }
    let r = s.execute("board.read", &json!({"limit": 2})).unwrap();
    assert_eq!(r["summary"]["stickies"], 5);
    assert_eq!(r["sections"][0]["items"].as_array().unwrap().len(), 2);
    assert!(r["truncated"].as_str().unwrap().contains("2 of 5"), "{r}");
}

#[test]
fn an_empty_board_reads_as_nothing() {
    let mut s = board();
    let r = s.execute("board.read", &json!({})).unwrap();
    assert_eq!(r["summary"]["stickies"], 0);
    assert_eq!(r["sections"], json!([]));
    assert_eq!((r["connectors"].clone(), r["tables"].clone()), (json!([]), json!([])));
}

#[test]
fn a_design_file_and_junk_parameters_are_refused() {
    let mut d = Session::new();
    assert!(d.execute("board.read", &json!({})).is_err());
    let mut s = board();
    for junk in [json!({"limit": -1}), json!({"limit": "x"}), json!({"limit": 99999}), json!({"page": "nope"}), json!({"page": 7})] {
        assert!(s.execute("board.read", &junk).is_err(), "{junk}");
    }
}
