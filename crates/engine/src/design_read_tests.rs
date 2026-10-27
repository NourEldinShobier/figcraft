//! The design-context read commands: layer outline, variables used, design context, search.

use serde_json::{Value, json};

use crate::Session;
use crate::params::id_of;

fn id(v: Value) -> figcraft_doc::NodeId {
    id_of(&v["id"]).unwrap()
}

/// A frame "Card <1>" (10,20 100x50) holding a rectangle "Bg" bound to `color/brand` and `radius/sm`,
/// then a hidden rectangle "Label" (the next id after "Bg").
fn card() -> (Session, figcraft_doc::NodeId, figcraft_doc::NodeId) {
    let mut s = Session::new();
    let f = id(s.execute("frame.create", &json!({"x": 10, "y": 20, "width": 100, "height": 50, "name": "Card <1>"})).unwrap());
    let r = id(s.execute("shape.rectangle", &json!({"x": 5, "y": 6, "width": 30, "height": 40, "parent": f, "name": "Bg"})).unwrap());
    let t = id(s.execute("shape.rectangle", &json!({"x": 1, "y": 2, "width": 3, "height": 4, "parent": f, "name": "Label"})).unwrap());
    s.execute("node.set", &json!({"id": t, "props": {"visible": false}})).unwrap();
    s.execute("variable.createCollection", &json!({"name": "Tokens"})).unwrap();
    s.execute("variable.create", &json!({"collection": "Tokens", "name": "color/brand", "type": "color", "value": "#3366FF"})).unwrap();
    s.execute("variable.create", &json!({"collection": "Tokens", "name": "radius/sm", "type": "number", "value": 6})).unwrap();
    s.execute("variable.bind", &json!({"ids": [r], "field": "fill", "variable": "color/brand"})).unwrap();
    s.execute("variable.bind", &json!({"ids": [r], "field": "cornerRadius", "variable": "radius/sm"})).unwrap();
    (s, f, r)
}

#[test]
fn metadata_is_a_sparse_xml_outline() {
    let (mut s, f, r) = card();
    let out = s.execute("inspect.metadata", &json!({"id": f})).unwrap();
    let t = r.0 + 1;
    assert_eq!(
        out["xml"],
        format!(
            "<frame id=\"{}\" name=\"Card &lt;1&gt;\" x=\"10\" y=\"20\" width=\"100\" height=\"50\">\n\
             <rectangle id=\"{}\" name=\"Bg\" x=\"5\" y=\"6\" width=\"30\" height=\"40\"/>\n\
             <rectangle id=\"{t}\" name=\"Label\" x=\"1\" y=\"2\" width=\"3\" height=\"4\" hidden=\"true\"/>\n\
             </frame>\n",
            f.0, r.0,
        )
    );
    assert_eq!((out["nodes"].as_u64(), out["truncated"].as_bool()), (Some(3), Some(false)));
}

#[test]
fn metadata_depth_zero_counts_omitted_children() {
    let (mut s, f, _) = card();
    let out = s.execute("inspect.metadata", &json!({"id": f, "depth": 0})).unwrap();
    assert_eq!(out["xml"], format!("<frame id=\"{}\" name=\"Card &lt;1&gt;\" x=\"10\" y=\"20\" width=\"100\" height=\"50\" omitted=\"2\"/>\n", f.0));
    assert_eq!(out["truncated"], true);
}

#[test]
fn metadata_max_nodes_bounds_the_outline() {
    let (mut s, f, _) = card();
    let out = s.execute("inspect.metadata", &json!({"id": f, "maxNodes": 2})).unwrap();
    assert_eq!(out["nodes"], 2);
    assert_eq!(out["truncated"], true);
    assert!(out["xml"].as_str().unwrap().contains("omitted=\"1\""));
}

#[test]
fn metadata_defaults_to_the_page_and_rejects_bad_input() {
    let (mut s, f, _) = card();
    let xml = s.execute("inspect.metadata", &json!({})).unwrap()["xml"].as_str().unwrap().to_string();
    assert!(xml.starts_with("<page id="), "{xml}");
    assert!(xml.contains(&format!("<frame id=\"{}\"", f.0)));
    assert!(s.execute("inspect.metadata", &json!({"id": 99999})).is_err());
    assert!(s.execute("inspect.metadata", &json!({"depth": 33})).is_err());
    assert!(s.execute("inspect.metadata", &json!({"maxNodes": 0})).is_err());
}

