//! `design_system_index`: usage counts and unused assets, registered read-only, documented.

use crate::common::Client;
use figcraft_mcp::TOOL_NAMES;
use serde_json::json;

#[test]
fn design_system_index_counts_instances_and_lists_unused_components() {
    let mut c = Client::new();
    let a = c.create(json!({"type": "rectangle", "name": "Used", "width": 40, "height": 40}));
    let b = c.create(json!({"type": "rectangle", "name": "Spare", "width": 40, "height": 40}));
    c.run("component.create", json!({"ids": [a]}));
    c.run("component.create", json!({"ids": [b]}));
    let r = c.ok("design_system_index", json!({"kinds": ["components"]}));
    assert_eq!(r["components"]["total"], 2, "{r}");
    assert_eq!(r["totals"]["unused"]["components"], 2, "{r}");
    assert!(r.get("variables").is_none(), "{r}");
    assert!(!c.err("design_system_index", json!({"kinds": ["bogus"]})).is_empty());
}

#[test]
fn design_system_index_is_registered_read_only_and_documented() {
    let mut c = Client::new();
    assert!(TOOL_NAMES.contains(&"design_system_index"));
    let tools = c.rpc("tools/list", json!({}))["result"]["tools"].clone();
    let t = tools.as_array().unwrap().iter().find(|t| t["name"] == "design_system_index").unwrap().clone();
    assert_eq!(t["annotations"]["readOnlyHint"], true);
    assert!(t["inputSchema"]["properties"]["document"].is_object());
    assert!(include_str!("../../../../docs/mcp.md").contains("| `design_system_index {"));
    let s = tools.as_array().unwrap().iter().find(|t| t["name"] == "search_design_system").unwrap().clone();
    assert!(s["description"].as_str().unwrap().contains("design_system_index"));
}
