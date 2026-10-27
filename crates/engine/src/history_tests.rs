//! `history.*`, `layer.autoName` and the fuzzy design-system search.

use serde_json::{Value, json};

use crate::Session;
use crate::params::id_of;

fn id(v: &Value) -> figcraft_doc::NodeId {
    id_of(&v["id"]).unwrap()
}

fn six_icons(s: &mut Session) -> Value {
    let steps: Vec<Value> =
        (0..6).map(|i| json!({"command": "shape.rectangle", "params": {"x": i * 30, "y": 0, "width": 24, "height": 24, "name": format!("icon{i}")}})).collect();
    s.execute("command.batch", &json!({"label": "Add 6 icons", "commands": steps})).unwrap()
}

#[test]
fn a_batch_label_and_its_changed_ids_are_listed_and_revert_restores_the_hash() {
    let mut s = Session::new();
    s.execute("shape.rectangle", &json!({"x": 0, "y": 100, "width": 10, "height": 10})).unwrap();
    let before = s.execute("history.hash", &json!({})).unwrap()["hash"].as_str().unwrap().to_string();
    let made = six_icons(&mut s);
    let mut want: Vec<u64> = made.as_array().unwrap().iter().map(|r| id(r).0).collect();
    want.sort_unstable();
    assert_eq!(want.len(), 6);

    let list = s.execute("history.list", &json!({})).unwrap();
    let entries = list["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    let last = &entries[1];
    assert_eq!((last["label"].as_str(), last["kind"].as_str(), last["index"].as_u64()), (Some("Add 6 icons"), Some("batch"), Some(1)));
    let mut got: Vec<u64> = last["changedIds"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
    got.sort_unstable();
    assert_eq!(got, want);
    assert_eq!(list["hash"].as_str().unwrap().len(), 16);
    assert_ne!(list["hash"].as_str().unwrap(), before);
    // The single command says what it created.
    assert_eq!(entries[0]["label"], "Rectangle");

    let reverted = s.execute("history.revertTo", &json!({"index": 1})).unwrap();
    assert_eq!(reverted["hash"].as_str().unwrap(), before);
    let list = s.execute("history.list", &json!({})).unwrap();
    assert_eq!((list["applied"].as_u64(), list["total"].as_u64()), (Some(1), Some(2)));
    assert_eq!(list["entries"][1]["applied"], false);
    // Forward again to the latest state.
    let again = s.execute("history.revertTo", &json!({"index": 2})).unwrap();
    assert_eq!(again["previousHash"].as_str().unwrap(), before);
    assert_ne!(again["hash"].as_str().unwrap(), before);
    assert!(s.execute("history.revertTo", &json!({"index": 3})).is_err());
    assert!(s.execute("history.revertTo", &json!({"index": -1})).is_err());
}

#[test]
fn create_layer_steps_are_named_after_what_they_made() {
    let mut s = Session::new();
    s.execute("node.create", &json!({"type": "rectangle", "name": "Card", "width": 10, "height": 10})).unwrap();
    let list = s.execute("history.list", &json!({"limit": 1})).unwrap();
    assert_eq!(list["entries"][0]["label"], "Create layer \"Card\"");
}

#[test]
fn hash_is_equal_for_equal_documents_and_changes_with_an_edit() {
    let mut s = Session::new();
    let r = id(&s.execute("shape.rectangle", &json!({"width": 10, "height": 10})).unwrap());
    let h = |s: &mut Session| s.execute("history.hash", &json!({})).unwrap()["hash"].as_str().unwrap().to_string();
    let a = h(&mut s);
    assert_eq!(a, h(&mut s));
    s.execute("node.set", &json!({"id": r, "props": {"width": 11}})).unwrap();
    assert_ne!(a, h(&mut s));
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(a, h(&mut s));
}

#[test]
fn revert_refuses_to_cross_a_version_restore() {
    let mut s = Session::new();
    s.execute("shape.rectangle", &json!({"width": 10, "height": 10})).unwrap();
    s.execute("file.saveVersion", &json!({"name": "v1"})).unwrap();
    s.execute("shape.rectangle", &json!({"width": 20, "height": 20})).unwrap();
    s.execute("version.restore", &json!({"index": 0})).unwrap();
    let list = s.execute("history.list", &json!({})).unwrap();
    assert_eq!(list["entries"].as_array().unwrap().last().unwrap()["kind"], "restore");
    let e = s.execute("history.revertTo", &json!({"index": 0})).unwrap_err().to_string();
    assert!(e.contains("version restore"), "{e}");
    assert_eq!(s.execute("history.list", &json!({})).unwrap()["applied"], 3);
}

fn name_of(s: &mut Session, i: figcraft_doc::NodeId) -> String {
    s.doc().unwrap().node(i).unwrap().name.clone()
}

#[test]
fn generic_names_follow_their_context_in_one_undo_step() {
    let mut s = Session::new();
    let folder = id(&s.execute("frame.create", &json!({"x": 0, "y": 0, "width": 24, "height": 24, "name": "folder"})).unwrap());
    let vec = id(&s.execute("vector.create", &json!({"path": "M0 0L10 10L0 10Z", "parent": folder, "name": "Vector"})).unwrap());
    let button = id(&s.execute("frame.create", &json!({"x": 100, "y": 0, "width": 120, "height": 40, "name": "Primary Button"})).unwrap());
    let bg = id(&s.execute("shape.rectangle", &json!({"width": 120, "height": 40, "parent": button, "name": "Rectangle 12"})).unwrap());
    let label = id(&s.execute("text.create", &json!({"text": "Sign in", "x": 10, "y": 10, "parent": button})).unwrap());
    let kept = id(&s.execute("shape.rectangle", &json!({"width": 5, "height": 5, "parent": button, "name": "Divider"})).unwrap());
    s.execute("node.set", &json!({"id": label, "props": {"name": "Text 2"}})).unwrap();
    let depth = s.undo_depth().0;

    let dry = s.execute("layer.autoName", &json!({"dryRun": true, "scope": "page"})).unwrap();
    assert_eq!(dry["count"], 3);
    assert_eq!(name_of(&mut s, vec), "Vector");
    assert_eq!(s.undo_depth().0, depth);

    let out = s.execute("layer.autoName", &json!({"scope": "page"})).unwrap();
    assert_eq!(out["renames"], dry["renames"]);
    assert_eq!(name_of(&mut s, vec), "icon/folder");
    assert_eq!(name_of(&mut s, bg), "button/bg");
    assert_eq!(name_of(&mut s, label), "Sign in");
    assert_eq!(name_of(&mut s, kept), "Divider");
    assert_eq!(s.undo_depth().0, depth + 1);

    // A second run finds nothing to do; one undo brings the old names back.
    assert_eq!(s.execute("layer.autoName", &json!({})).unwrap()["count"], 0);
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(name_of(&mut s, vec), "Vector");
    assert_eq!(name_of(&mut s, bg), "Rectangle 12");
}

#[test]
fn sibling_shapes_get_distinct_names() {
    let mut s = Session::new();
    let card = id(&s.execute("frame.create", &json!({"width": 200, "height": 100, "name": "Card"})).unwrap());
    let a = id(&s.execute("shape.ellipse", &json!({"width": 10, "height": 10, "parent": card, "name": "Ellipse"})).unwrap());
    let b = id(&s.execute("shape.ellipse", &json!({"width": 10, "height": 10, "parent": card, "name": "Ellipse 2"})).unwrap());
    s.execute("layer.autoName", &json!({"ids": [card]})).unwrap();
    assert_eq!((name_of(&mut s, a).as_str(), name_of(&mut s, b).as_str()), ("Card/ellipse", "Card/ellipse-2"));
    assert!(s.execute("layer.autoName", &json!({"scope": "everything"})).is_err());
}

#[test]
fn search_finds_a_component_set_by_a_variant_name_and_by_loose_letters() {
    let mut s = Session::new();
    let a = id(&s.execute("node.create", &json!({"type": "component", "name": "State=Default", "width": 10, "height": 10})).unwrap());
    let b = id(&s.execute("node.create", &json!({"type": "component", "name": "State=Hover", "width": 10, "height": 10})).unwrap());
    s.execute("component.combineVariants", &json!({"ids": [a, b]})).unwrap();
    let set = s.doc().unwrap().node(a).unwrap().parent.unwrap();
    s.execute("node.set", &json!({"id": set, "props": {"name": "Row"}})).unwrap();

    let by_variant = s.execute("inspect.searchDesignSystem", &json!({"query": "state=def", "types": ["component"]})).unwrap();
    let hit = &by_variant["components"][0];
    assert_eq!((hit["name"].as_str(), hit["matchedOn"].as_str(), hit["matchedVariant"]["name"].as_str()), (Some("Row"), Some("variant"), Some("State=Default")));
    assert_eq!(hit["score"], 81.0);
    assert_eq!(by_variant["semantic"], false);

    let by_name = s.execute("inspect.searchDesignSystem", &json!({"query": "ROW"})).unwrap();
    assert_eq!((by_name["components"][0]["matchedOn"].as_str(), by_name["components"][0]["score"].as_f64()), (Some("name"), Some(100.0)));
    // Letters in order, not adjacent.
    let loose = s.execute("inspect.searchDesignSystem", &json!({"query": "stdf"})).unwrap();
    assert_eq!(loose["components"][0]["name"], "Row");
    assert_eq!(s.execute("inspect.searchDesignSystem", &json!({"query": "zzz"})).unwrap()["total"], 0);
}
