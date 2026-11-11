//! `changesets`: name a task's edits, then revert just that change.

use crate::common::Client;
use figcraft_mcp::TOOL_NAMES;
use serde_json::{Value, json};

fn opacity(c: &mut Client, id: &Value) -> f64 {
    c.ok("get_node", json!({"id": id, "depth": 0}))["opacity"].as_f64().unwrap_or(1.0)
}

fn dim(c: &mut Client, label: &str, id: &Value, to: f64) -> Value {
    let begun = c.ok("changesets", json!({"action": "begin", "label": label}));
    let r = c.ok("set_properties", json!({"ids": [id], "props": {"opacity": to}}));
    assert_eq!(r["changed"]["changeset"], json!(label), "a mutating result names the open changeset: {r}");
    c.ok("changesets", json!({"action": "end"}));
    begun["id"].clone()
}

#[test]
fn the_middle_change_reverts_and_an_overlapping_one_is_refused() {
    let mut c = Client::new();
    let rects: Vec<Value> = (0..3).map(|i| c.create(json!({"type": "rectangle", "x": i * 50, "y": 0, "width": 40, "height": 40}))).collect();
    let ids: Vec<Value> = ["dim 0", "dim 1", "dim 2"].iter().zip(&rects).map(|(l, r)| dim(&mut c, l, r, 0.5)).collect();
    let after = c.ok("set_properties", json!({"ids": [rects[0]], "props": {"opacity": 0.2}}));
    assert!(after["changed"].get("changeset").is_none(), "{after}");

    let list = c.ok("changesets", json!({"action": "list"}));
    assert_eq!(list["total"], 3, "{list}");
    assert_eq!(list["changesets"][2]["conflicts"], 1, "dim 0 was touched again: {list}");
    let e = c.err("changesets", json!({"action": "revert", "id": ids[0]}));
    assert!(e.contains("refused") && e.contains("restore_version"), "{e}");
    assert!((opacity(&mut c, &rects[0]) - 0.2).abs() < 1e-6, "nothing changed");

    let r = c.ok("changesets", json!({"action": "revert", "id": ids[1]}));
    assert_eq!((r["label"].clone(), r["reverted"].clone()), (json!("dim 1"), json!(1)), "{r}");
    assert_eq!(opacity(&mut c, &rects[1]), 1.0);
    assert_eq!(opacity(&mut c, &rects[2]), 0.5);
}

#[test]
fn junk_arguments_are_errors() {
    let mut c = Client::new();
    for junk in [
        json!({}),
        json!({"action": "nope"}),
        json!({"action": "begin"}),
        json!({"action": "begin", "label": ""}),
        json!({"action": "begin", "label": ["x"]}),
        json!({"action": "end"}),
        json!({"action": "revert"}),
        json!({"action": "revert", "id": "x"}),
        json!({"action": "revert", "id": 99}),
        json!({"action": "list", "limit": 0}),
        json!({"action": "list", "limit": "many"}),
    ] {
        assert!(!c.err("changesets", junk.clone()).is_empty(), "{junk}");
    }
}

#[test]
fn the_tool_is_registered_and_documented() {
    let mut c = Client::new();
    let tools = c.rpc("tools/list", json!({}))["result"]["tools"].clone();
    assert!(TOOL_NAMES.contains(&"changesets"));
    let t = tools.as_array().unwrap().iter().find(|t| t["name"] == "changesets").unwrap().clone();
    assert!(t["inputSchema"]["properties"]["document"].is_object());
    assert!(include_str!("../../../../docs/mcp.md").contains("| `changesets {"), "needs a row in docs/mcp.md");
}
