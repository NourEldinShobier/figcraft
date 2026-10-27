//! The attached backend against a fake app: a loopback control server answering with a headless
//! session, speaking the desktop app's JSON-lines protocol.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;

use figcraft_mcp::{Attached, Backend, Headless, Server};
use serde_json::{Value, json};

fn token() -> figcraft_collab_net::control_auth::ControlToken {
    figcraft_collab_net::control_auth::ControlToken::new("test_control_token_with_32_chars").unwrap()
}

fn attach(port: u16) -> Attached {
    Attached::connect_with_token(&format!("127.0.0.1:{port}"), token()).unwrap()
}

fn fake_app() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let mut h = Headless::new();
        for stream in listener.incoming().flatten() {
            let mut out = stream.try_clone().unwrap();
            for line in BufReader::new(stream).lines() {
                let Ok(line) = line else { break };
                let v: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                assert_eq!(v["token"], "test_control_token_with_32_chars");
                let method = v["method"].as_str().unwrap_or("");
                let reply = match h.call(method, v["params"].clone()) {
                    Ok(r) => json!({"id": v["id"], "ok": true, "result": r}),
                    Err(e) => json!({"id": v["id"], "ok": false, "error": e}),
                };
                if writeln!(out, "{reply}").is_err() {
                    break;
                }
            }
        }
    });
    port
}

fn call(s: &mut Server, tool: &str, args: Value) -> Value {
    let line = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": tool, "arguments": args}}).to_string();
    let r: Value = serde_json::from_str(&s.handle_line(&line).unwrap()).unwrap();
    r["result"].clone()
}

#[test]
fn attached_backend_forwards_everything() {
    let port = fake_app();
    let backend = attach(port);
    assert!(backend.describe().contains(&port.to_string()));
    let mut s = Server::new(Box::new(backend));
    let r = call(&mut s, "create_node", json!({"type": "frame", "width": 64, "height": 32, "props": {"fill": "#FF0000"}}));
    assert_eq!(r["isError"], false, "{r}");
    let created: Value = serde_json::from_str(r["content"][0]["text"].as_str().unwrap()).unwrap();
    let id = created["id"].clone();
    // Render through ui.render + a temp file.
    let shot = call(&mut s, "screenshot", json!({}));
    assert_eq!(shot["content"][0]["type"], "image", "{shot}");
    let shot = call(&mut s, "screenshot", json!({"ids": [id], "scale": 2}));
    assert_eq!(shot["isError"], false, "{shot}");
    // Window capture through ui.screenshot.
    let win = call(&mut s, "screenshot", json!({"window": true}));
    assert_eq!(win["content"][0]["type"], "image", "{win}");
    let e = call(&mut s, "export", json!({"ids": [id]}));
    assert_eq!(e["content"][0]["mimeType"], "image/png");
    assert!(call(&mut s, "key", json!({"key": "Delete"}))["isError"] == false);
    let doc = call(&mut s, "inspect_document", json!({}));
    assert!(!doc["content"][0]["text"].as_str().unwrap().contains("\"Frame"));
    let err = call(&mut s, "run_command", json!({"command": "nope"}));
    assert_eq!(err["isError"], true);
}

#[test]
fn remote_destinations_are_rejected_before_connecting() {
    let result = Attached::connect_with_token("192.0.2.1:7979", token());
    let error = match result {
        Ok(_) => panic!("remote destination unexpectedly accepted"),
        Err(error) => error,
    };
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    assert!(error.to_string().contains("loopback"));
}

#[test]
fn attached_inspection_forwards_compact_options_to_the_app() {
    let mut s = Server::new(Box::new(attach(fake_app())));
    let created = call(&mut s, "create_node", json!({"type": "text", "text": "界".repeat(121), "name": "Title"}));
    let id = serde_json::from_str::<Value>(created["content"][0]["text"].as_str().unwrap()).unwrap()["id"].clone();
    let r = call(&mut s, "get_node", json!({"id": id, "outline": true}));
    assert_eq!(r["isError"], false, "{r}");
    let outline: Value = serde_json::from_str(r["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(outline["characters"], format!("{}...", "界".repeat(120)));
    assert_eq!(outline["visible"], true);
    assert!(outline.get("fontSize").is_none());
    let r = call(&mut s, "inspect_document", json!({"fields": [], "outline": false}));
    assert_eq!(r["isError"], false, "{r}");
    let compact: Value = serde_json::from_str(r["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(compact["pages"][0]["children"][0], json!({"id": id}));
    assert!(compact["pages"][0].get("name").is_none());
}

#[test]
fn attached_to_nothing_fails_cleanly() {
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    assert!(Attached::connect_with_token(&format!("127.0.0.1:{port}"), token()).is_err());
}

#[test]
fn lost_mutation_reply_is_not_replayed_and_next_call_reconnects() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = std::thread::spawn(move || {
        let mut h = Headless::new();
        let mut methods = Vec::new();
        for lose_reply in [true, false] {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap()).read_line(&mut line).unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["token"], "test_control_token_with_32_chars");
            let method = request["method"].as_str().unwrap();
            methods.push(method.to_string());
            let result = h.call(method, request["params"].clone()).unwrap();
            if !lose_reply {
                writeln!(stream, "{}", json!({"id": request["id"], "ok": true, "result": result})).unwrap();
            }
        }
        methods
    });
    let mut backend = attach(port);
    let result = backend.call("engine.execute", json!({"command": "shape.rectangle", "params": {"width": 64, "height": 32}}));
    assert!(result.is_err(), "a lost reply must not turn into a successful replay: {result:?}");
    let error = result.unwrap_err();
    assert!(error.contains("may have completed"), "{error}");
    assert!(error.contains("before retrying"), "{error}");
    let doc = backend.call("document.inspect", json!({"depth": 1})).unwrap();
    assert_eq!(doc["pages"][0]["children"].as_array().unwrap().len(), 1, "{doc}");
    assert_eq!(app.join().unwrap(), ["engine.execute", "document.inspect"]);
}

#[test]
fn a_connection_the_app_closed_while_idle_is_replaced_before_sending() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (closed_tx, closed_rx) = std::sync::mpsc::channel();
    let app = std::thread::spawn(move || {
        let mut h = Headless::new();
        let mut methods = Vec::new();
        // Each connection answers one request, then the app drops it as it does an idle one.
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap()).read_line(&mut line).unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["token"], "test_control_token_with_32_chars");
            let method = request["method"].as_str().unwrap();
            methods.push(method.to_string());
            let result = h.call(method, request["params"].clone()).unwrap();
            writeln!(stream, "{}", json!({"id": request["id"], "ok": true, "result": result})).unwrap();
            drop(stream);
            closed_tx.send(()).unwrap();
        }
        methods
    });
    let mut backend = attach(port);
    backend.call("engine.execute", json!({"command": "shape.rectangle", "params": {"width": 64, "height": 32}})).unwrap();
    closed_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));
    // No error, nothing sent twice: the request goes out once, on a fresh connection.
    let doc = backend.call("document.inspect", json!({"depth": 1})).unwrap();
    assert_eq!(doc["pages"][0]["children"].as_array().unwrap().len(), 1, "{doc}");
    assert_eq!(app.join().unwrap(), ["engine.execute", "document.inspect"]);
}

#[test]
fn reconnect_failure_reports_request_not_sent() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut backend = attach(listener.local_addr().unwrap().port());
    drop(listener);
    assert!(backend.call("document.inspect", json!({})).is_err());
    let error = backend.call("document.inspect", json!({})).unwrap_err();
    assert!(error.contains("request was not sent"), "{error}");
}
