//! `get_board` and `get_selection_context`: registered, read-only, documented, and working
//! without a board or a selection.

use crate::common::Client;
use figcraft_mcp::TOOL_NAMES;
use serde_json::json;

#[test]
fn a_design_file_has_no_board_and_an_empty_selection_is_count_zero() {
    let mut c = Client::new();
    assert!(!c.err("get_board", json!({})).is_empty());
    let r = c.ok("get_selection_context", json!({}));
    assert_eq!(r["count"], 0, "{r}");
    assert!(!c.err("get_selection_context", json!({"limit": "x"})).is_empty());
}

#[test]
fn a_board_and_a_selection_read_through_the_tools() {
    let mut c = Client::new();
    c.run("file.newBoard", json!({"name": "Ideas"}));
    let empty = c.ok("get_board", json!({}));
    assert_eq!(empty["summary"]["stickies"], 0, "{empty}");
    let a = c.run("board.sticky", json!({"x": 0, "y": 0, "text": "Hello"}));
    let r = c.ok("get_board", json!({}));
    assert_eq!(r["sections"][0]["items"][0]["text"], "Hello", "{r}");
    assert_eq!(r["sections"][0]["items"][0]["id"], a["id"], "{r}");
    c.run("select.set", json!({"ids": [a["id"]]}));
    let s = c.ok("get_selection_context", json!({}));
    assert_eq!(s["count"], 1, "{s}");
    assert_eq!(s["layers"][0]["id"], a["id"], "{s}");
}

#[test]
fn board_and_selection_tools_are_registered_and_documented() {
    let mut c = Client::new();
    let tools = c.rpc("tools/list", json!({}))["result"]["tools"].clone();
    for name in ["get_board", "get_selection_context"] {
        assert!(TOOL_NAMES.contains(&name));
        let t = tools.as_array().unwrap().iter().find(|t| t["name"] == name).unwrap().clone();
        assert_eq!(t["annotations"]["readOnlyHint"], true);
        assert!(t["inputSchema"]["properties"]["document"].is_object());
        assert!(include_str!("../../../../docs/mcp.md").contains(&format!("| `{name} {{")));
    }
}
