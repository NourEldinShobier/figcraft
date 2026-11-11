//! The target document, `documents` calls, `copy_between_documents` and the version tools, on a
//! headless session with several documents.

use crate::common::{Client, approx};
use figcraft_mcp::{INSTRUCTIONS, TOOL_NAMES};
use serde_json::{Value, json};

/// The open documents' ids, in tab order.
fn doc_ids(c: &mut Client) -> Vec<Value> {
    c.run("doc.list", json!({})).as_array().unwrap().iter().map(|d| d["id"].clone()).collect()
}

/// The id of the document in front.
fn active(c: &mut Client) -> Value {
    c.ok("diagnostics", json!({}))["documents"].as_array().unwrap().iter().find(|d| d["active"] == true).unwrap()["id"].clone()
}

/// Three documents: the first one, "B" and "C" (in front).
fn three(c: &mut Client) -> (Value, Value, Value) {
    c.run("file.new", json!({"name": "B"}));
    c.run("file.new", json!({"name": "C"}));
    let ids = doc_ids(c);
    assert_eq!(ids.len(), 3, "{ids:?}");
    (ids[0].clone(), ids[1].clone(), ids[2].clone())
}

/// `create_node` in the document `doc`, whichever is in front.
fn add(c: &mut Client, doc: &Value, mut args: Value) -> Value {
    args["document"] = doc.clone();
    c.create(args)
}

/// A frame "Card" in `doc`, with a text "Label" in it when `label`.
fn card(c: &mut Client, doc: &Value, label: bool) -> (Value, Option<Value>) {
    let frame = add(c, doc, json!({"type": "frame", "name": "Card", "width": 200, "height": 100}));
    let text = label.then(|| add(c, doc, json!({"type": "text", "text": "Hi", "name": "Label", "parent": frame})));
    (frame, text)
}

/// The top-level layers of the current page of `doc`.
fn kids(c: &mut Client, doc: &Value) -> Vec<Value> {
    let d = c.ok("inspect_document", json!({"depth": 1, "document": doc}));
    let current = d["currentPage"].clone();
    d["pages"].as_array().unwrap().iter().find(|p| p["id"] == current).unwrap()["children"].as_array().cloned().unwrap_or_default()
}

fn total(c: &mut Client, doc: &Value, name: &str) -> u64 {
    c.ok("find_nodes", json!({"name": name, "document": doc}))["total"].as_u64().unwrap()
}

#[test]
fn documents_all_runs_find_nodes_in_every_open_file() {
    let mut c = Client::new();
    let (a, b, d) = three(&mut c);
    card(&mut c, &a, false);
    card(&mut c, &b, false);
    add(&mut c, &d, json!({"type": "frame", "name": "Other"}));

    let r = c.ok("find_nodes", json!({"name": "Card", "documents": "all"}));
    let rows = r["documents"].as_array().unwrap();
    assert_eq!(rows.iter().map(|x| x["document"].clone()).collect::<Vec<_>>(), [a.clone(), b.clone(), d.clone()]);
    assert_eq!(rows.iter().map(|x| x["ok"].clone()).collect::<Vec<_>>(), vec![json!(true); 3]);
    assert_eq!(rows.iter().map(|x| x["result"]["total"].clone()).collect::<Vec<_>>(), [json!(1), json!(1), json!(0)], "{r}");
    assert_eq!(rows[1]["name"], "B");
    assert_eq!((r["total"].clone(), r["totalMatched"].clone()), (json!(2), json!(2)));
    assert_eq!(active(&mut c), d, "the tab in front stays in front");

    // A list names the files; one that is not open fails alone.
    let r = c.ok("find_nodes", json!({"name": "Card", "documents": [b.clone(), 999_999]}));
    let rows = r["documents"].as_array().unwrap();
    assert_eq!((rows[0]["ok"].clone(), rows[0]["result"]["total"].clone()), (json!(true), json!(1)));
    assert_eq!(rows[1]["ok"], false, "{r}");
    assert!(rows[1]["error"].is_string());
    assert_eq!(r["total"], 1);

    // The call is an error only when it fails everywhere.
    assert!(!c.err("find_nodes", json!({"name": "Card", "documents": [999_999]})).is_empty());
}

#[test]
fn documents_is_checked_before_anything_runs() {
    let mut c = Client::new();
    let (a, _, _) = three(&mut c);
    assert!(c.err("find_nodes", json!({"name": "x", "documents": "all", "document": a})).contains("not both"));
    assert!(c.err("find_nodes", json!({"name": "x", "documents": "some"})).contains("must be"));
    assert!(c.err("find_nodes", json!({"name": "x", "documents": [{}]})).contains("must be"));
    assert!(c.err("find_nodes", json!({"name": "x", "documents": 3})).contains("must be"));
    assert!(c.err("find_nodes", json!({"name": "x", "documents": []})).contains("no open document"));
    let many: Vec<u64> = (0..101).collect();
    assert!(c.err("find_nodes", json!({"name": "x", "documents": many})).contains("at most 100"));
    // Only the tools that list it take `documents`.
    assert!(c.err("get_selection", json!({"documents": "all"})).contains("unknown argument"));
}

