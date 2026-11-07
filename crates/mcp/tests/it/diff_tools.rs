//! `diff_documents` and `summarize_document`: structural diff, token-budgeted overview.

use crate::common::Client;
use figcraft_mcp::TOOL_NAMES;
use serde_json::json;

#[test]
fn diff_documents_compares_two_open_files_by_path_and_a_version_by_id() {
    let mut c = Client::new();
    let a = c.run("doc.list", json!({}))[0]["id"].clone();
    c.create(json!({"type": "frame", "name": "Home", "width": 300, "height": 200}));
    c.create(json!({"type": "frame", "name": "Old", "width": 50, "height": 50}));
    c.run("file.new", json!({"name": "B"}));
    let b = c.run("doc.list", json!({}))[1]["id"].clone();
    let home = c.create(json!({"type": "frame", "name": "Home", "width": 320, "height": 200}));
    c.create(json!({"type": "frame", "name": "New", "width": 50, "height": 50}));

    let r = c.ok("diff_documents", json!({"a": a, "b": b}));
    assert_eq!(r["counts"], json!({"added": 1, "removed": 1, "moved": 0, "changed": 1}), "{r}");
    assert_eq!(r["changed"][0]["path"], "Page 1/Home", "{r}");
    assert_eq!(r["changed"][0]["id"], home, "{r}");
    assert!(r["changed"][0]["changes"].as_array().unwrap().iter().any(|x| x["prop"] == "width" && x["old"] == 300.0 && x["new"] == 320.0), "{r}");

    let r = c.ok("diff_documents", json!({"a": a, "b": b, "limit": 1}));
    assert_eq!(r["truncated"], true, "{r}");
    assert_eq!(r["changed"].as_array().unwrap().len(), 1, "{r}");

    let v = c.ok("checkpoint", json!({"name": "base"}));
    c.ok("set_properties", json!({"ids": [home], "props": {"name": "Home 2"}}));
    let r = c.ok("diff_documents", json!({"version": v["id"]}));
    assert_eq!(r["mode"], "version", "{r}");
    assert_eq!(r["counts"]["changed"], 1, "{r}");
    assert_eq!(r["changed"][0]["changes"][0]["prop"], "name", "{r}");
}

#[test]
fn diff_documents_rejects_bad_arguments() {
    let mut c = Client::new();
    assert!(!c.err("diff_documents", json!({})).is_empty());
    assert!(!c.err("diff_documents", json!({"a": 99999})).is_empty());
    assert!(!c.err("diff_documents", json!({"version": "nope"})).is_empty());
    assert!(!c.err("diff_documents", json!({"version": "x", "limit": -1})).is_empty());
}

#[test]
fn summarize_document_fits_its_budget_by_showing_less() {
    let mut c = Client::new();
    for i in 0..120 {
        c.create(json!({"type": "frame", "name": format!("Screen {i}"), "x": i * 500, "width": 400, "height": 800}));
    }
    let big = c.ok("summarize_document", json!({"budgetTokens": 50000}));
    assert_eq!(big["counts"]["layers"], 120, "{big}");
    assert_eq!(big["pages"][0]["frames"].as_array().unwrap().len(), 120, "{big}");
    let small = c.ok("summarize_document", json!({"budgetTokens": 500}));
    assert!(small["estTokens"].as_u64().unwrap() <= 500, "{small}");
    let shown = small["pages"][0]["frames"].as_array().map_or(0, Vec::len) as u64;
    assert_eq!(shown + small["pages"][0]["moreFrames"].as_u64().unwrap_or(0), 120, "{small}");
    assert!(small["drill"].is_array());
    assert!(!c.err("summarize_document", json!({"budgetTokens": 5})).is_empty());
}

#[test]
fn both_tools_are_registered_read_only_and_documented() {
    let mut c = Client::new();
    let tools = c.rpc("tools/list", json!({}))["result"]["tools"].clone();
    for name in ["diff_documents", "summarize_document"] {
        assert!(TOOL_NAMES.contains(&name));
        let t = tools.as_array().unwrap().iter().find(|t| t["name"] == name).unwrap().clone();
        assert_eq!(t["annotations"]["readOnlyHint"], true);
        assert!(t["inputSchema"]["properties"]["document"].is_object());
        assert!(include_str!("../../../../docs/mcp.md").contains(&format!("| `{name} {{")));
    }
}
