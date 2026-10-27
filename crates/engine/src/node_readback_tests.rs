//! What `node.set` accepts, `node.get` reports back (literal expected values).

use serde_json::{Value, json};

use crate::Session;

fn ex(s: &mut Session, cmd: &str, p: Value) -> Value {
    s.execute(cmd, &p).unwrap_or_else(|e| panic!("{cmd}: {e}"))
}

#[test]
fn stroke_cap_join_and_width_alias_read_back() {
    let mut s = Session::new();
    let id = ex(&mut s, "node.create", json!({"type": "rectangle", "width": 10, "height": 10}))["id"].clone();
    let n = ex(&mut s, "node.set", json!({"ids": [id], "props": {"stroke": "#000000", "strokeWidth": 2.5, "strokeCap": "round", "strokeJoin": "round"}}));
    drop(n);
    let n = ex(&mut s, "node.get", json!({"id": id}));
    assert_eq!(n["strokeWeight"], 2.5);
    assert_eq!(n["strokeCap"], "round");
    assert_eq!(n["strokeJoin"], "round");
    ex(&mut s, "node.set", json!({"ids": [id], "props": {"startCap": "square", "strokeJoin": "miter"}}));
    let n = ex(&mut s, "node.get", json!({"id": id}));
    assert_eq!((n["startCap"].clone(), n["endCap"].clone(), n["strokeJoin"].clone()), (json!("square"), json!("round"), json!("miter")));
    assert!(n.get("strokeCap").is_none());
}

#[test]
fn text_and_frame_settings_read_back() {
    let mut s = Session::new();
    let t = ex(&mut s, "node.create", json!({"type": "text", "text": "Hi"}))["id"].clone();
    ex(
        &mut s,
        "node.set",
        json!({"ids": [t], "props": {"textAlignVertical": "bottom", "textCase": "upper", "paragraphSpacing": 6, "paragraphIndent": 4, "constrainProportions": true}}),
    );
    let n = ex(&mut s, "node.get", json!({"id": t}));
    assert_eq!(n["textAlignVertical"], "bottom");
    assert_eq!(n["textCase"], "upper");
    assert_eq!(n["paragraphSpacing"], 6.0);
    assert_eq!(n["paragraphIndent"], 4.0);
    assert_eq!(n["constrainProportions"], true);
}
