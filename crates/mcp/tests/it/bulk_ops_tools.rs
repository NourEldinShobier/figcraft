//! The bulk operation tools: `run_command` runs with per-layer results and a cursor, `batch` takes a
//! window, `fill_instances`, `swap_component` and `replace_color`.

use crate::common::Client;
use figcraft_mcp::TOOL_NAMES;
use serde_json::{Value, json};

fn rects(c: &mut Client, n: i64) -> Vec<Value> {
    (0..n).map(|i| c.create(json!({"type": "rectangle", "x": i * 50, "y": 0, "width": 40, "height": 40}))).collect()
}

fn opacity(c: &mut Client, id: &Value) -> f64 {
    c.ok("get_node", json!({"id": id, "depth": 0}))["opacity"].as_f64().unwrap_or(1.0)
}

/// A component "Card" with a `Title` text bound to the text property `label`.
fn card(c: &mut Client) -> Value {
    let f = c.create(json!({"type": "frame", "x": 0, "y": 0, "width": 200, "height": 60, "name": "Card"}));
    let title = c.create(json!({"type": "text", "text": "Title", "x": 10, "y": 10, "parent": f, "name": "Title"}));
    let comp = c.run("component.create", json!({"ids": [f]}))["id"].clone();
    c.run("component.addProperty", json!({"id": comp, "name": "label", "type": "text", "layer": title}));
    comp
}

#[test]
fn run_command_with_continue_on_error_answers_per_layer_and_pages_with_a_cursor() {
    let mut c = Client::new();
    let r = rects(&mut c, 5);
    let first =
        c.ok("run_command", json!({"command": "node.set", "params": {"ids": r, "props": {"opacity": 0.5}}, "continueOnError": true, "limit": 2}));
    assert_eq!((first["processed"].clone(), first["remaining"].clone(), first["failed"].clone()), (json!(2), json!(3), json!(0)), "{first}");
    assert_eq!(first["changed"]["count"], 2, "the changed summary carries over: {first}");
    let cursor = first["nextCursor"].clone();
    assert!(cursor.is_string(), "{first}");
    let second = c.ok("run_command", json!({"command": "node.set", "params": {"props": {"opacity": 0.5}}, "cursor": cursor, "limit": 10}));
    assert_eq!((second["processed"].clone(), second["done"].clone()), (json!(3), json!(true)), "{second}");
    for id in &r {
        assert_eq!(opacity(&mut c, id), 0.5);
    }
    // Failures are listed per layer and do not stop the others.
    let t = c.create(json!({"type": "text", "text": "Hi", "x": 0, "y": 200}));
    let mixed = c.ok("run_command", json!({"command": "text.set", "params": {"ids": [r[0], t], "characters": "Changed"}, "continueOnError": true}));
    assert_eq!((mixed["ok"].clone(), mixed["failed"].clone()), (json!(1), json!(1)), "{mixed}");
    assert_eq!(mixed["items"][0]["ok"], false, "{mixed}");
    // The layers can come from where, and the generic dryRun belongs to the whole call.
    let dry = c.ok(
        "run_command",
        json!({"command": "node.set", "params": {"where": {"type": "RECTANGLE"}, "props": {"opacity": 0.1}, "dryRun": true}, "continueOnError": true}),
    );
    assert_eq!(dry["dryRun"], true, "{dry}");
    assert_eq!(dry["count"], 5, "{dry}");
    assert_eq!(opacity(&mut c, &r[0]), 0.5);
}

#[test]
fn run_command_each_is_guarded_by_max_affected() {
    let mut c = Client::new();
    let r = rects(&mut c, 4);
    let e = c.err(
        "run_command",
        json!({"command": "node.set", "params": {"ids": r, "props": {"opacity": 0.5}}, "continueOnError": true, "maxAffected": 2}),
    );
    assert!(e.contains("maxAffected 2"), "{e}");
    assert_eq!(opacity(&mut c, &r[0]), 1.0);
    for junk in [json!({"limit": 0}), json!({"limit": "x"}), json!({"cursor": 4}), json!({"cursor": "each-99"}), json!({"continueOnError": "maybe"})]
    {
        let mut args = json!({"command": "node.set", "params": {"ids": r, "props": {"opacity": 0.5}}});
        args.as_object_mut().unwrap().extend(junk.as_object().unwrap().clone());
        assert!(!c.err("run_command", args).is_empty(), "{junk}");
    }
    assert_eq!(opacity(&mut c, &r[0]), 1.0);
}

