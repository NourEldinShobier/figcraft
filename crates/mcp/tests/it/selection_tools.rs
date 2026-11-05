//! `get_selection`, the `where` filters of `select` and `set_properties`, and `diagnostics`.

use crate::common::{Client, approx};
use serde_json::{Value, json};

fn sorted(v: &Value) -> Vec<u64> {
    let mut ids: Vec<u64> = v.as_array().unwrap().iter().map(|i| i.as_u64().unwrap()).collect();
    ids.sort_unstable();
    ids
}

/// A frame "Cards" holding text "Label A", text "Label B" and a rectangle "Label shape".
fn cards(c: &mut Client) -> (Value, Value, Value, Value) {
    let frame = c.create(json!({"type": "frame", "name": "Cards", "width": 300, "height": 200}));
    let a = c.create(json!({"type": "text", "text": "A", "name": "Label A", "parent": frame}));
    let b = c.create(json!({"type": "text", "text": "B", "name": "Label B", "parent": frame}));
    let shape = c.create(json!({"type": "rectangle", "name": "Label shape", "parent": frame}));
    (frame, a, b, shape)
}

#[test]
fn get_selection_reports_the_selected_layers_and_their_parents() {
    let mut c = Client::new();
    let empty = c.ok("get_selection", json!({}));
    assert_eq!((empty["count"].clone(), empty["selection"].clone()), (json!(0), json!([])));
    assert!(empty["page"]["name"].is_string() && empty["page"]["id"].is_u64(), "{empty}");

    let frame = c.create(json!({"type": "frame", "name": "Card", "x": 10, "y": 20, "width": 200, "height": 100}));
    let rect = c.create(json!({"type": "rectangle", "name": "Box", "parent": frame, "x": 5, "y": 6, "width": 30, "height": 40}));
    let one = c.ok("get_selection", json!({}));
    assert_eq!(one["count"], 1, "creating a layer selects it: {one}");
    let r = &one["selection"][0];
    assert_eq!(
        (r["id"].clone(), r["name"].clone(), r["type"].clone(), r["parent"].clone()),
        (rect.clone(), json!("Box"), json!("RECTANGLE"), frame.clone())
    );
    assert!(approx(&r["x"], 5.0) && approx(&r["y"], 6.0) && approx(&r["width"], 30.0) && approx(&r["height"], 40.0), "{r}");
    assert_eq!(r.as_object().unwrap().len(), 8, "only the eight listed keys: {r}");

    c.ok("select", json!({"ids": [frame, rect]}));
    let two = c.ok("get_selection", json!({}));
    assert_eq!(two["count"], 2);
    let top = two["selection"].as_array().unwrap().iter().find(|n| n["id"] == frame).unwrap();
    assert_eq!(top["parent"], c.doc()["currentPage"], "a top-level layer's parent is the page");
    assert_eq!(two["page"]["id"], c.doc()["currentPage"]);

    c.ok("select", json!({"ids": []}));
    assert_eq!(c.ok("get_selection", json!({}))["count"], 0);
    assert!(c.err("get_selection", json!({"ids": [1]})).contains("unknown argument"));
}

#[test]
fn select_where_selects_every_match_and_leaves_the_selection_when_none_match() {
    let mut c = Client::new();
    let (frame, a, b, shape) = cards(&mut c);
    let r = c.ok("select", json!({"where": {"type": "TEXT", "nameGlob": "Label*"}}));
    assert_eq!(r["matched"], 2, "{r}");
    assert_eq!(sorted(&r["ids"]), sorted(&json!([a, b])));
    assert_eq!(sorted(&r["selection"]), sorted(&json!([a, b])));
    assert_eq!(c.ok("get_selection", json!({}))["count"], 2);

    // The rectangle matches by name only; `root` limits the search to the frame's subtree.
    let r = c.ok("select", json!({"where": {"root": frame, "name": "label shape"}}));
    assert_eq!((r["matched"].clone(), r["ids"].clone(), r["selection"].clone()), (json!(1), json!([shape]), json!([shape])));

    let r = c.ok("select", json!({"where": {"name": "no layer is called this"}}));
    assert_eq!((r["matched"].clone(), r["ids"].clone(), r["selection"].clone()), (json!(0), json!([]), json!([shape])));
    assert!(r["note"].as_str().unwrap().contains("unchanged"), "{r}");
}

