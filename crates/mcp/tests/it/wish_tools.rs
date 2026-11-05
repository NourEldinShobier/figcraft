//! `find_nodes`, `get_nodes`, `contact_sheet`, the generic command params (`where`, `list`,
//! `dryRun`, `keepSelection`) and `save_file`'s unsaved-changes flags.

use crate::common::{Client, approx, png_dims};
use figcraft_mcp::TOOL_NAMES;
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

/// `n` red frames "Slide 0", "Slide 1", ... of `w` x `h`, side by side.
fn slides(c: &mut Client, n: u32, w: u32, h: u32) -> Vec<Value> {
    (0..n)
        .map(|i| c.create(json!({"type": "frame", "name": format!("Slide {i}"), "x": i * (w + 10), "width": w, "height": h, "fill": "#FF0000"})))
        .collect()
}

fn image_and_legend(r: &Value) -> ((u32, u32), Value) {
    assert_eq!(r["isError"], false, "{r}");
    let content = r["content"].as_array().unwrap();
    assert_eq!((content[0]["type"].clone(), content[0]["mimeType"].clone()), (json!("image"), json!("image/png")));
    let legend: Value = serde_json::from_str(content[1]["text"].as_str().unwrap()).unwrap();
    (png_dims(content[0]["data"].as_str().unwrap()), legend)
}

#[test]
fn find_nodes_searches_without_selecting_anything() {
    let mut c = Client::new();
    let (_, a, b, _) = cards(&mut c);
    c.ok("select", json!({"ids": [a]}));
    let r = c.ok("find_nodes", json!({"type": "TEXT", "nameGlob": "Label*"}));
    assert_eq!(r["total"], 2, "{r}");
    assert_eq!(sorted(&r["ids"]), sorted(&json!([a, b])));
    assert!(r.get("nodes").is_none(), "no `nodes` without details: {r}");
    let sel = c.ok("get_selection", json!({}));
    assert_eq!((sel["count"].clone(), sel["selection"][0]["id"].clone()), (json!(1), a), "the selection is untouched");
    let one = c.ok("find_nodes", json!({"type": "TEXT", "limit": 1}));
    assert_eq!((one["ids"].as_array().unwrap().len(), one["total"].clone()), (1, json!(2)));
    assert!(c.err("find_nodes", json!({"bogus": 1})).contains("unknown argument"));
    assert!(c.err("find_nodes", json!({"where": {"type": "TEXT"}})).contains("unknown argument"));
}

#[test]
fn find_nodes_details_and_page() {
    let mut c = Client::new();
    let (frame, a, _, _) = cards(&mut c);
    let r = c.ok("find_nodes", json!({"name": "Label A", "details": true}));
    let n = &r["nodes"][0];
    assert_eq!((n["id"].clone(), n["name"].clone(), n["type"].clone(), n["parent"].clone()), (a, json!("Label A"), json!("TEXT"), frame));
    assert_eq!(n.as_object().unwrap().len(), 8, "id, name, type, parent, x, y, width, height: {n}");
    let page = c.doc()["pages"][0]["name"].clone();
    let by_name = c.ok("find_nodes", json!({"type": "TEXT", "page": page}));
    let everywhere = c.ok("find_nodes", json!({"type": "TEXT", "page": "all"}));
    assert_eq!((by_name["total"].clone(), everywhere["total"].clone()), (json!(2), json!(2)));
}

#[test]
fn get_nodes_outlines_several_layers_in_one_call() {
    let mut c = Client::new();
    let (frame, _, _, shape) = cards(&mut c);
    let r = c.ok("get_nodes", json!({"ids": [frame, shape]}));
    assert_eq!((r["count"].clone(), r["total"].clone()), (json!(2), json!(2)), "{r}");
    assert!(r.get("truncated").is_none());
    let f = &r["nodes"][0];
    assert_eq!(
        (f["id"].clone(), f["name"].clone(), f["type"].clone(), f["childCount"].clone()),
        (frame.clone(), json!("Cards"), json!("FRAME"), json!(3))
    );
    assert!(approx(&f["width"], 300.0) && approx(&f["height"], 200.0), "{f}");
    let kids = f["children"].as_array().unwrap();
    let names: Vec<&str> = kids.iter().map(|k| k["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Label A", "Label B", "Label shape"]);
    assert!(kids[0].get("fills").is_none(), "only the default fields: {}", kids[0]);
    assert_eq!(r["nodes"][1]["id"], shape);

    let flat = c.ok("get_nodes", json!({"ids": [frame], "depth": 0}));
    let f = &flat["nodes"][0];
    assert_eq!(f["childCount"], 3);
    assert!(f.get("children").and_then(Value::as_array).is_none_or(Vec::is_empty), "depth 0 lists no children: {f}");

    let some = c.ok("get_nodes", json!({"ids": [frame], "fields": ["name"]}));
    let f = &some["nodes"][0];
    assert!(f.get("x").is_none() && f.get("type").is_none() && f["name"] == "Cards", "{f}");
}

