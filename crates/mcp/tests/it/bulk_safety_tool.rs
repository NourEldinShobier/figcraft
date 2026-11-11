//! Bulk-edit safety through the tools: `maxAffected` and `confirm`, the `changed` summary, the
//! scope fence and the owner locks.

use crate::common::Client;
use serde_json::{Value, json};

fn rects(c: &mut Client, n: i64) -> Vec<Value> {
    (0..n).map(|i| c.create(json!({"type": "rectangle", "x": i * 50, "y": 0, "width": 40, "height": 40}))).collect()
}

fn opacity(c: &mut Client, id: &Value) -> f64 {
    c.ok("get_node", json!({"id": id, "depth": 0}))["opacity"].as_f64().unwrap_or(1.0)
}

#[test]
fn a_call_over_max_affected_is_refused_until_confirmed() {
    let mut c = Client::new();
    let r = rects(&mut c, 5);
    let e = c.err("set_properties", json!({"ids": r, "props": {"opacity": 0.5}, "maxAffected": 3}));
    assert!(e.contains("5 layers") && e.contains("maxAffected 3") && e.contains("confirm: true") && e.contains("sample ids"), "{e}");
    assert_eq!(opacity(&mut c, &r[0]), 1.0, "nothing changed");
    let ok = c.ok("set_properties", json!({"ids": r, "props": {"opacity": 0.5}, "maxAffected": 3, "confirm": true}));
    assert_eq!(ok["changed"]["count"], 5, "{ok}");
    assert_eq!(ok["changed"]["props"], json!(["opacity"]));
    // run_command and batch are guarded the same way.
    let e = c.err("run_command", json!({"command": "node.set", "params": {"ids": r, "props": {"opacity": 0.2}}, "maxAffected": 2}));
    assert!(e.contains("node.set would change 5 layers"), "{e}");
    let steps: Vec<Value> = r.iter().map(|i| json!({"command": "node.set", "params": {"ids": [i], "props": {"opacity": 0.2}}})).collect();
    let e = c.err("batch", json!({"commands": steps, "maxAffected": 4}));
    assert!(e.contains("command.batch would change 5 layers"), "{e}");
    assert_eq!(opacity(&mut c, &r[4]), 0.5);
}

#[test]
fn the_default_limit_is_200_layers() {
    let mut c = Client::new();
    let code = "for i in 0..201 { cmd(\"shape.rectangle\", #{x: i * 3, y: 0, width: 2, height: 2}); }";
    let e = c.err("run_script", json!({"code": code}));
    assert!(e.contains("script.run would change 201 layers") && e.contains("maxAffected 200"), "{e}");
    let ok = c.ok("run_script", json!({"code": code, "confirm": true}));
    assert_eq!(ok["affected"]["count"], 201, "script.run has its own `changed`, so the summary is `affected`: {ok}");
    assert_eq!(ok["created"].as_array().map(Vec::len), Some(201));
}

#[test]
fn every_mutating_result_carries_a_changed_summary() {
    let mut c = Client::new();
    let r = rects(&mut c, 12);
    let ok = c.ok("set_properties", json!({"ids": r, "props": {"opacity": 0.5}}));
    assert_eq!(ok["changed"]["count"], 12, "{ok}");
    assert_eq!(ok["changed"]["ids"].as_array().map(Vec::len), Some(10));
    assert_eq!(ok["changed"]["more"], 2);
    // A write that changes nothing says so.
    let ok = c.ok("set_properties", json!({"ids": r[..2], "props": {"opacity": 0.5}}));
    assert_eq!((ok["changed"]["count"].clone(), ok["changed"]["noop"].clone()), (json!(0), json!(2)), "{ok}");
    let ok = c.ok("run_command", json!({"command": "node.set", "params": {"ids": r[..1], "props": {"opacity": 0.5}}}));
    assert_eq!(ok["changed"]["noop"], 1, "{ok}");
    let made = c.ok("create_node", json!({"type": "ellipse", "width": 10, "height": 10}));
    assert_eq!(made["changed"]["created"], 1, "{made}");
    assert!(made["id"].is_number(), "{made}");
    // A batch answers with an array: the summary is a second text block.
    let steps = vec![json!({"command": "node.set", "params": {"ids": r[..3], "props": {"opacity": 0.25}}})];
    let call = c.call("batch", json!({"commands": steps}));
    let blocks = call["content"].as_array().unwrap();
    assert_eq!(blocks.len(), 2, "{call}");
    assert!(serde_json::from_str::<Value>(blocks[0]["text"].as_str().unwrap()).unwrap().is_array());
    assert_eq!(serde_json::from_str::<Value>(blocks[1]["text"].as_str().unwrap()).unwrap()["changed"]["count"], 3);
    // Reads and previews have none.
    let preview = c.ok("set_properties", json!({"ids": r[..3], "props": {"opacity": 0.9}, "dryRun": true}));
    assert!(preview.get("changed").is_none(), "{preview}");
    let found = c.ok("find_nodes", json!({"type": "RECTANGLE"}));
    assert!(found.get("changed").is_none(), "{found}");
}

