//! `inspect.lintIcons` on a small icon set with known mistakes (literal expected values).

use serde_json::{Value, json};

use crate::Session;

fn ex(s: &mut Session, cmd: &str, p: Value) -> Value {
    s.execute(cmd, &p).unwrap_or_else(|e| panic!("{cmd}: {e}"))
}

fn icon(s: &mut Session, set: &Value, x: f64, name: &str) -> Value {
    ex(s, "node.create", json!({"type": "frame", "parent": set, "x": x, "y": 0, "width": 24, "height": 24, "name": name}))["id"].clone()
}

fn outline(s: &mut Session, icon: &Value, x: f64, w: f64, weight: f64, round: bool) -> Value {
    let id = ex(s, "node.create", json!({"type": "rectangle", "parent": icon, "x": x, "y": 3, "width": w, "height": 18, "name": "body"}))["id"].clone();
    let mut props = json!({"fill": "none", "stroke": "#111111", "strokeWeight": weight});
    if round {
        props["strokeCap"] = json!("round");
        props["strokeJoin"] = json!("round");
    }
    ex(s, "node.set", json!({"ids": [id], "props": props}));
    id
}

fn rules(report: &Value, id: &Value) -> Vec<String> {
    let mut v: Vec<String> = report["issues"].as_array().unwrap().iter().filter(|i| &i["id"] == id).map(|i| i["rule"].as_str().unwrap().to_string()).collect();
    v.sort();
    v
}

#[test]
fn lint_finds_the_odd_icon_out_and_offers_fixes() {
    let mut s = Session::new();
    let set = ex(&mut s, "node.create", json!({"type": "frame", "width": 200, "height": 100, "name": "icons"}))["id"].clone();
    let a = icon(&mut s, &set, 0.0, "arrow-left");
    let a_body = outline(&mut s, &a, 3.0, 18.0, 2.0, true);
    let b = icon(&mut s, &set, 50.0, "arrow_right");
    let b_body = outline(&mut s, &b, 3.5, 17.0, 1.5, false);
    let c = icon(&mut s, &set, 100.0, "Frame 1");
    let c_body = outline(&mut s, &c, 3.0, 18.0, 2.0, true);

    let r = ex(&mut s, "inspect.lintIcons", json!({"ids": [set]}));
    assert_eq!(r["icons"], 3);
    assert_eq!(r["expected"], json!({"strokeWeight": 2.0, "strokeCap": "round", "strokeJoin": "round", "cornerRadius": null, "color": "#111111", "padding": 2.0}));
    assert_eq!(rules(&r, &a_body), Vec::<String>::new());
    assert_eq!(rules(&r, &a), Vec::<String>::new());
    assert_eq!(rules(&r, &c_body), Vec::<String>::new());
    assert_eq!(rules(&r, &c), ["naming"]);
    assert_eq!(rules(&r, &b), ["naming"]);
    assert_eq!(rules(&r, &b_body), ["pixel-grid", "stroke-cap", "stroke-join", "stroke-width"]);
    let issue = |rule: &str| r["issues"].as_array().unwrap().iter().find(|i| i["id"] == b_body && i["rule"] == rule).unwrap().clone();
    assert_eq!(issue("stroke-width")["fix"], json!({"command": "node.set", "params": {"ids": [b_body], "props": {"strokeWeight": 2.0}}}));
    assert_eq!(issue("pixel-grid")["fix"], json!({"command": "node.set", "params": {"ids": [b_body], "props": {"x": 4.0, "y": 3.0}}}));
    assert_eq!(issue("stroke-cap")["found"], "none");
    assert_eq!(r["fixable"], 4);

    // Running the fixes leaves nothing to report for that icon.
    for rule in ["stroke-width", "stroke-cap", "stroke-join", "pixel-grid"] {
        let fix = issue(rule)["fix"].clone();
        ex(&mut s, fix["command"].as_str().unwrap(), fix["params"].clone());
    }
    let r = ex(&mut s, "inspect.lintIcons", json!({"ids": [set]}));
    assert_eq!(rules(&r, &b_body), Vec::<String>::new());
}

#[test]
fn lint_refuses_junk() {
    let mut s = Session::new();
    assert!(s.execute("inspect.lintIcons", &json!({})).is_err());
    let f = ex(&mut s, "node.create", json!({"type": "frame", "width": 24, "height": 24}))["id"].clone();
    for p in [json!({"ids": [f], "grid": 0}), json!({"ids": [f], "grid": "x"}), json!({"ids": [f], "padding": 99}), json!({"ids": [f], "color": 3})] {
        assert!(s.execute("inspect.lintIcons", &p).is_err(), "{p}");
    }
}