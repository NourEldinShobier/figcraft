//! `file.open {activate: false}`: the file is open afterwards, the tab the person looks at stays in
//! front, and the result names the tab (`id`) for the `document` param.

use base64::Engine as _;
use serde_json::{Value, json};

use crate::Session;

/// A saved `.figcraft` file called `name` in a folder of its own.
fn saved(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("figcraft-open-background-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.figcraft")).to_string_lossy().to_string();
    let mut s = Session::new();
    s.execute("file.rename", &json!({"name": name})).unwrap();
    s.execute("file.save", &json!({"path": path})).unwrap();
    path
}

fn list(s: &mut Session) -> Vec<Value> {
    s.execute("doc.list", &json!({})).unwrap().as_array().unwrap().clone()
}

/// The `id` of the tab in front, as `doc.list` reports it.
fn front(s: &mut Session) -> Value {
    list(s).into_iter().find(|d| d["active"] == true).unwrap()["id"].clone()
}

fn open(s: &mut Session, path: &str, activate: Option<bool>) -> Value {
    let mut p = json!({"path": path});
    if let Some(a) = activate {
        p["activate"] = json!(a);
    }
    s.execute("file.open", &p).unwrap()
}

#[test]
fn a_new_file_opens_behind_the_tab_in_front() {
    let a = saved("new-behind");
    let mut s = Session::new();
    let before = front(&mut s);
    let r = open(&mut s, &a, Some(false));
    let l = list(&mut s);
    assert_eq!(l.len(), 2);
    assert_eq!(front(&mut s), before, "the first tab stays in front");
    assert_eq!((r["index"].clone(), r["id"].clone()), (json!(1), l[1]["id"].clone()));
    assert!(r["summary"].is_object(), "{r}");
    // The id addresses the file without bringing it forward.
    assert_eq!(s.document_uid(&r["id"]).unwrap(), s.docs[1].uid);
    let _ = std::fs::remove_dir_all(std::path::Path::new(&a).parent().unwrap());
}

#[test]
fn a_file_that_is_open_stays_where_it_is() {
    let a = saved("open-stays");
    let mut s = Session::new();
    let first = open(&mut s, &a, None);
    assert_eq!(front(&mut s), first["id"], "opened normally: in front");
    s.execute("doc.switch", &json!({"index": 0})).unwrap();
    let before = front(&mut s);
    assert_ne!(before, first["id"]);
    let again = open(&mut s, &a, Some(false));
    assert_eq!(front(&mut s), before, "not switched to the open file");
    assert_eq!((again["index"].clone(), again["id"].clone()), (json!(1), first["id"].clone()));
    assert_eq!(list(&mut s).len(), 2, "no second copy");
    // Reading it again keeps the tab in front too, and says which tab holds the file now.
    let reloaded = s.execute("file.open", &json!({"path": a, "activate": false, "reload": true})).unwrap();
    assert_eq!(reloaded["reloaded"], true);
    assert_eq!(front(&mut s), before);
    assert_eq!(reloaded["id"], list(&mut s)[1]["id"]);
    let _ = std::fs::remove_dir_all(std::path::Path::new(&a).parent().unwrap());
}

#[test]
fn opening_without_activate_still_switches_and_names_the_tab() {
    let (a, b) = (saved("default-a"), saved("default-b"));
    let mut s = Session::new();
    let r = open(&mut s, &a, None);
    assert_eq!(front(&mut s), r["id"], "a new file takes the front");
    open(&mut s, &b, Some(true));
    s.execute("doc.switch", &json!({"index": 0})).unwrap();
    let again = open(&mut s, &a, None);
    assert_eq!(front(&mut s), again["id"], "so does one that is open");
    assert_eq!(again["id"], r["id"]);
    for p in [&a, &b] {
        let _ = std::fs::remove_dir_all(std::path::Path::new(p).parent().unwrap());
    }
}

#[test]
fn data_opens_behind_too_and_a_session_without_tabs_shows_the_file() {
    let a = saved("data-behind");
    let data = base64::engine::general_purpose::STANDARD.encode(std::fs::read(&a).unwrap());
    let mut s = Session::new();
    let before = front(&mut s);
    let r = s.execute("file.open", &json!({"data": data, "name": "dropped.figcraft", "activate": false})).unwrap();
    assert_eq!(front(&mut s), before);
    assert_eq!(r["id"], list(&mut s)[1]["id"]);
    // Nothing to stay in front: the only tab is the one shown.
    let mut empty = Session::empty();
    let r = open(&mut empty, &a, Some(false));
    assert_eq!(front(&mut empty), r["id"]);
    let _ = std::fs::remove_dir_all(std::path::Path::new(&a).parent().unwrap());
}

/// `file.open` as the control channel runs it for an AI agent (`engine.execute {agent}`, and every
/// MCP call): `Session::execute_as` with the client's name.
fn open_as_agent(s: &mut Session, path: &str, activate: Option<bool>) -> Value {
    let mut p = json!({"path": path});
    if let Some(a) = activate {
        p["activate"] = json!(a);
    }
    s.execute_as(Some("Claude"), "file.open", &p).unwrap()
}

#[test]
fn an_agent_opens_behind_the_tab_in_front_unless_it_asks_otherwise() {
    let (a, b) = (saved("agent-a"), saved("agent-b"));
    let mut s = Session::new();
    let before = front(&mut s);
    let r = open_as_agent(&mut s, &a, None);
    assert_eq!(list(&mut s).len(), 2);
    assert_eq!(front(&mut s), before, "no `activate`: the person's tab stays in front");
    assert_eq!(r["id"], list(&mut s)[1]["id"], "the result still names the new tab");
    // An explicit `activate` wins, either way.
    let r = open_as_agent(&mut s, &b, Some(true));
    assert_eq!(front(&mut s), r["id"]);
    s.execute("doc.switch", &json!({"index": 0})).unwrap();
    assert_eq!(front(&mut s), before);
    open_as_agent(&mut s, &a, Some(true));
    assert_eq!(front(&mut s), list(&mut s)[1]["id"], "asked to: the open file comes forward");
    for p in [&a, &b] {
        let _ = std::fs::remove_dir_all(std::path::Path::new(p).parent().unwrap());
    }
}

#[test]
fn an_agent_finding_an_open_file_leaves_the_tab_in_front_and_the_next_person_call_switches() {
    let (a, b) = (saved("agent-open-a"), saved("agent-open-b"));
    let mut s = Session::new();
    open(&mut s, &a, Some(false));
    let before = front(&mut s);
    open_as_agent(&mut s, &a, None);
    assert_eq!(front(&mut s), before, "the file is open already: still not switched to");
    assert_eq!(list(&mut s).len(), 2, "no second copy");
    // The agent's mark ends with its call: a person's call switches again.
    assert!(!s.is_agent_call());
    let again = open(&mut s, &b, None);
    assert_eq!(front(&mut s), again["id"]);
    for p in [&a, &b] {
        let _ = std::fs::remove_dir_all(std::path::Path::new(p).parent().unwrap());
    }
}

#[test]
fn an_agent_that_gave_no_name_is_an_agent_too() {
    let a = saved("agent-unnamed");
    let mut s = Session::new();
    let before = front(&mut s);
    s.execute_as(Some(""), "file.open", &json!({"path": a})).unwrap();
    assert_eq!(list(&mut s).len(), 2);
    assert_eq!(front(&mut s), before, "an agent with no name is an agent too");
    let _ = std::fs::remove_dir_all(std::path::Path::new(&a).parent().unwrap());
}