#[test]
fn where_is_checked_before_anything_is_selected_or_changed() {
    let mut c = Client::new();
    let (_, a, b, _) = cards(&mut c);
    c.ok("select", json!({"ids": [a]}));
    for (tool, args, want) in [
        ("select", json!({"ids": [a], "where": {"name": "Label"}}), "not both"),
        ("select", json!({}), "required"),
        ("select", json!({"where": {}}), "besides `limit`"),
        ("select", json!({"where": {"limit": 5}}), "besides `limit`"),
        ("select", json!({"where": {"name": "Label", "limit": 1}}), "3 layers match"),
        ("select", json!({"where": {"bogus": 1}}), "`where.bogus`"),
        ("select", json!({"where": "Label"}), "must be an object"),
        ("select", json!({"where": {"type": "NOT_A_TYPE"}}), "unknown type"),
        ("set_properties", json!({"ids": [a], "where": {"name": "Label"}, "props": {"opacity": 0.5}}), "not both"),
        ("set_properties", json!({"where": {"name": "Label", "limit": 2}, "props": {"opacity": 0.5}}), "3 layers match"),
        ("set_properties", json!({"where": {"name": "no layer is called this"}, "props": {"opacity": 0.5}}), "no layer matches"),
    ] {
        let e = c.err(tool, args.clone());
        assert!(e.contains(want), "{tool} {args}: {e}");
    }
    assert_eq!(c.ok("get_selection", json!({}))["selection"][0]["id"], a, "the selection survived every refusal");
    assert!(c.node(&a).get("opacity").is_none() && c.node(&b).get("opacity").is_none(), "nothing was changed");
}

#[test]
fn set_properties_where_changes_every_match_as_one_undo_step() {
    let mut c = Client::new();
    let (_, a, b, shape) = cards(&mut c);
    let r = c.ok("set_properties", json!({"where": {"type": "TEXT", "nameGlob": "Label*"}, "props": {"opacity": 0.5}}));
    assert_eq!((r["ok"].clone(), r["matched"].clone()), (json!(true), json!(2)), "{r}");
    assert_eq!(sorted(&r["ids"]), sorted(&json!([a, b])));
    assert_eq!((c.node(&a)["opacity"].clone(), c.node(&b)["opacity"].clone()), (json!(0.5), json!(0.5)));
    assert!(c.node(&shape).get("opacity").is_none(), "the rectangle does not match type TEXT");

    c.ok("undo", json!({}));
    assert!(c.node(&a).get("opacity").is_none() && c.node(&b).get("opacity").is_none(), "one undo reverts both layers");

    // A single match reports the updated layer unless `quiet` is set, like `ids`.
    let r = c.ok("set_properties", json!({"where": {"text": "b"}, "props": {"name": "Renamed"}}));
    assert_eq!((r["matched"].clone(), r["ids"].clone(), r["nodes"][0]["name"].clone()), (json!(1), json!([b]), json!("Renamed")), "{r}");
}

#[test]
fn diagnostics_describes_the_headless_backend_and_its_one_document() {
    let mut c = Client::new();
    let d = c.ok("diagnostics", json!({}));
    assert_eq!(d["attached"], false);
    assert!(d["backend"].as_str().unwrap().starts_with("headless"), "{d}");
    assert_eq!(d["mcpServer"]["pid"], std::process::id());
    let docs = d["documents"].as_array().unwrap();
    assert_eq!(docs.len(), 1, "{d}");
    let doc = &docs[0];
    assert_eq!((doc["index"].clone(), doc["active"].clone(), doc["path"].clone()), (json!(0), json!(true), json!(null)));
    assert_eq!((doc["dirty"].clone(), doc["changedOnDisk"].clone(), doc["lastSaved"].clone()), (json!(false), json!(false), json!(null)));
    assert!(doc["name"].is_string() && doc["id"].is_u64(), "{doc}");

    c.create(json!({"type": "rectangle"}));
    assert_eq!(c.ok("diagnostics", json!({}))["documents"][0]["dirty"], true);
    c.run("file.new", json!({"name": "Second"}));
    let docs = c.ok("diagnostics", json!({}))["documents"].clone();
    assert_eq!(docs.as_array().unwrap().len(), 2);
    assert_eq!((docs[0]["active"].clone(), docs[1]["active"].clone(), docs[1]["name"].clone()), (json!(false), json!(true), json!("Second")));
    assert!(c.err("diagnostics", json!({"x": 1})).contains("unknown argument"));
}

#[test]
fn where_lists_exactly_the_node_find_filters_and_the_docs_name_the_new_tools() {
    let mut c = Client::new();
    let tools = c.rpc("tools/list", json!({}));
    let tools = tools["result"]["tools"].as_array().unwrap();
    let find = figcraft_engine::all_commands().iter().find(|s| s.id == "node.find").unwrap().params;
    for name in ["select", "set_properties"] {
        let tool = tools.iter().find(|t| t["name"] == name).unwrap();
        let mut keys: Vec<&str> = tool["inputSchema"]["properties"]["where"]["properties"].as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "component",
                "edgeGap",
                "fill",
                "height",
                "heightNot",
                "key",
                "keyGlob",
                "limit",
                "name",
                "nameGlob",
                "nameRegex",
                "outsideParent",
                "page",
                "parent",
                "root",
                "stroke",
                "strokeWeight",
                "text",
                "type",
                "variant",
                "width",
                "widthNot"
            ],
            "{name}"
        );
        for k in keys {
            assert!(find.contains(&format!("{k}?")), "node.find does not document `{k}`: {find}");
        }
    }
    let docs = include_str!("../../../../docs/mcp.md");
    for row in
        ["| `get_selection {}` |", "| `diagnostics {}` |", r"| `select {ids \| where}` |", r"| `set_properties {ids \| where, props, quiet?}` |"]
    {
        assert!(docs.contains(row), "docs/mcp.md has no row {row}");
    }
}