#[test]
fn batch_takes_a_window_and_continue_on_error() {
    let mut c = Client::new();
    let r = rects(&mut c, 4);
    let steps: Vec<Value> = r.iter().map(|i| json!({"command": "node.set", "params": {"ids": [i], "props": {"opacity": 0.5}}})).collect();
    let a = c.ok("batch", json!({"commands": steps, "limit": 3, "continueOnError": true}));
    assert_eq!((a["processed"].clone(), a["next"].clone(), a["done"].clone()), (json!(3), json!(3), json!(false)), "{a}");
    let q = c.ok("batch", json!({"commands": steps, "from": 3, "quiet": true}));
    assert_eq!((q["ok"].clone(), q["count"].clone(), q["done"].clone()), (json!(true), json!(1), json!(true)), "{q}");
    for id in &r {
        assert_eq!(opacity(&mut c, id), 0.5);
    }
    assert!(c.err("batch", json!({"commands": steps, "from": 99})).contains("past the last"));
    assert!(c.err("batch", json!({"commands": steps, "limit": 0})).contains("`limit`"));
}

#[test]
fn fill_instances_makes_and_updates_instances_from_rows_and_csv() {
    let mut c = Client::new();
    let comp = card(&mut c);
    let rows: Vec<Value> = (0..30).map(|i| json!({"key": format!("Card {i}"), "label": format!("Item {i}"), "stray": 1})).collect();
    let r = c.ok("fill_instances", json!({"component": comp, "rows": rows, "keyColumn": "key", "maxAffected": 500}));
    assert_eq!((r["created"].clone(), r["failed"].clone(), r["unmatchedColumns"].clone()), (json!(30), json!([]), json!(["stray"])), "{r}");
    assert!(r["changed"]["count"].as_u64().unwrap() >= 30, "{r}");
    let found = c.ok("find_nodes", json!({"name": "Card 29"}));
    assert_eq!(found["total"], 1, "{found}");
    // Update by the key column from CSV, with a row that matches nothing.
    let csv = "key,label\nCard 3,Three\nCard 29,\"Twenty, nine\"\nNope,x\n";
    let u = c.ok("fill_instances", json!({"component": comp, "mode": "update", "keyColumn": "key", "csv": csv}));
    assert_eq!((u["updated"].clone(), u["unmatchedRowCount"].clone()), (json!(2), json!(1)), "{u}");
    let titles = c.ok("find_nodes", json!({"name": "Title", "text": "Twenty, nine"}));
    assert_eq!(titles["total"], 1, "{titles}");
    // A dry run changes nothing.
    let dry = c.ok("fill_instances", json!({"component": comp, "rows": [{"label": "z"}], "dryRun": true}));
    assert_eq!(dry["dryRun"], true, "{dry}");
    let after = c.ok("find_nodes", json!({"name": "Card", "limit": 100}));
    assert_eq!(after["total"], 31, "the main component and 30 instances: {after}");
}

#[test]
fn fill_instances_refuses_junk() {
    let mut c = Client::new();
    let comp = card(&mut c);
    for junk in [
        json!({}),
        json!({"component": comp}),
        json!({"component": comp, "rows": "x"}),
        json!({"component": comp, "rows": [{"label": "x"}], "csv": "a\n1"}),
        json!({"component": comp, "rows": [{"label": "x"}], "mode": "merge"}),
        json!({"component": comp, "rows": [{"label": "x"}], "mode": "update"}),
        json!({"component": comp, "rows": [{"label": "x"}], "map": 3}),
        json!({"component": comp, "rows": [{"label": "x"}], "gap": "wide"}),
        json!({"component": comp, "rows": [{"label": "x"}], "bogus": 1}),
        json!({"component": comp, "rows": [{"label": "x"}], "maxAffected": 0}),
    ] {
        assert!(!c.err("fill_instances", junk.clone()).is_empty(), "{junk}");
    }
}

