//! `run_script`: loops and conditions in one call, one undo step, rolled back whole on an error.

use crate::common::Client;
use figcraft_mcp::TOOL_NAMES;
use serde_json::json;

fn layers(c: &mut Client) -> usize {
    c.ok("find_nodes", json!({"limit": 10000}))["total"].as_u64().unwrap() as usize
}

#[test]
fn a_script_creates_five_frames_in_a_loop_as_one_undo_step() {
    let mut c = Client::new();
    let r = c.ok(
        "run_script",
        json!({"code": r#"for i in 0..5 { cmd("node.create", #{type: "frame", name: "Card " + i, x: i * 220, y: 0, width: 200, height: 120}); }"#}),
    );
    assert_eq!(r["created"].as_array().unwrap().len(), 5, "{r}");
    assert_eq!(r["commands"], 5, "{r}");
    assert_eq!(layers(&mut c), 5);
    c.ok("undo", json!({}));
    assert_eq!(layers(&mut c), 0, "one undo takes the whole script back");
}

#[test]
fn a_script_recolours_every_layer_with_a_name() {
    let mut c = Client::new();
    for (i, name) in ["Button", "Button", "Card"].iter().enumerate() {
        c.create(json!({"type": "rectangle", "name": name, "x": i * 100, "y": 0, "width": 80, "height": 40}));
    }
    let r = c.ok(
        "run_script",
        json!({"code": r##"let n = 0; for id in find(#{name: "Button"}) { cmd("node.set", #{ids: [id], props: #{fills: [#{type: "solid", color: "#2563EB"}]}}); n += 1; } n"##}),
    );
    assert_eq!((r["value"].clone(), r["changed"].clone()), (json!(2), json!(2)), "{r}");
    let blue = c.ok("find_nodes", json!({"fill": "#2563EB"}));
    assert_eq!(blue["total"], 2, "{blue}");
}

#[test]
fn a_throw_midway_changes_nothing_and_the_error_names_the_line() {
    let mut c = Client::new();
    let e =
        c.err("run_script", json!({"code": "cmd(\"node.create\", #{type: \"frame\", width: 10, height: 10});\nlet x = 1;\nthrow \"stop here\";"}));
    assert!(e.contains("line 3") && e.contains("stop here") && e.contains("nothing was changed"), "{e}");
    assert_eq!(layers(&mut c), 0);
    let e = c.err("run_script", json!({"code": "let = ;"}));
    assert!(e.contains("line 1"), "{e}");
}

#[test]
fn an_exhausted_budget_is_a_readable_error() {
    let mut c = Client::new();
    let e = c.err("run_script", json!({"code": "let i = 0; loop { i += 1; }", "maxOperations": 5000}));
    assert!(e.contains("more than 5000 operations") && e.contains("maxOperations"), "{e}");
}

#[test]
fn a_script_cannot_touch_files_and_junk_arguments_are_errors() {
    let mut c = Client::new();
    let path = std::env::temp_dir().join("figcraft-run-script-test.fig");
    let p = path.to_string_lossy().replace('\\', "/");
    for code in [
        format!("cmd(\"file.saveAs\", #{{path: \"{p}\"}})"),
        format!("cmd(\"image.place\", #{{path: \"{p}\"}})"),
        "cmd(\"script.run\", #{code: \"1\"})".into(),
    ] {
        let e = c.err("run_script", json!({"code": code}));
        assert!(e.contains("isn't available") || e.contains("can't read"), "{e}");
    }
    assert!(!path.exists());
    for junk in [
        json!({}),
        json!({"code": 5}),
        json!({"code": ""}),
        json!({"code": "1", "maxOperations": 0}),
        json!({"code": "1", "maxOperations": "x"}),
        json!({"code": "1", "extra": true}),
    ] {
        assert!(!c.err("run_script", junk.clone()).is_empty(), "{junk}");
    }
}

#[test]
fn dry_run_reports_and_leaves_the_file_alone() {
    let mut c = Client::new();
    let r = c.ok("run_script", json!({"code": "cmd(\"node.create\", #{type: \"frame\", width: 10, height: 10}); 3", "dryRun": true}));
    assert_eq!((r["dryRun"].clone(), r["created"].as_array().map(Vec::len), r["result"]["value"].clone()), (json!(true), Some(1), json!(3)), "{r}");
    assert_eq!(layers(&mut c), 0);
}

#[test]
fn run_script_is_registered_writing_and_documented() {
    let mut c = Client::new();
    assert!(TOOL_NAMES.contains(&"run_script"));
    let tools = c.rpc("tools/list", json!({}))["result"]["tools"].clone();
    let t = tools.as_array().unwrap().iter().find(|t| t["name"] == "run_script").unwrap().clone();
    assert_eq!(t["annotations"]["readOnlyHint"], false);
    assert_eq!(t["inputSchema"]["required"], json!(["code"]));
    assert!(t["inputSchema"]["properties"]["document"].is_object());
    let desc = t["description"].as_str().unwrap();
    assert!(desc.contains("for i in 0..5") && desc.contains("find(#{name:"), "{desc}");
    assert!(include_str!("../../../../docs/mcp.md").contains("| `run_script {"));
}
