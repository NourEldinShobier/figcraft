//! `check_accessibility`: audit counts, alt text through set_properties/get_node, registered read-only, documented.

use crate::common::Client;
use figcraft_mcp::TOOL_NAMES;
use serde_json::json;

#[test]
fn check_accessibility_counts_findings_and_alt_text_round_trips() {
    let mut c = Client::new();
    let f = c.create(json!({"type": "frame", "name": "Screen", "width": 400, "height": 800}));
    let b = c.create(json!({"type": "frame", "name": "Button", "parent": f, "x": 0, "y": 0, "width": 20, "height": 20}));
    let v = c.ok("check_accessibility", json!({}));
    assert_eq!(v["checks"]["targets"]["failures"], 1, "{v}");
    assert_eq!(v["details"]["targets"][0]["id"], json!(b), "{v}");
    c.ok("set_properties", json!({"ids": [b], "props": {"alt": "Submit"}}));
    let n = c.ok("get_node", json!({"id": b}));
    assert_eq!(n["alt"], "Submit", "{n}");
    assert!(n.get("pluginData").is_none(), "{n}");
    assert!(!c.err("check_accessibility", json!({"limit": -1})).is_empty());
}

#[test]
fn check_accessibility_is_registered_read_only_and_documented() {
    let mut c = Client::new();
    assert!(TOOL_NAMES.contains(&"check_accessibility"));
    let tools = c.rpc("tools/list", json!({}))["result"]["tools"].clone();
    let t = tools.as_array().unwrap().iter().find(|t| t["name"] == "check_accessibility").unwrap().clone();
    assert_eq!(t["annotations"]["readOnlyHint"], true);
    assert!(t["inputSchema"]["properties"]["document"].is_object());
    assert!(include_str!("../../../../docs/mcp.md").contains("| `check_accessibility {"));
}