#[test]
fn get_nodes_goes_down_to_depth_and_names_layers_by_where() {
    let mut c = Client::new();
    let outer = c.create(json!({"type": "frame", "name": "Outer"}));
    let inner = c.create(json!({"type": "frame", "name": "Inner", "parent": outer}));
    c.create(json!({"type": "text", "text": "Deep", "name": "Deepest", "parent": inner}));
    let one = c.ok("get_nodes", json!({"ids": [outer]}));
    assert!(one["nodes"][0]["children"][0].get("children").is_none_or(|k| k.as_array().is_some_and(Vec::is_empty)), "{one}");
    let two = c.ok("get_nodes", json!({"ids": [outer], "depth": 2}));
    assert_eq!(two["nodes"][0]["children"][0]["children"][0]["name"], "Deepest", "{two}");

    c.ok("select", json!({"ids": [outer]}));
    let found = c.ok("get_nodes", json!({"where": {"type": "TEXT"}}));
    assert_eq!((found["count"].clone(), found["nodes"][0]["name"].clone()), (json!(1), json!("Deepest")));
    assert_eq!(c.ok("get_selection", json!({}))["selection"][0]["id"], outer, "reading does not select");
}

#[test]
fn get_nodes_says_what_it_left_out_and_what_it_cannot_read() {
    let mut c = Client::new();
    let (_, a, b, shape) = cards(&mut c);
    let r = c.ok("get_nodes", json!({"ids": [a, b, shape], "limit": 2}));
    assert_eq!((r["count"].clone(), r["total"].clone()), (json!(2), json!(3)), "{r}");
    assert_eq!((r["truncated"]["returned"].clone(), r["truncated"]["total"].clone()), (json!(2), json!(3)));

    let missing = c.ok("get_nodes", json!({"ids": [a, 999_999]}));
    assert_eq!(missing["count"], 2);
    assert_eq!(missing["nodes"][0]["id"], a);
    assert!(missing["nodes"][1]["error"].as_str().unwrap().contains("no such"), "{missing}");

    for args in [json!({}), json!({"ids": [a], "where": {"type": "TEXT"}}), json!({"ids": [a], "list": "x"})] {
        assert!(c.err("get_nodes", args.clone()).contains("exactly one"), "{args}");
    }
    assert!(!c.err("get_nodes", json!({"ids": [a], "fields": ["bogus"]})).is_empty());
    assert!(c.err("get_nodes", json!({"ids": [a], "x": 1})).contains("unknown argument"));
}

#[test]
fn contact_sheet_draws_the_frames_in_a_grid_and_returns_a_legend() {
    let mut c = Client::new();
    let ids = slides(&mut c, 3, 100, 80);
    c.ok("select", json!({"ids": [ids[0]]}));
    let r = c.call("contact_sheet", json!({"ids": ids, "columns": 2, "scale": 1}));
    let (dims, legend) = image_and_legend(&r);
    // Two columns of 100, two rows of 80, an 8 px margin and gap.
    assert_eq!(dims, (8 + 100 + 8 + 100 + 8, 8 + 80 + 8 + 80 + 8));
    assert_eq!((legend["width"].clone(), legend["height"].clone(), legend["columns"].clone()), (json!(dims.0), json!(dims.1), json!(2)));
    let frames = legend["frames"].as_array().unwrap();
    assert_eq!(frames.len(), 3);
    for (i, f) in frames.iter().enumerate() {
        assert_eq!((f["index"].clone(), f["id"].clone(), f["name"].clone()), (json!(i + 1), ids[i].clone(), json!(format!("Slide {i}"))), "{f}");
        assert!(approx(&f["width"], 100.0) && approx(&f["height"], 80.0), "{f}");
    }
    assert!(legend.get("truncated").is_none() && legend.get("failed").is_none());
    let sel = c.ok("get_selection", json!({}));
    assert_eq!((sel["count"].clone(), sel["selection"][0]["id"].clone()), (json!(1), ids[0].clone()), "the selection is untouched");

    // A smaller scale makes smaller cells; three columns fit on one row.
    let (dims, _) = image_and_legend(&c.call("contact_sheet", json!({"ids": ids, "columns": 3, "scale": 0.5})));
    assert_eq!(dims, (8 + 3 * 50 + 2 * 8 + 8, 8 + 40 + 8));
}

