//! `--profile core|full`: the short tool list, the hidden-tool error and `more_tools`.

use figcraft_mcp::{CORE_TOOLS, Profile, Server, TOOL_NAMES};
use serde_json::{Value, json};

fn rpc(s: &mut Server, id: u64, method: &str, params: Value) -> Value {
    let line = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string();
    serde_json::from_str(&s.handle_line(&line).unwrap()).unwrap()
}

fn server(p: Profile) -> Server {
    Server::headless().with_profile(p)
}

fn text(r: &Value) -> String {
    r["result"]["content"][0]["text"].as_str().unwrap_or("").to_string()
}

#[test]
fn core_lists_the_short_set_with_short_descriptions() {
    let mut s = server(Profile::Core);
    let r = rpc(&mut s, 1, "tools/list", json!({}));
    let tools = r["result"]["tools"].as_array().unwrap();
    let mut names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    names.sort_unstable();
    let mut want: Vec<&str> = CORE_TOOLS.to_vec();
    want.push("more_tools");
    want.sort_unstable();
    assert_eq!(names, want);
    assert_eq!(CORE_TOOLS.len(), 21);
    assert_eq!(r["result"]["_meta"]["profile"], "core");
    assert!(r["result"]["_meta"]["note"].as_str().unwrap().contains("more_tools"));
    let full = rpc(&mut server(Profile::Full), 1, "tools/list", json!({}));
    let size = |v: &Value| v["result"]["tools"].to_string().len();
    assert!(size(&r) * 2 < size(&full), "core {} vs full {}", size(&r), size(&full));
    let rc = tools.iter().find(|t| t["name"] == "run_command").unwrap();
    let d = rc["description"].as_str().unwrap();
    assert!(d.contains("Args: command"), "{d}");
    assert!(d.len() < 400, "{d}");
}

#[test]
fn full_is_unchanged() {
    for mut s in [Server::headless(), server(Profile::Full)] {
        let r = rpc(&mut s, 1, "tools/list", json!({}));
        let names: Vec<&str> = r["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, TOOL_NAMES);
        assert_eq!(r["result"]["_meta"]["profile"], "full");
    }
}

#[test]
fn a_hidden_tool_fails_with_how_to_enable_it() {
    let mut s = server(Profile::Core);
    let r = rpc(&mut s, 1, "tools/call", json!({"name": "lint_layout", "arguments": {}}));
    assert_eq!(r["result"]["isError"], true);
    let t = text(&r);
    assert!(t.contains("hidden in the core profile") && t.contains("--profile full") && t.contains("FIGCRAFT_MCP_PROFILE"), "{t}");
    // A core tool still works, and a hidden tool works in full.
    assert_eq!(rpc(&mut s, 2, "tools/call", json!({"name": "diagnostics", "arguments": {}}))["result"]["isError"], false);
    let mut f = server(Profile::Full);
    assert_eq!(rpc(&mut f, 1, "tools/call", json!({"name": "get_design_guidelines", "arguments": {}}))["result"]["isError"], false);
    // Unknown names stay protocol errors.
    assert!(rpc(&mut s, 3, "tools/call", json!({"name": "nope", "arguments": {}})).get("error").is_some());
}

#[test]
fn more_tools_lists_what_is_hidden_and_filters() {
    let mut s = server(Profile::Core);
    let all = text(&rpc(&mut s, 1, "tools/call", json!({"name": "more_tools", "arguments": {}})));
    assert!(all.contains("lint_layout: ") && all.contains("--profile full"), "{all}");
    assert!(!all.contains("\nrun_command: "), "{all}");
    let hidden = TOOL_NAMES.len() - CORE_TOOLS.len();
    assert!(all.starts_with(&format!("{hidden} hidden tool(s)")), "{all}");
    let some = text(&rpc(&mut s, 2, "tools/call", json!({"name": "more_tools", "arguments": {"filter": "lint_layout"}})));
    assert!(some.starts_with("1 hidden") && some.contains("lint_layout"), "{some}");
    let none = text(&rpc(&mut server(Profile::Full), 3, "tools/call", json!({"name": "more_tools", "arguments": {}})));
    assert!(none.contains("nothing is hidden"));
}

#[test]
fn instructions_mention_the_profile() {
    let init = json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}});
    let r = rpc(&mut server(Profile::Core), 1, "initialize", init.clone());
    assert!(r["result"]["instructions"].as_str().unwrap().contains("Tool profile: core"));
    let r = rpc(&mut server(Profile::Full), 1, "initialize", init);
    assert!(r["result"]["instructions"].as_str().unwrap().contains("Tool profile: full"));
}

#[test]
fn flag_beats_env_and_default_is_full() {
    assert_eq!(Profile::resolve(None, None), Ok(Profile::Full));
    assert_eq!(Profile::resolve(None, Some("")), Ok(Profile::Full));
    assert_eq!(Profile::resolve(None, Some("core")), Ok(Profile::Core));
    assert_eq!(Profile::resolve(None, Some(" CORE ")), Ok(Profile::Core));
    assert_eq!(Profile::resolve(Some("full"), Some("core")), Ok(Profile::Full));
    assert_eq!(Profile::resolve(Some("core"), Some("full")), Ok(Profile::Core));
    assert!(Profile::resolve(None, Some("tiny")).unwrap_err().contains("FIGCRAFT_MCP_PROFILE"));
    assert!(Profile::resolve(Some("tiny"), None).is_err());
}