#[test]
fn variable_defs_resolve_bound_variables_of_descendants() {
    let (mut s, f, r) = card();
    let out = s.execute("inspect.variableDefs", &json!({"id": f})).unwrap();
    assert_eq!(out, json!({"count": 2, "variables": {"color/brand": "#3366FF", "radius/sm": "6"}}));
    let none = s.execute("inspect.variableDefs", &json!({"id": r.0 + 1})).unwrap();
    assert_eq!(none, json!({"count": 0, "variables": {}}));
}

#[test]
fn variable_defs_follow_the_layers_mode() {
    let (mut s, f, r) = card();
    s.execute("variable.addMode", &json!({"collection": "Tokens", "name": "Dark"})).unwrap();
    s.execute("variable.set", &json!({"variable": "color/brand", "mode": "Dark", "value": "#FF0000"})).unwrap();
    s.execute("variable.setMode", &json!({"ids": [f], "collection": "Tokens", "mode": "Dark"})).unwrap();
    let out = s.execute("inspect.variableDefs", &json!({"id": r})).unwrap();
    assert_eq!(out["variables"]["color/brand"], "#FF0000");
}

#[test]
fn design_context_bundles_code_properties_variables_and_outline() {
    let (mut s, f, _) = card();
    let out = s.execute("inspect.designContext", &json!({"id": f, "language": "css"})).unwrap();
    assert_eq!((out["id"].as_u64(), out["name"].as_str(), out["type"].as_str(), out["language"].as_str()), (Some(f.0), Some("Card <1>"), Some("FRAME"), Some("css")));
    assert!(out["code"].as_str().unwrap().contains("width: 100px"), "{}", out["code"]);
    assert_eq!(out["codeTruncated"], false);
    assert_eq!(out["variables"], json!({"color/brand": "#3366FF", "radius/sm": "6"}));
    assert_eq!(out["properties"][0]["title"], "Layout");
    assert!(out["outline"].as_str().unwrap().starts_with("<frame "));
    // html is the default language
    assert_eq!(s.execute("inspect.designContext", &json!({"id": f})).unwrap()["language"], "html");
    assert!(s.execute("inspect.designContext", &json!({"id": f, "language": "cobol"})).is_err());
    s.execute("select.none", &json!({})).unwrap();
    assert!(s.execute("inspect.designContext", &json!({})).is_err(), "no selection, no id");
}

#[test]
fn search_finds_components_variables_and_styles_by_text() {
    let (mut s, f, _) = card();
    s.execute("component.create", &json!({"ids": [f]})).unwrap();
    s.execute("style.create", &json!({"type": "paint", "name": "Brand/Blue", "from": f})).ok();
    let all = s.execute("inspect.searchDesignSystem", &json!({"query": "BRAND"})).unwrap();
    assert_eq!(all["variables"], json!([{"id": all["variables"][0]["id"], "name": "color/brand", "collection": "Tokens", "type": "color", "value": "#3366FF"}]));
    assert_eq!(all["components"].as_array().unwrap().len(), 0);
    let comps = s.execute("inspect.searchDesignSystem", &json!({"query": "card", "types": ["component"]})).unwrap();
    assert_eq!((comps["components"].as_array().unwrap().len(), comps["variables"].as_array().unwrap().len(), comps["total"].as_u64()), (1, 0, Some(1)));
    assert_eq!(comps["components"][0]["name"], "Card <1>");
    assert_eq!(comps["components"][0]["type"], "COMPONENT");
    let limited = s.execute("inspect.searchDesignSystem", &json!({"query": "", "types": "variable", "limit": 1})).unwrap();
    assert_eq!((limited["variables"].as_array().unwrap().len(), limited["total"].as_u64()), (1, Some(2)));
    assert!(s.execute("inspect.searchDesignSystem", &json!({"limit": 0})).is_err());
}