#[test]
fn contact_sheet_stays_within_max_size_and_by_where() {
    let mut c = Client::new();
    slides(&mut c, 5, 400, 300);
    let (dims, legend) = image_and_legend(&c.call("contact_sheet", json!({"where": {"type": "FRAME"}, "columns": 2, "scale": 1, "maxSize": 300})));
    assert!(dims.0 <= 303 && dims.1 <= 303 && dims.0 > 100, "{dims:?}");
    assert_eq!(legend["frames"].as_array().unwrap().len(), 5, "{legend}");
}

#[test]
fn contact_sheet_draws_at_most_64_frames() {
    let mut c = Client::new();
    slides(&mut c, 66, 20, 20);
    let (_, legend) = image_and_legend(&c.call("contact_sheet", json!({"where": {"type": "FRAME"}, "scale": 1})));
    assert_eq!(legend["frames"].as_array().unwrap().len(), 64, "{legend}");
    assert!(legend["truncated"].as_str().unwrap().contains("66"), "{legend}");
}

#[test]
fn contact_sheet_reports_what_it_cannot_draw() {
    let mut c = Client::new();
    let (_, a, _, _) = cards(&mut c);
    let (_, legend) = image_and_legend(&c.call("contact_sheet", json!({"ids": [a, 999_999]})));
    assert_eq!(legend["frames"].as_array().unwrap().len(), 1, "{legend}");
    assert_eq!(legend["failed"][0]["id"], 999_999);
    assert!(c.err("contact_sheet", json!({})).contains("exactly one"));
    assert!(c.err("contact_sheet", json!({"ids": []})).contains("no layers"));
    assert!(c.err("contact_sheet", json!({"ids": [999_999]})).contains("no layer could be drawn"));
    assert!(c.err("contact_sheet", json!({"ids": [a], "bogus": 1})).contains("unknown argument"));
}

#[test]
fn generic_params_are_accepted_and_where_is_checked_inside_them() {
    let mut c = Client::new();
    let e = c.err("run_command", json!({"command": "node.set", "params": {"where": {"bogus": 1}, "props": {"opacity": 0.5}}}));
    assert!(e.contains("params.where.bogus"), "{e}");
    let e = c.err("batch", json!({"commands": [{"command": "node.set", "params": {"where": {"bogus": 1}}}]}));
    assert!(e.contains("commands[0].params.where.bogus"), "{e}");
    // Other keys of `params` stay open: the command checks them.
    let r = c.call("run_command", json!({"command": "node.create", "params": {"type": "rectangle", "x": 5}}));
    assert_eq!(r["isError"], false, "{r}");
}

#[test]
fn dry_run_changes_nothing_and_keep_selection_leaves_the_selection() {
    let mut c = Client::new();
    let (frame, a, b, _) = cards(&mut c);
    c.ok("select", json!({"ids": [frame]}));

    let r = c.ok("set_properties", json!({"where": {"type": "TEXT"}, "props": {"opacity": 0.5}, "dryRun": true}));
    assert_eq!((r["dryRun"].clone(), r["matched"].clone()), (json!(true), json!(2)), "{r}");
    assert!(c.node(&a).get("opacity").is_none() && c.node(&b).get("opacity").is_none());

    let r = c.ok("run_command", json!({"command": "node.set", "params": {"where": {"type": "TEXT"}, "props": {"opacity": 0.5}, "dryRun": true}}));
    assert_eq!(r["dryRun"], true, "{r}");
    assert!(c.node(&a).get("opacity").is_none());

    let before = c.page_children().len();
    let r = c.ok("create_node", json!({"type": "rectangle", "dryRun": true}));
    assert_eq!(r["dryRun"], true, "{r}");
    assert_eq!(c.page_children().len(), before, "nothing was created");

    c.ok("create_node", json!({"type": "rectangle", "keepSelection": true}));
    let sel = c.ok("get_selection", json!({}));
    assert_eq!((sel["count"].clone(), sel["selection"][0]["id"].clone()), (json!(1), frame), "{sel}");
    assert!(c.err("set_properties", json!({"ids": [a], "props": {"opacity": 0.5}, "dryRun": "yes"})).contains("boolean"));
}