#[test]
fn swap_component_swaps_every_instance_and_reports_what_did_not_carry_over() {
    let mut c = Client::new();
    let old = card(&mut c);
    let f = c.create(json!({"type": "frame", "x": 400, "y": 0, "width": 200, "height": 60, "name": "Newer"}));
    c.create(json!({"type": "text", "text": "Title", "x": 10, "y": 10, "parent": f, "name": "Title"}));
    let new = c.run("component.create", json!({"ids": [f]}))["id"].clone();
    c.ok("fill_instances", json!({"component": old, "rows": [{"label": "a"}, {"label": "b"}, {"label": "c"}]}));
    let dry = c.ok("swap_component", json!({"from": old, "to": new, "dryRun": true}));
    assert_eq!(dry["dryRun"], true, "{dry}");
    let r = c.ok("swap_component", json!({"from": old, "to": new}));
    assert_eq!((r["swapped"].clone(), r["failed"].clone()), (json!(3), json!([])), "{r}");
    assert_eq!(r["unmatched"]["props"].as_array().unwrap().len(), 1, "the label property has no counterpart: {r}");
    for junk in [
        json!({}),
        json!({"from": old}),
        json!({"from": old, "to": old}),
        json!({"from": old, "to": new, "mapOverrides": [1]}),
        json!({"from": old, "to": new, "ids": "x"}),
    ] {
        assert!(!c.err("swap_component", junk.clone()).is_empty(), "{junk}");
    }
}

#[test]
fn replace_color_replaces_in_fills_strokes_and_shadows_with_a_dry_run() {
    let mut c = Client::new();
    let a = c.create(json!({"type": "rectangle", "x": 0, "y": 0, "width": 10, "height": 10, "fill": "#FF0000", "stroke": "#FF0000"}));
    let b = c.create(json!({"type": "rectangle", "x": 20, "y": 0, "width": 10, "height": 10, "fill": "#0000FF"}));
    let dry = c.ok("replace_color", json!({"from": "#FF0000", "to": "#00FF00", "dryRun": true}));
    assert_eq!(dry["dryRun"], true, "{dry}");
    assert_eq!(dry["wouldChange"], json!([a]), "{dry}");
    let r = c.ok("replace_color", json!({"from": "#FF0000", "to": "#00FF00", "scope": "file"}));
    assert_eq!((r["paints"].clone(), r["changed"].clone()), (json!(2), json!([a])), "{r}");
    assert_eq!(r["affected"]["count"], 1, "the changed summary of the call: {r}");
    let n = c.node(&a);
    assert_eq!(n["fills"][0]["color"].as_str().map(str::to_uppercase), Some("#00FF00".into()), "{n}");
    assert_eq!(c.node(&b)["fills"][0]["color"].as_str().map(str::to_uppercase), Some("#0000FF".into()));
    for junk in [
        json!({}),
        json!({"from": "#FF0000"}),
        json!({"from": "#FF0000", "to": "#00FF00", "variable": "x"}),
        json!({"from": "#FF0000", "to": "#00FF00", "scope": "nowhere"}),
        json!({"from": 4, "to": "#00FF00"}),
    ] {
        assert!(!c.err("replace_color", junk.clone()).is_empty(), "{junk}");
    }
}

#[test]
fn the_bulk_operation_tools_are_registered_guarded_and_documented() {
    let mut c = Client::new();
    let tools = c.rpc("tools/list", json!({}))["result"]["tools"].clone();
    for name in ["fill_instances", "swap_component", "replace_color"] {
        assert!(TOOL_NAMES.contains(&name));
        let t = tools.as_array().unwrap().iter().find(|t| t["name"] == name).unwrap().clone();
        let props = &t["inputSchema"]["properties"];
        for k in ["document", "maxAffected", "as", "confirm", "ifRev", "documents"] {
            assert_eq!(props.get(k).is_some(), k != "documents", "{name} {k}");
        }
        assert!(include_str!("../../../../docs/mcp.md").contains(&format!("| `{name} {{")), "{name} needs a row in docs/mcp.md");
    }
    let run = tools.as_array().unwrap().iter().find(|t| t["name"] == "run_command").unwrap().clone();
    for k in ["continueOnError", "limit", "cursor"] {
        assert!(run["inputSchema"]["properties"].get(k).is_some(), "run_command {k}");
    }
    let batch = tools.as_array().unwrap().iter().find(|t| t["name"] == "batch").unwrap().clone();
    for k in ["continueOnError", "from", "limit"] {
        assert!(batch["inputSchema"]["properties"].get(k).is_some(), "batch {k}");
    }
}
