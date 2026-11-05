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
