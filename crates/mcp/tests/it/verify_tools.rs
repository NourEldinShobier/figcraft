//! `check_changes` and `visual_diff`: what a bulk edit introduced since a checkpoint.

use crate::common::Client;
use figcraft_mcp::TOOL_NAMES;
use serde_json::json;

fn setup(c: &mut Client) -> (serde_json::Value, serde_json::Value, serde_json::Value, String) {
    let f1 = c.create(json!({"type": "frame", "name": "A", "x": 0, "width": 200, "height": 100}));
    let f2 = c.create(json!({"type": "frame", "name": "B", "x": 300, "width": 200, "height": 100}));
    let r = c.create(json!({"type": "rectangle", "parent": f1, "x": 20, "y": 20, "width": 60, "height": 40}));
    let v = c.ok("checkpoint", json!({"name": "base"}));
    (f1, f2, r, v["id"].as_str().unwrap().to_string())
}

#[test]
fn check_changes_returns_only_what_the_edit_introduced() {
    let mut c = Client::new();
    let (_, _, r, v) = setup(&mut c);
    let quiet = c.ok("check_changes", json!({"since": v}));
    assert_eq!((quiet["ok"].clone(), quiet["new"].clone()), (json!(true), json!(0)), "{quiet}");
    c.ok("set_properties", json!({"ids": [r], "props": {"x": 190}}));
    let broke = c.ok("check_changes", json!({"since": v, "checks": ["overflow"]}));
    assert_eq!((broke["ok"].clone(), broke["new"].clone()), (json!(false), json!(1)), "{broke}");
    assert_eq!(broke["issues"][0]["id"], r, "{broke}");
}

#[test]
fn visual_diff_lists_the_changed_frame_and_writes_a_sheet() {
    let mut c = Client::new();
    let (f1, _, r, v) = setup(&mut c);
    let same = c.ok("visual_diff", json!({"since": v}));
    assert_eq!((same["compared"].clone(), same["changed"].clone()), (json!(2), json!([])), "{same}");
    c.ok("set_properties", json!({"ids": [r], "props": {"fills": [{"type": "solid", "color": "#CC3300"}]}}));
    let dir = std::env::temp_dir().join(format!("figcraft-mcp-visual-diff-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("sheet.png").to_string_lossy().to_string();
    let d = c.ok("visual_diff", json!({"since": v, "sheet": path}));
    assert_eq!(d["changed"][0]["id"], f1, "{d}");
    assert_eq!(d["changed"][0]["targeted"], json!(true), "{d}");
    assert_eq!(d["untargeted"], json!([]), "{d}");
    assert!(std::fs::read(&path).unwrap().starts_with(&[0x89, b'P', b'N', b'G']), "{d}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn verify_tools_reject_bad_arguments() {
    let mut c = Client::new();
    let (_, _, _, v) = setup(&mut c);
    for junk in [
        json!({}),
        json!({"since": "nope"}),
        json!({"since": 99}),
        json!({"since": v, "limit": -1}),
        json!({"since": v, "checks": ["x"]}),
        json!({"since": v, "root": "x"}),
    ] {
        assert!(!c.err("check_changes", junk.clone()).is_empty(), "{junk}");
    }
    for junk in [
        json!({}),
        json!({"since": "nope"}),
        json!({"since": v, "threshold": 500}),
        json!({"since": v, "frames": ["x"]}),
        json!({"since": v, "sheet": "a.txt"}),
        json!({"since": v, "scale": 0}),
    ] {
        assert!(!c.err("visual_diff", junk.clone()).is_empty(), "{junk}");
    }
}

#[test]
fn verify_tools_are_registered_read_only_and_documented() {
    let mut c = Client::new();
    let tools = c.rpc("tools/list", json!({}))["result"]["tools"].clone();
    for name in ["check_changes", "visual_diff"] {
        assert!(TOOL_NAMES.contains(&name));
        let t = tools.as_array().unwrap().iter().find(|t| t["name"] == name).unwrap().clone();
        assert_eq!(t["annotations"]["readOnlyHint"], true);
        assert!(t["inputSchema"]["properties"]["document"].is_object());
        assert!(include_str!("../../../../docs/mcp.md").contains(&format!("| `{name} {{")), "{name} needs a row in docs/mcp.md");
    }
}
