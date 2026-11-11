//! `extract_text` and `apply_text`: a translation round trip, registered, documented.

use crate::common::Client;
use figcraft_mcp::TOOL_NAMES;
use serde_json::json;

#[test]
fn extract_then_apply_round_trips_copy_and_reports_fit() {
    let mut c = Client::new();
    let f = c.create(json!({"type": "frame", "name": "Screen", "width": 300, "height": 200}));
    let t = c.create(json!({"type": "text", "parent": f, "characters": "Save", "x": 0, "y": 0}));
    let r = c.ok("extract_text", json!({}));
    assert_eq!(r["texts"][0]["characters"], "Save", "{r}");
    assert_eq!(r["texts"][0]["path"], json!(["Screen"]), "{r}");

    let r = c.ok("apply_text", json!({"changes": [{"id": t, "characters": "Speichern 🎉"}], "dryRun": true}));
    assert_eq!(r["applied"], 1, "{r}");
    assert_eq!(c.ok("extract_text", json!({}))["texts"][0]["characters"], "Save");
    c.ok("apply_text", json!({"changes": [{"id": t, "characters": "Speichern 🎉"}]}));
    assert_eq!(c.ok("extract_text", json!({"root": f}))["texts"][0]["characters"], "Speichern 🎉");

    assert!(!c.err("apply_text", json!({"changes": [{"id": t}]})).is_empty());
    assert!(!c.err("apply_text", json!({})).is_empty());
    assert!(!c.err("extract_text", json!({"limit": -3})).is_empty());
}

#[test]
fn text_inventory_tools_are_registered_and_documented() {
    let mut c = Client::new();
    let tools = c.rpc("tools/list", json!({}))["result"]["tools"].clone();
    for (name, read_only) in [("extract_text", true), ("apply_text", false)] {
        assert!(TOOL_NAMES.contains(&name));
        let t = tools.as_array().unwrap().iter().find(|t| t["name"] == name).unwrap().clone();
        assert_eq!(t["annotations"]["readOnlyHint"], read_only);
        assert!(t["inputSchema"]["properties"]["document"].is_object());
        assert!(include_str!("../../../../docs/mcp.md").contains(&format!("| `{name} {{")));
    }
}
