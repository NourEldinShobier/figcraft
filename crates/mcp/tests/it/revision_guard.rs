//! `rev` on reads and `ifRev` on writes: an agent working from stale context is stopped.

use crate::common::Client;
use serde_json::{Value, json};

fn rev(v: &Value) -> u64 {
    v["rev"].as_u64().unwrap_or_else(|| panic!("no rev in {v}"))
}

#[test]
fn reads_carry_the_revision_and_it_moves_with_every_change() {
    let mut c = Client::new();
    let id = c.create(json!({"type": "rectangle", "name": "Box", "width": 10, "height": 10}));
    let doc = c.ok("inspect_document", json!({"depth": 1}));
    let node = c.ok("get_node", json!({"id": id, "depth": 0}));
    assert_eq!(rev(&doc), rev(&node));
    assert!(c.ok("find_nodes", json!({"name": "Box"}))["rev"].is_u64());
    assert!(c.ok("get_selection", json!({}))["rev"].is_u64());
    c.ok("set_properties", json!({"ids": [id], "props": {"name": "Other"}}));
    assert!(rev(&c.ok("get_node", json!({"id": id, "depth": 0}))) > rev(&node));
}

#[test]
fn a_stale_ifrev_changes_nothing_and_says_what_changed() {
    let mut c = Client::new();
    let id = c.create(json!({"type": "rectangle", "name": "Box", "width": 10, "height": 10}));
    let seen = rev(&c.ok("get_node", json!({"id": id, "depth": 0})));
    c.ok("set_properties", json!({"ids": [id], "props": {"name": "Renamed by someone else"}}));
    for (tool, args) in [
        ("set_properties", json!({"ids": [id], "props": {"name": "Mine"}, "ifRev": seen})),
        ("run_command", json!({"command": "layer.rename", "params": {"ids": [id], "name": "Mine"}, "ifRev": seen})),
        ("create_node", json!({"type": "rectangle", "ifRev": seen})),
        ("batch", json!({"commands": [{"command": "edit.undo"}], "ifRev": seen})),
        ("run_script", json!({"code": "1", "ifRev": seen})),
        ("undo", json!({"ifRev": seen})),
        ("redo", json!({"ifRev": seen})),
    ] {
        let e = c.err(tool, args);
        assert!(e.contains(&format!("you read revision {seen}")) && e.contains("nothing was changed"), "{tool}: {e}");
        assert!(e.contains("Re-read"), "{tool}: {e}");
    }
    assert_eq!(c.ok("get_node", json!({"id": id, "depth": 0}))["name"], "Renamed by someone else");
    let e = c.err("set_properties", json!({"ids": [id], "props": {"name": "Mine"}, "ifRev": seen}));
    assert!(e.contains("latest undo steps"), "the history labels say what changed: {e}");
}

#[test]
fn a_current_ifrev_applies_and_returns_the_next_rev_to_chain() {
    let mut c = Client::new();
    let id = c.create(json!({"type": "rectangle", "name": "Box", "width": 10, "height": 10}));
    let mut at = rev(&c.ok("get_node", json!({"id": id, "depth": 0})));
    for name in ["One", "Two"] {
        let r = c.ok("set_properties", json!({"ids": [id], "props": {"name": name}, "ifRev": at}));
        let next = rev(&r);
        assert!(next > at, "{r}");
        at = next;
    }
    assert_eq!(c.ok("get_node", json!({"id": id, "depth": 0}))["name"], "Two");
    assert_eq!(rev(&c.ok("get_node", json!({"id": id, "depth": 0}))), at);
}

#[test]
fn ifrev_must_be_an_integer_and_not_combine_with_documents() {
    let mut c = Client::new();
    c.err("set_properties", json!({"where": {"name": "x"}, "props": {"name": "y"}, "ifRev": "5"}));
    let e = c.err("set_properties", json!({"where": {"name": "x"}, "props": {"name": "y"}, "ifRev": 1, "documents": "all"}));
    assert!(e.contains("documents"), "{e}");
    c.err("get_node", json!({"id": 1, "ifRev": 1}));
}

#[test]
fn undo_with_a_current_ifrev_undoes_and_with_a_stale_one_leaves_the_other_agents_step() {
    let mut c = Client::new();
    let id = c.create(json!({"type": "rectangle", "name": "Box", "width": 10, "height": 10}));
    let seen = rev(&c.ok("get_node", json!({"id": id, "depth": 0})));
    c.ok("set_properties", json!({"ids": [id], "props": {"name": "Theirs"}}));
    c.err("undo", json!({"ifRev": seen}));
    assert_eq!(c.ok("get_node", json!({"id": id, "depth": 0}))["name"], "Theirs");
    let now = rev(&c.ok("get_node", json!({"id": id, "depth": 0})));
    assert_eq!(c.ok("undo", json!({"ifRev": now}))["undone"], true);
    assert_eq!(c.ok("get_node", json!({"id": id, "depth": 0}))["name"], "Box");
}
