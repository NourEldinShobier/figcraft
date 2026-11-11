//! `fix_contrast` and `export_assets`: results, write-root refusal, registered and documented.

use figcraft_engine::file_access::Roots;
use figcraft_mcp::{Server, TOOL_NAMES};
use serde_json::{Value, json};

use crate::common::Client;

#[test]
fn fix_contrast_changes_failing_text_and_reports_variable_bound_text() {
    let mut c = Client::new();
    let f = c.create(json!({"type": "frame", "width": 200, "height": 100, "props": {"fill": "#FFFFFF"}}));
    let t = c.create(json!({"type": "text", "text": "Hello", "parent": f, "x": 10, "y": 10, "props": {"fill": "#BBBBBB"}}));
    let u = c.create(json!({"type": "text", "text": "Bound", "parent": f, "x": 10, "y": 50, "props": {"fill": "#BBBBBB"}}));
    c.run("variable.createCollection", json!({"name": "Tokens"}));
    c.run("variable.create", json!({"collection": "Tokens", "name": "Muted", "type": "color", "value": "#BBBBBB"}));
    c.run("variable.bind", json!({"ids": [u], "field": "fill", "variable": "Muted"}));
    let dry = c.ok("fix_contrast", json!({"dryRun": true}));
    assert_eq!(dry["changed"].as_array().unwrap().len(), 1, "{dry}");
    assert_eq!(dry["skipped"][0]["variable"], "Muted", "{dry}");
    let r = c.ok("fix_contrast", json!({"root": f}));
    assert_eq!(r["changed"][0]["layer"], t, "{r}");
    assert_eq!(r["remaining"], 1, "{r}");
    let fills = c.node(&t)["fills"].clone();
    assert_ne!(fills[0]["color"], "#BBBBBB", "{fills}");
    assert!(!c.err("fix_contrast", json!({"prefer": "both"})).is_empty());
    assert!(!c.err("fix_contrast", json!({"root": f, "ids": [f]})).is_empty());
}

#[test]
fn export_assets_writes_files_inside_the_write_root_only() {
    let base = std::env::temp_dir().join(format!("figcraft-mcp-assets-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    for sub in ["out", "outside"] {
        std::fs::create_dir_all(base.join(sub)).unwrap();
    }
    let p = |rel: &str| base.join(rel).to_string_lossy().into_owned();
    let mut s = Server::headless().with_roots(Some(Roots::new(&[], &[p("out")]).unwrap()));
    let mut call = |tool: &str, args: Value| {
        let line = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": tool, "arguments": args}}).to_string();
        let r: Value = serde_json::from_str(&s.handle_line(&line).unwrap()).unwrap();
        r["result"].clone()
    };
    assert_eq!(call("create_node", json!({"type": "frame", "name": "Logo", "width": 40, "height": 20}))["isError"], false);
    let r = call("export_assets", json!({"dir": p("out")}));
    assert_eq!(r["isError"], true, "no export settings: {r}");
    assert_eq!(call("run_command", json!({"command": "export.add"}))["isError"], false);
    let r = call("export_assets", json!({"dir": p("outside")}));
    assert_eq!(r["isError"], true);
    assert!(r["content"][0]["text"].as_str().unwrap().contains("automation path rejected"), "{r}");
    assert_eq!(std::fs::read_dir(base.join("outside")).unwrap().count(), 0);
    let r = call("export_assets", json!({"dir": p("out")}));
    assert_eq!(r["isError"], false, "{r}");
    assert!(base.join("out/Logo.png").is_file() && base.join("out/manifest.json").is_file());
    let again = call("export_assets", json!({"dir": p("out")}));
    assert_eq!(again["isError"], true, "needs overwrite");
    assert_eq!(call("export_assets", json!({"dir": p("out"), "overwrite": true}))["isError"], false);
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn both_tools_are_registered_and_documented() {
    let mut c = Client::new();
    let tools = c.rpc("tools/list", json!({}))["result"]["tools"].clone();
    for name in ["fix_contrast", "export_assets"] {
        assert!(TOOL_NAMES.contains(&name));
        let t = tools.as_array().unwrap().iter().find(|t| t["name"] == name).unwrap().clone();
        assert_eq!(t["annotations"]["readOnlyHint"], false);
        assert!(t["inputSchema"]["properties"]["document"].is_object());
        assert!(include_str!("../../../../docs/mcp.md").contains(&format!("| `{name} {{")));
    }
}