#[test]
fn set_properties_with_where_and_documents_changes_every_file_that_has_the_element() {
    let mut c = Client::new();
    let (a, b, d) = three(&mut c);
    let (_, label_a) = card(&mut c, &a, true);
    let (_, label_b) = card(&mut c, &b, true);
    card(&mut c, &d, false);
    let change = |dry: bool| json!({"where": {"name": "Label"}, "props": {"name": "Caption"}, "documents": "all", "dryRun": dry});

    let r = c.ok("set_properties", change(true));
    assert_eq!(r["totalMatched"], 2, "{r}");
    assert_eq!(total(&mut c, &a, "Label") + total(&mut c, &b, "Label"), 2, "a dry run changes nothing");

    let r = c.ok("set_properties", change(false));
    let rows = r["documents"].as_array().unwrap();
    assert_eq!(rows.iter().map(|x| x["ok"].clone()).collect::<Vec<_>>(), vec![json!(true); 3], "{r}");
    assert_eq!(rows.iter().map(|x| x["result"]["matched"].clone()).collect::<Vec<_>>(), [json!(1), json!(1), json!(0)], "{r}");
    assert_eq!(r["totalMatched"], 2);
    for (doc, label) in [(&a, &label_a), (&b, &label_b)] {
        let n = c.ok("get_node", json!({"id": label.clone().unwrap(), "document": doc, "depth": 0}));
        assert_eq!(n["name"], "Caption");
        assert_eq!(total(&mut c, doc, "Label"), 0);
    }
    assert_eq!(total(&mut c, &d, "Caption"), 0);
    assert_eq!(active(&mut c), d);
}

#[test]
fn get_nodes_and_run_command_take_documents() {
    let mut c = Client::new();
    let (a, b, d) = three(&mut c);
    let (frame_a, _) = card(&mut c, &a, false);
    card(&mut c, &b, false);

    let r = c.ok("get_nodes", json!({"where": {"name": "Card"}, "fields": ["name"], "documents": [a.clone(), b.clone(), d.clone()]}));
    let rows = r["documents"].as_array().unwrap();
    assert_eq!(rows.iter().map(|x| x["result"]["count"].clone()).collect::<Vec<_>>(), [json!(1), json!(1), json!(0)], "{r}");
    assert_eq!(rows[0]["result"]["nodes"][0]["id"], frame_a);

    let r = c.ok("run_command", json!({"command": "node.find", "params": {"name": "Card"}, "documents": "all"}));
    let rows = r["documents"].as_array().unwrap();
    assert_eq!(rows.iter().map(|x| x["result"]["total"].clone()).collect::<Vec<_>>(), [json!(1), json!(1), json!(0)], "{r}");
}

#[test]
fn copy_between_documents_pastes_into_the_other_file_and_touches_neither_tab() {
    let mut c = Client::new();
    let (a, b, d) = three(&mut c);
    let (frame, _) = card(&mut c, &a, true);
    let host = add(&mut c, &b, json!({"type": "frame", "name": "Host", "x": 0, "y": 0, "width": 400, "height": 400}));
    assert_eq!((kids(&mut c, &a).len(), kids(&mut c, &b).len()), (1, 1));

    let r = c.ok("copy_between_documents", json!({"from": a, "to": b, "ids": [frame]}));
    assert_eq!((r["copied"].clone(), r["from"].clone(), r["to"].clone()), (json!(1), a.clone(), b.clone()), "{r}");
    let new = r["newIds"][0].clone();
    assert_eq!(r["newIds"].as_array().unwrap().len(), 1);
    let in_b = kids(&mut c, &b);
    assert!(in_b.iter().any(|k| k["id"] == new && k["name"] == "Card"), "the copy is in B: {in_b:?}");
    assert_eq!(kids(&mut c, &a).len(), 1, "the original stays");
    assert_eq!(kids(&mut c, &a)[0]["id"], frame);
    assert_eq!(total(&mut c, &a, "Label"), 1);
    assert_eq!(active(&mut c), d, "neither tab was switched");

    // By `where` (run in `from`), into a frame of `to` at page coordinates.
    let r = c.ok("copy_between_documents", json!({"from": a, "to": b, "where": {"name": "Card"}, "parent": host, "x": 50, "y": 60}));
    let moved = r["newIds"][0].clone();
    let n = c.ok("get_node", json!({"id": moved, "document": b, "depth": 0}));
    let h = c.ok("get_node", json!({"id": host, "document": b, "depth": 0}));
    assert_eq!(h["childCount"], 1, "the copy went into Host: {h}");
    assert!(approx(&n["x"], 50.0) && approx(&n["y"], 60.0), "{n}");
    assert_eq!(active(&mut c), d);
}

