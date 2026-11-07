//! `lint_layout`: findings with ready fixes, registered read-only, documented.

use crate::common::Client;
use figcraft_mcp::TOOL_NAMES;
use serde_json::json;

#[test]
fn lint_layout_reports_an_overlap_and_a_misalignment_with_runnable_fixes() {
    let mut c = Client::new();
    let f = c.create(json!({"type": "frame", "name": "Screen", "width": 1000, "height": 800}));
    let a = c.create(json!({"type": "rectangle", "parent": f, "x": 100, "y": 0, "width": 80, "height": 80}));
    let b = c.create(json!({"type": "rectangle", "parent": f, "x": 102, "y": 40, "width": 80, "height": 80}));

    let r = c.ok("lint_layout", json!({}));
    let found = r["findings"].as_array().unwrap();
    assert!(found.iter().any(|x| x["check"] == "overlap" && x["ids"] == json!([a, b])), "{r}");
    let fix = found.iter().find(|x| x["fix"]["command"] == "align.left").unwrap_or_else(|| panic!("no align fix: {r}"))["fix"].clone();

    c.run(fix["command"].as_str().unwrap(), fix["params"].clone());
    let after = c.ok("lint_layout", json!({"checks": ["misaligned"]}));
    assert_eq!(after["total"], 0, "{after}");

    assert!(!c.err("lint_layout", json!({"checks": ["bogus"]})).is_empty());
}

#[test]
fn lint_layout_is_registered_read_only_and_documented() {
    let mut c = Client::new();
    assert!(TOOL_NAMES.contains(&"lint_layout"));
    let tools = c.rpc("tools/list", json!({}))["result"]["tools"].clone();
    let t = tools.as_array().unwrap().iter().find(|t| t["name"] == "lint_layout").unwrap().clone();
    assert_eq!(t["annotations"]["readOnlyHint"], true);
    assert!(t["inputSchema"]["properties"]["document"].is_object());
    assert!(include_str!("../../../../docs/mcp.md").contains("| `lint_layout {"));
}