#[test]
fn select_dry_run_reports_without_selecting() {
    let mut c = Client::new();
    let (frame, a, b, _) = cards(&mut c);
    c.ok("select", json!({"ids": [frame]}));
    let r = c.ok("select", json!({"where": {"type": "TEXT"}, "dryRun": true}));
    assert_eq!((r["dryRun"].clone(), r["matched"].clone()), (json!(true), json!(2)), "{r}");
    assert_eq!(sorted(&r["wouldSelect"]), sorted(&json!([a, b])));
    assert_eq!(r["selection"], json!([frame]), "the selection is unchanged");
}

#[test]
fn saved_lists_name_layers_for_the_tools() {
    let mut c = Client::new();
    let (frame, a, b, _) = cards(&mut c);
    c.run("list.save", json!({"name": "labels", "ids": [a, b]}));
    let r = c.ok("get_nodes", json!({"list": "labels"}));
    assert_eq!(r["count"], 2, "{r}");
    c.ok("select", json!({"ids": [frame]}));
    let r = c.ok("select", json!({"list": "labels", "dryRun": true}));
    assert_eq!(sorted(&r["wouldSelect"]), sorted(&json!([a, b])), "{r}");
    let r = c.ok("set_properties", json!({"list": "labels", "props": {"opacity": 0.5}}));
    assert_eq!(r["matched"], 2, "{r}");
    assert_eq!((c.node(&a)["opacity"].clone(), c.node(&b)["opacity"].clone()), (json!(0.5), json!(0.5)));
    let (_, legend) = image_and_legend(&c.call("contact_sheet", json!({"list": "labels"})));
    assert_eq!(legend["frames"].as_array().unwrap().len(), 2);
    assert!(c.err("select", json!({"list": "labels", "where": {"type": "TEXT"}})).contains("not both"));
    assert!(c.err("select", json!({"list": "labels", "ids": [a]})).contains("not both"));
}

#[test]
fn headless_save_file_adds_no_unsaved_flags() {
    let mut c = Client::new();
    c.create(json!({"type": "rectangle"}));
    let dir = std::env::temp_dir().join(format!("figcraft-mcp-wish-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let saved = c.ok("save_file", json!({"path": dir.join("a.figcraft").to_string_lossy()}));
    assert!(saved.get("unsavedBefore").is_none() && saved.get("changedOnDisk").is_none(), "only an attached window reports them: {saved}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_new_tools_are_registered_read_only_and_documented() {
    let mut c = Client::new();
    assert_eq!(TOOL_NAMES.len(), 35);
    let tools = c.rpc("tools/list", json!({}))["result"]["tools"].clone();
    let tools = tools.as_array().unwrap();
    let tool = |name: &str| tools.iter().find(|t| t["name"] == name).unwrap_or_else(|| panic!("no tool {name}"));
    for name in ["find_nodes", "get_nodes", "contact_sheet"] {
        assert!(TOOL_NAMES.contains(&name));
        assert_eq!(tool(name)["annotations"]["readOnlyHint"], true, "{name}");
        assert!(tool(name)["inputSchema"]["properties"]["document"].is_object(), "{name} takes `document`");
    }
    let keys = |v: &Value| -> Vec<String> {
        let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
        k.sort_unstable();
        k
    };
    let mut want = keys(&tool("select")["inputSchema"]["properties"]["where"]["properties"]);
    want.push("details".into());
    want.sort_unstable();
    assert_eq!(keys(&tool("find_nodes")["inputSchema"]["properties"]).into_iter().filter(|k| k != "document").collect::<Vec<_>>(), want);
    for name in ["select", "set_properties", "get_nodes", "contact_sheet"] {
        assert!(tool(name)["inputSchema"]["properties"]["where"]["properties"]["edgeGap"].is_object(), "{name}");
    }
    for name in ["set_properties", "select"] {
        let p = &tool(name)["inputSchema"]["properties"];
        assert!(p["list"].is_object() && p["dryRun"].is_object() && p["keepSelection"].is_object(), "{name}");
    }

    let docs = include_str!("../../../../docs/mcp.md");
    for name in TOOL_NAMES {
        assert!(docs.contains(&format!("`{name} ")) || docs.contains(&format!("`{name}`")), "docs/mcp.md does not document `{name}`");
    }
    for row in [
        "| `find_nodes {",
        r"| `get_nodes {ids \| where \| list, depth?, fields?, limit?}` |",
        r"| `contact_sheet {ids \| where \| list, scale?, columns?, maxSize?}` |",
    ] {
        assert!(docs.contains(row), "docs/mcp.md has no row {row}");
    }
    for name in ["find_nodes", "get_nodes", "contact_sheet", "get_selection", "diagnostics"] {
        assert!(figcraft_mcp::INSTRUCTIONS.contains(name), "the server instructions name {name}");
    }
}