#[test]
fn copy_between_documents_checks_its_arguments() {
    let mut c = Client::new();
    let (a, b, _) = three(&mut c);
    let (frame, _) = card(&mut c, &a, false);
    for (args, want) in [
        (json!({"from": a, "to": b}), "exactly one"),
        (json!({"from": a, "to": b, "ids": [frame], "where": {"name": "Card"}}), "exactly one"),
        (json!({"from": a, "to": b, "ids": [frame], "x": 5}), "together"),
        (json!({"to": b, "ids": [frame]}), "`from` is required"),
        (json!({"from": a, "ids": [frame]}), "`to` is required"),
        (json!({"from": a, "to": b, "ids": []}), "nothing to copy"),
        (json!({"from": a, "to": b, "where": {"name": "no layer has this name"}}), "nothing to copy"),
    ] {
        let e = c.err("copy_between_documents", args.clone());
        assert!(e.contains(want), "{args}: {e}");
    }
    assert!(!c.err("copy_between_documents", json!({"from": 999_999, "to": b, "ids": [frame]})).is_empty());
    assert_eq!(kids(&mut c, &b).len(), 0, "nothing was pasted");
}

#[test]
fn the_target_document_decides_where_calls_without_document_act() {
    let mut c = Client::new();
    let (a, b, d) = three(&mut c);
    let diag = c.ok("diagnostics", json!({}));
    assert_eq!(diag["target"], Value::Null);
    assert!(diag["autoSave"].is_boolean(), "diagnostics reports the auto-save preference: {diag}");

    let r = c.ok("use_document", json!({"document": a}));
    assert_eq!(r["target"], a);
    assert_eq!(r["documents"].as_array().unwrap().len(), 3);
    assert_eq!(c.ok("diagnostics", json!({}))["target"], a);
    c.create(json!({"type": "rectangle", "name": "InA"}));
    assert_eq!((total(&mut c, &a, "InA"), total(&mut c, &b, "InA"), total(&mut c, &d, "InA")), (1, 0, 0));
    assert_eq!(active(&mut c), d, "the tab in front did not change");
    // Reads follow the target too.
    assert_eq!(c.ok("find_nodes", json!({"name": "InA"}))["total"], 1);

    // `document` wins over the target.
    c.ok("create_node", json!({"type": "rectangle", "name": "InB", "document": b}));
    assert_eq!((total(&mut c, &a, "InB"), total(&mut c, &b, "InB")), (0, 1));

    // Clearing goes back to the tab in front; a name or path also names a file.
    assert_eq!(c.ok("use_document", json!({"document": null}))["target"], Value::Null);
    c.create(json!({"type": "rectangle", "name": "InC"}));
    assert_eq!((total(&mut c, &a, "InC"), total(&mut c, &d, "InC")), (0, 1));
    assert_eq!(c.ok("use_document", json!({"document": "B"}))["target"], b);
    assert_eq!(c.ok("find_nodes", json!({"name": "InB"}))["total"], 1);

    assert!(c.err("use_document", json!({"document": 999_999})).contains("no open document"));
    assert!(c.err("use_document", json!({})).contains("required"));
    assert!(c.err("use_document", json!({"document": true})).contains("must be"));
    assert_eq!(c.ok("diagnostics", json!({}))["target"], b, "a refused change leaves the target");
}