#[test]
fn propagate_change_is_guarded_too() {
    let mut c = Client::new();
    let mut frames = Vec::new();
    for i in 0..3 {
        let f = c.create(json!({"type": "frame", "name": format!("Header {i}"), "x": i * 400, "y": 0, "width": 300, "height": 60}));
        c.create(json!({"type": "rectangle", "name": "Logo", "parent": f, "x": 10, "y": 10, "width": 40, "height": 40}));
        frames.push(f);
    }
    c.ok("set_properties", json!({"ids": [frames[0]], "props": {"fills": [{"type": "solid", "color": "#00AA00"}]}}));
    let e = c.err("propagate_change", json!({"from": frames[0], "confirm": true, "maxAffected": 1}));
    assert!(e.contains("family.propagate would change") && e.contains("larger maxAffected"), "{e}");
    let done = c.ok("propagate_change", json!({"from": frames[0], "confirm": true}));
    assert!(done["changed"].is_array() && done["affected"]["count"].as_u64().unwrap() >= 2, "{done}");
}

#[test]
fn the_scope_fence_is_set_through_run_command() {
    let mut c = Client::new();
    let r = rects(&mut c, 2);
    let set = c.ok("run_command", json!({"command": "scope.set", "params": {"ids": [r[0]]}}));
    assert_eq!(set["ids"], json!([r[0]]));
    c.ok("set_properties", json!({"ids": [r[0]], "props": {"opacity": 0.5}}));
    let e = c.err("set_properties", json!({"ids": [r[1]], "props": {"opacity": 0.5}}));
    assert!(e.contains("outside the scope fence") && e.contains("scope.clear"), "{e}");
    assert_eq!(c.ok("run_command", json!({"command": "scope.get", "params": {}}))["active"], true);
    c.ok("run_command", json!({"command": "scope.clear", "params": {}}));
    c.ok("set_properties", json!({"ids": [r[1]], "props": {"opacity": 0.5}}));
}

#[test]
fn locks_hold_for_other_owner_labels() {
    let mut c = Client::new();
    let r = rects(&mut c, 2);
    c.ok("run_command", json!({"command": "lock.set", "params": {"ids": [r[0]], "owner": "alice"}, "as": "alice"}));
    let list = c.ok("run_command", json!({"command": "lock.list", "params": {}}));
    assert_eq!(list["owners"]["alice"], 1, "{list}");
    for args in [json!({"ids": [r[0]], "props": {"opacity": 0.5}}), json!({"ids": [r[0]], "props": {"opacity": 0.5}, "as": "bob"})] {
        let e = c.err("set_properties", args);
        assert!(e.contains("locked by \"alice\""), "{e}");
    }
    c.ok("set_properties", json!({"ids": [r[0]], "props": {"opacity": 0.5}, "as": "alice"}));
    c.ok("set_properties", json!({"ids": [r[1]], "props": {"opacity": 0.5}, "as": "bob"}));
    let released = c.ok("run_command", json!({"command": "lock.release", "params": {"ids": [r[0]]}, "as": "alice"}));
    assert_eq!(released["released"], json!([r[0]]));
    c.ok("set_properties", json!({"ids": [r[0]], "props": {"opacity": 0.9}, "as": "bob"}));
}

#[test]
fn bad_safety_arguments_are_errors() {
    let mut c = Client::new();
    let r = rects(&mut c, 1);
    let props = json!({"opacity": 0.5});
    for extra in [
        json!({"maxAffected": 0}),
        json!({"maxAffected": -1}),
        json!({"maxAffected": "x"}),
        json!({"maxAffected": 1.5}),
        json!({"confirm": "yes"}),
        json!({"as": ""}),
        json!({"as": 3}),
    ] {
        let mut args = json!({"ids": r, "props": props});
        args.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        let e = c.err("set_properties", args);
        assert!(!e.is_empty(), "{extra}");
        assert_eq!(opacity(&mut c, &r[0]), 1.0, "{extra}");
    }
    assert!(c.err("propagate_change", json!({"from": 1, "confirm": true, "maxAffected": 0})).contains("maxAffected"));
}

#[test]
fn the_tools_list_the_safety_arguments() {
    let mut c = Client::new();
    let tools = c.rpc("tools/list", json!({}))["result"]["tools"].clone();
    for name in
        ["run_command", "batch", "create_node", "set_properties", "propagate_change", "run_script", "apply_text", "import_html", "generate_diagram"]
    {
        let props = tools.as_array().unwrap().iter().find(|t| t["name"] == name).unwrap()["inputSchema"]["properties"].clone();
        assert!(props.get("maxAffected").is_some() && props.get("as").is_some(), "{name}");
        assert_eq!(
            props.get("confirm").is_some(),
            name != "propagate_change" || props["confirm"]["description"].as_str().unwrap().contains("applies"),
            "{name}"
        );
    }
}
