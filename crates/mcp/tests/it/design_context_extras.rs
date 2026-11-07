//! `get_design_context` extras: the `include` list, bindings with CSS fallbacks, annotations.

use crate::common::Client;
use serde_json::json;

#[test]
fn get_design_context_adds_the_requested_extras() {
    let mut c = Client::new();
    let f = c.create(json!({"type": "frame", "name": "Card", "width": 100, "height": 50}));
    let r = c.create(json!({"type": "rectangle", "parent": f, "width": 10, "height": 10}));
    c.run("variable.createCollection", json!({"name": "Tokens"}));
    c.run("variable.create", json!({"collection": "Tokens", "name": "color/brand", "type": "color", "value": "#3366FF"}));
    c.run("variable.bind", json!({"ids": [r], "field": "fill", "variable": "color/brand"}));
    c.run("annotation.add", json!({"id": r, "label": "Fade in"}));

    let d = c.ok("get_design_context", json!({"nodeId": f}));
    assert_eq!(d["annotations"][0]["label"], "Fade in");
    assert_eq!(d["bindings"][0]["bound"]["fill"]["css"], "var(--color-brand, #3366FF)");
    assert!(d.get("comments").is_none(), "heavy lists are opt-in");

    let only = c.ok("get_design_context", json!({"nodeId": f, "include": ["hints"]}));
    assert!(only.get("bindings").is_none() && only["hints"].as_array().is_some_and(|h| h.len() >= 2), "{only}");
    c.err("get_design_context", json!({"nodeId": f, "include": ["bogus"]}));
}

#[test]
fn get_design_context_documents_include() {
    assert!(include_str!("../../../../docs/mcp.md").contains("| `get_design_context {nodeId?, language?, depth?, include?}`"));
}
