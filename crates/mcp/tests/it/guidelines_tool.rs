//! `get_design_guidelines`, `library.setGuidelines` through run_command, and the prompt.

use crate::common::Client;
use figcraft_mcp::TOOL_NAMES;
use serde_json::json;

#[test]
fn guidelines_round_trip_through_the_tools() {
    let mut c = Client::new();
    assert_eq!(c.ok("get_design_guidelines", json!({}))["text"], "");
    c.run("library.setGuidelines", json!({"text": "Use brand blue."}));
    let g = c.ok("get_design_guidelines", json!({}));
    assert_eq!((g["text"].clone(), g["bytes"].clone()), (json!("Use brand blue."), json!(15)));
    let f = c.create(json!({"type": "frame", "width": 10, "height": 10}));
    let d = c.ok("get_design_context", json!({"nodeId": f, "include": ["hints"]}));
    assert!(d["hints"][0].as_str().unwrap().contains("Use brand blue."), "{d}");
    assert!(TOOL_NAMES.contains(&"get_design_guidelines"));
    assert!(include_str!("../../../../docs/mcp.md").contains("| `get_design_guidelines {}`"));
}

#[test]
fn prompt_mentions_set_guidelines() {
    let mut c = Client::new();
    let r = c.rpc("prompts/get", json!({"name": "create_design_system_rules", "arguments": {"target": "CLAUDE.md"}}));
    let text = r["result"]["messages"][0]["content"]["text"].as_str().unwrap().to_string();
    assert!(text.contains("CLAUDE.md") && text.contains("library.setGuidelines"), "{text}");
}