#[test]
fn checkpoint_list_restore_and_compare_round_trip() {
    let mut c = Client::new();
    let one = c.create(json!({"type": "frame", "name": "One", "width": 100, "height": 100}));
    let cp = c.ok("checkpoint", json!({"name": "before", "description": "one frame"}));
    let version = cp["id"].as_str().unwrap().to_string();
    assert_eq!(cp["name"], "before", "{cp}");
    c.create(json!({"type": "frame", "name": "Two", "width": 100, "height": 100}));
    assert_eq!(c.page_children().len(), 2);

    let list = c.ok("list_versions", json!({}));
    let found = list["versions"].as_array().unwrap().iter().find(|v| v["id"] == version.as_str()).cloned().unwrap();
    assert_eq!((found["name"].clone(), found["description"].clone()), (json!("before"), json!("one frame")));
    assert!(list["total"].as_u64().unwrap() >= 1);

    // What changed since: one frame by `node`, or every top-level frame of the page.
    let one_frame = c.ok("compare_version", json!({"id": version, "node": one}));
    assert_eq!(one_frame["version"]["id"], version.as_str(), "{one_frame}");
    let all = c.ok("compare_version", json!({"id": version}));
    assert_eq!(all["frames"], 2, "{all}");
    let reported: usize = ["changed", "unchanged", "failed"].iter().map(|k| all.get(*k).and_then(Value::as_array).map_or(0, Vec::len)).sum();
    assert_eq!(reported, 2, "{all}");
    assert!(!c.err("compare_version", json!({"id": "no such version"})).is_empty());

    // Restoring needs `confirm`, and is one undo step.
    for args in [json!({"id": version}), json!({"id": version, "confirm": false})] {
        assert!(c.err("restore_version", args).contains("confirm"));
    }
    assert_eq!(c.page_children().len(), 2, "a refused restore changes nothing");
    c.ok("restore_version", json!({"id": version, "confirm": true}));
    let kids_now = c.page_children();
    assert_eq!((kids_now.len(), kids_now[0]["name"].clone()), (1, json!("One")));
    c.ok("undo", json!({}));
    assert_eq!(c.page_children().len(), 2);
    assert!(!c.err("restore_version", json!({"id": "no such version", "confirm": true})).is_empty());
}

#[test]
fn list_versions_returns_the_newest_ones_and_checkpoint_checks_its_arguments() {
    let mut c = Client::new();
    c.create(json!({"type": "frame"}));
    let names = ["v1", "v2", "v3"];
    for n in names {
        c.ok("checkpoint", json!({"name": n}));
    }
    let list = c.ok("list_versions", json!({"limit": 2}));
    let shown: Vec<&str> = list["versions"].as_array().unwrap().iter().map(|v| v["name"].as_str().unwrap()).collect();
    assert_eq!(shown, ["v2", "v3"], "{list}");
    assert!(list["total"].as_u64().unwrap() >= 3);
    assert!(c.err("checkpoint", json!({"name": 5})).contains("must be a string"));
    assert!(c.err("list_versions", json!({"limit": "many"})).contains("finite number"));
    assert!(c.err("restore_version", json!({})).contains("`id` is required"));
}

#[test]
fn the_new_tools_are_registered_documented_and_in_the_instructions() {
    let mut c = Client::new();
    let tail = ["use_document", "copy_between_documents", "checkpoint", "list_versions", "restore_version", "compare_version"];
    for name in tail {
        assert!(TOOL_NAMES.contains(&name), "{name} is registered");
    }
    let tools = c.rpc("tools/list", json!({}))["result"]["tools"].clone();
    let tools = tools.as_array().unwrap();
    let tool = |name: &str| tools.iter().find(|t| t["name"] == name).unwrap_or_else(|| panic!("no tool {name}")).clone();
    for (name, read_only) in [
        ("use_document", false),
        ("copy_between_documents", false),
        ("checkpoint", false),
        ("list_versions", true),
        ("restore_version", false),
        ("compare_version", true),
    ] {
        assert_eq!(tool(name)["annotations"]["readOnlyHint"], read_only, "{name}");
    }
    for name in ["checkpoint", "list_versions", "restore_version", "compare_version"] {
        assert!(tool(name)["inputSchema"]["properties"]["document"].is_object(), "{name} takes `document`");
    }
    for name in ["find_nodes", "set_properties", "run_command", "get_nodes"] {
        assert!(tool(name)["inputSchema"]["properties"]["documents"].is_object(), "{name} takes `documents`");
    }
    assert!(tool("open_file")["inputSchema"]["properties"]["focus"].is_object());
    assert!(tool("use_document")["inputSchema"]["properties"]["document"].is_object());
    assert_eq!(tool("restore_version")["inputSchema"]["required"], json!(["id"]));
    for name in ["use_document", "documents", "checkpoint", "copy_between_documents", "restore_version"] {
        assert!(INSTRUCTIONS.contains(name), "the server instructions name {name}");
    }

    let docs = include_str!("../../../../docs/mcp.md");
    for row in [
        "| `use_document {document}` |",
        r"| `copy_between_documents {from, to, ids \| where, parent?, x?, y?}` |",
        "| `checkpoint {name?, description?}` |",
        "| `list_versions {limit?}` |",
        "| `restore_version {id, confirm}` |",
        "| `compare_version {id, node?}` |",
        "| `run_command {command, params, continueOnError?, limit?, cursor?, documents?}` |",
        "| `open_file {path, variables?, reload?, focus?}`",
    ] {
        assert!(docs.contains(row), "docs/mcp.md has no row {row}");
    }
    assert!(docs.contains("FIGCRAFT_MCP_NO_AUTOSAVE=1"), "docs/mcp.md names the auto-save variable");
    assert!(docs.contains("auto-save was off: the MCP server turned it on"));
}
