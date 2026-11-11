//! `batch` assert steps and `preview`, through the MCP tool.

use crate::common::Client;
use serde_json::json;

fn scene(c: &mut Client) -> (serde_json::Value, serde_json::Value) {
    let f = c.create(json!({"type": "frame", "name": "Screen", "width": 200, "height": 100}));
    let a = c.create(json!({"type": "rectangle", "name": "A", "parent": f, "x": 10, "y": 10, "width": 50, "height": 50}));
    (f, a)
}

#[test]
fn an_assert_step_passes_or_rolls_the_batch_back() {
    let mut c = Client::new();
    let (f, a) = scene(&mut c);
    let ok = c.ok(
        "batch",
        json!({"commands": [
            {"command": "node.set", "params": {"id": a, "props": {"x": 20}}},
            {"assert": {"noOverflow": {"root": f}, "exists": {"where": {"name": "A"}}}}
        ]}),
    );
    assert_eq!(ok[1]["ok"], true, "{ok}");

    let e = c.err(
        "batch",
        json!({"commands": [
            {"command": "node.set", "params": {"id": a, "props": {"x": 180}}},
            {"assert": {"noOverflow": {}}}
        ]}),
    );
    assert!(e.contains("step 1") && e.contains("noOverflow") && e.contains("rolled back"), "{e}");
    assert_eq!(c.node(&a)["x"], 20.0);

    assert!(!c.err("batch", json!({"commands": [{"assert": 5}]})).is_empty());
    assert!(!c.err("batch", json!({"commands": [{"assert": {}, "command": "node.set"}]})).is_empty());
    assert!(!c.err("batch", json!({"commands": [{"assert": {"bogus": {}}}]})).is_empty());
}

#[test]
fn preview_shows_the_diff_and_changes_nothing() {
    let mut c = Client::new();
    let (_, a) = scene(&mut c);
    let r = c.ok("batch", json!({"preview": true, "quiet": true, "commands": [{"command": "node.set", "params": {"id": a, "props": {"x": 40}}}]}));
    assert_eq!(r["preview"], true, "{r}");
    assert_eq!(r["diff"]["counts"]["changed"], 1, "{r}");
    assert_eq!(c.node(&a)["x"], 10.0);
    assert!(!c.err("batch", json!({"preview": "yes", "commands": [{"command": "edit.selectAll"}]})).is_empty());
}
