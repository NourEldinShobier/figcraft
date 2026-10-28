//! `quiet` results of `set_properties` and `batch`.

use crate::common::Client;
use serde_json::json;

#[test]
fn set_properties_on_several_layers_returns_only_ids() {
    let mut c = Client::new();
    let a = c.create(json!({"type": "rectangle"}));
    let b = c.create(json!({"type": "rectangle"}));
    let r = c.ok("set_properties", json!({"ids": [a, b], "props": {"opacity": 0.5}}));
    assert_eq!(r, json!({"ok": true, "ids": [a, b]}));
    let r = c.ok("set_properties", json!({"ids": [a], "props": {"opacity": 0.4}}));
    assert!(r["nodes"].is_array(), "{r}");
    let r = c.ok("set_properties", json!({"ids": [a], "props": {"opacity": 0.3}, "quiet": true}));
    assert_eq!(r, json!({"ok": true, "ids": [a]}));
}

#[test]
fn create_node_takes_style_keys_beside_type() {
    let mut c = Client::new();
    let id = c.create(json!({"type": "rectangle", "fill": "#FF0000", "cornerRadius": 6}));
    let n = c.node(&id);
    assert!(n["fills"].to_string().to_lowercase().contains("ff0000"), "{n}");
    assert_eq!(n["cornerRadius"].as_f64(), Some(6.0), "{n}");
}

#[test]
fn quiet_batch_lists_only_failures() {
    let mut c = Client::new();
    let a = c.create(json!({"type": "rectangle"}));
    let steps = json!([
        {"command": "node.set", "params": {"ids": [a], "props": {"opacity": 0.5}}},
        {"command": "node.set", "params": {"ids": [a]}},
        {"command": "node.set", "params": {"ids": [999999], "props": {"opacity": 0.5}}},
    ]);
    let r = c.ok("batch", json!({"commands": steps, "quiet": true, "onError": "continue"}));
    assert_eq!(r["ok"], false);
    assert_eq!(r["count"], 3);
    assert_eq!(r["failures"].as_array().map(Vec::len), Some(1));
    assert_eq!(r["failures"][0]["step"], 2);
    let r = c.ok("batch", json!({"commands": [steps[0].clone(), steps[1].clone()], "quiet": true}));
    assert_eq!(r, json!({"ok": true, "count": 2}));
}
