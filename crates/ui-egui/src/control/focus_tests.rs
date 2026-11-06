//! Over the control channel an agent's `file.open` leaves the person's tab alone, and a launch handing
//! over a link or asking for the window to come forward is answered.

use figcraft_engine::Session;
use serde_json::{Value, json};

use crate::{FigApp, NoServices};

fn handle(app: &mut FigApp, method: &str, p: Value) -> Result<Value, String> {
    super::handle(app, &egui::Context::default(), method, &p)
}

/// A saved file in a folder of its own, and the folder.
fn saved_file() -> (String, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("figcraft-focus-tests-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("other.figcraft").to_string_lossy().to_string();
    let mut s = Session::new();
    s.execute("file.save", &json!({"path": path})).unwrap();
    (path, dir)
}

#[test]
fn an_agents_open_stays_behind_while_a_person_s_open_switches() {
    let (path, dir) = saved_file();
    let mut app = FigApp::new(Session::new(), Box::new(NoServices));
    let front = app.session.docs[app.session.active].uid;
    let opened = handle(&mut app, "engine.execute", json!({"command": "file.open", "params": {"path": path}, "agent": "Claude"})).unwrap();
    assert_eq!(app.session.docs.len(), 2);
    assert_eq!(app.session.docs[app.session.active].uid, front, "the agent's file opened behind the person's tab");
    assert_eq!(opened["id"].as_u64(), Some(app.session.docs[1].uid));
    // The same call without an agent name is the person's: the new file takes the front.
    let mut person = FigApp::new(Session::new(), Box::new(NoServices));
    handle(&mut person, "engine.execute", json!({"command": "file.open", "params": {"path": path}})).unwrap();
    assert_eq!((person.session.docs.len(), person.session.active), (2, 1), "a person's open switches to the file");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_invite_link_opens_the_join_dialog_and_anything_else_is_refused() {
    let mut app = FigApp::new(Session::new(), Box::new(NoServices));
    assert_eq!(handle(&mut app, "ui.invite", json!({"link": "  figcraft://join?host=h&file=f  "})).unwrap(), json!({"ok": true}));
    assert_eq!(app.ui.live.join.as_deref(), Some("figcraft://join?host=h&file=f"));
    app.ui.live.join = None;
    assert!(handle(&mut app, "ui.invite", json!({"link": "https://example.com"})).is_err());
    assert!(handle(&mut app, "ui.invite", json!({})).is_err());
    assert!(handle(&mut app, "ui.invite", json!({"link": format!("figcraft://{}", "a".repeat(5000))})).is_err());
    assert_eq!(app.ui.live.join, None, "a refused link opens nothing");
}

#[test]
fn asking_for_the_window_to_come_forward_is_answered() {
    let mut app = FigApp::new(Session::new(), Box::new(NoServices));
    assert_eq!(handle(&mut app, "ui.focus", json!({})).unwrap(), json!({"ok": true}));
}
