//! A detached copy beside an open app says so in its tool results.

use std::net::TcpListener;

use serde_json::json;

use super::*;
use crate::call_tool;

/// A port nothing listens on.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").and_then(|l| l.local_addr()).map(|a| a.port()).unwrap_or(1)
}

fn texts(r: &crate::ToolResult) -> Vec<String> {
    r.content.iter().filter_map(|c| c["text"].as_str().map(str::to_string)).collect()
}

#[test]
fn no_app_means_no_warning() {
    let mut auto = Auto::with_port(free_port());
    assert_eq!(auto.detached(), None);
    let r = call_tool(&mut auto, "inspect_ui", &json!({}));
    assert_eq!(texts(&r).len(), 1, "{:?}", texts(&r));
}

#[test]
fn a_listening_app_makes_every_result_carry_the_warning() {
    let app = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = app.local_addr().unwrap().port();
    // Nothing answers the control protocol here, and there is no token for this port: the server
    // stays on its headless copy.
    let mut auto = Auto::with_port(port);
    let r = call_tool(&mut auto, "set_properties", &json!({"ids": [], "props": {"name": "x"}}));
    let warning = texts(&r).into_iter().find(|t| t.starts_with("NOT LIVE")).unwrap_or_default();
    assert!(warning.contains(&format!("port {port}")) && warning.contains("headless copy"), "{:?}", texts(&r));
}

#[test]
fn unsaved_edits_say_how_to_get_attached() {
    let app = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut auto = Auto::with_port(app.local_addr().unwrap().port());
    call_tool(&mut auto, "create_node", &json!({"type": "rectangle", "width": 10, "height": 10}));
    let warning = auto.detached().unwrap_or_default();
    assert!(warning.contains("unsaved edits") && warning.contains("save_file"), "{warning}");
}

#[test]
fn inspect_ui_names_the_server() {
    let mut auto = Auto::with_port(free_port());
    let r = call_tool(&mut auto, "inspect_ui", &json!({}));
    let v: serde_json::Value = serde_json::from_str(&texts(&r)[0]).unwrap();
    assert_eq!(v["mcpServer"]["pid"], json!(std::process::id()));
    assert!(v["mcpServer"]["backend"].as_str().unwrap_or("").starts_with("headless"));
}

#[test]
fn the_port_variable_picks_the_control_port() {
    assert_eq!(port_from_env(None), 7979);
    assert_eq!(port_from_env(Some("8123")), 8123);
    assert_eq!(port_from_env(Some(" 65535 ")), 65535);
    for bad in ["", "0", "65536", "-1", "seven", "80.5"] {
        assert_eq!(port_from_env(Some(bad)), 7979, "{bad:?}");
    }
}

/// A fake app on a loopback port: remembers every `params` it is sent and answers `pref.autoSave`
/// like the engine (`{on}`; `on` sets it), everything else with `{}`.
fn fake_app(auto_save: bool) -> (u16, std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>) {
    use std::io::{BufRead, BufReader, Write};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        let mut on = auto_save;
        for stream in listener.incoming().flatten() {
            let mut out = stream.try_clone().unwrap();
            for line in BufReader::new(stream).lines() {
                let Ok(line) = line else { break };
                let v: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
                log.lock().unwrap().push(v["params"].clone());
                let mut result = json!({});
                if v["params"]["command"] == "pref.autoSave" {
                    if let Some(x) = v["params"]["params"]["on"].as_bool() {
                        on = x;
                    }
                    result = json!({"on": on});
                }
                if writeln!(out, "{}", json!({"id": v["id"], "ok": true, "result": result})).is_err() {
                    break;
                }
            }
        }
    });
    (port, seen)
}

/// An `Auto` attached to the fake app, as after `look`.
fn attached_auto(port: u16) -> Auto {
    let token = figcraft_collab_net::control_auth::ControlToken::new("test_control_token_with_32_chars").unwrap();
    let app = Attached::connect_with_token(&format!("127.0.0.1:{port}"), token).unwrap();
    Auto {
        headless: Headless::new(),
        attached: Some(app),
        next_look: Instant::now() + LOOK_EVERY,
        port,
        app_listening: true,
        target: Some(json!(3)),
        note: None,
    }
}

#[test]
fn attaching_turns_auto_save_on_and_the_next_result_says_so_once() {
    let (port, seen) = fake_app(false);
    let mut auto = attached_auto(port);
    auto.on_attach();
    assert_eq!(auto.target(), None, "ids belong to one app run");
    let sent: Vec<_> = seen.lock().unwrap().iter().map(|p| (p["command"].clone(), p["params"].clone())).collect();
    assert_eq!(sent, [(json!("pref.autoSave"), json!({})), (json!("pref.autoSave"), json!({"on": true}))]);
    let first = texts(&call_tool(&mut auto, "inspect_ui", &json!({})));
    assert_eq!(first.len(), 2, "{first:?}");
    assert_eq!(first[1], "auto-save was off: the MCP server turned it on, so the agent's changes reach the file");
    assert_eq!(texts(&call_tool(&mut auto, "inspect_ui", &json!({}))).len(), 1, "said once per attach");
}

#[test]
fn auto_save_that_is_already_on_is_left_alone_and_not_mentioned() {
    let (port, seen) = fake_app(true);
    let mut auto = attached_auto(port);
    auto.on_attach();
    assert_eq!(seen.lock().unwrap().len(), 1, "only the question was sent");
    assert_eq!(texts(&call_tool(&mut auto, "inspect_ui", &json!({}))).len(), 1);
}

#[test]
fn the_guard_runs_unless_the_variable_is_1() {
    assert!(autosave_guard(None));
    for value in ["", "0", "yes", "true"] {
        assert!(autosave_guard(Some(value)), "{value:?}");
    }
    for value in ["1", " 1 "] {
        assert!(!autosave_guard(Some(value)), "{value:?}");
    }
}
