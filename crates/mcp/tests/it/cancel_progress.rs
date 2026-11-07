//! `notifications/cancelled` for requests still queued, `notifications/progress` for long tools,
//! and the request state in `diagnostics`.

use std::io::Cursor;

use figcraft_mcp::Server;
use serde_json::{Value, json};

fn lines(msgs: &[Value]) -> Vec<u8> {
    msgs.iter().map(|m| format!("{m}\n")).collect::<String>().into_bytes()
}

fn replies(out: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(out).lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

fn call(id: u64, tool: &str, args: Value, meta: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": tool, "arguments": args, "_meta": meta}})
}

#[test]
fn a_request_cancelled_before_it_starts_is_dropped_without_a_reply() {
    let input = lines(&[
        call(1, "diagnostics", json!({}), json!({})),
        json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 2}}),
        call(2, "inspect_document", json!({}), json!({})),
        call(3, "get_selection", json!({}), json!({})),
    ]);
    let mut out = Vec::new();
    // The cancel is read before request 2 can start, whatever the timing.
    Server::headless().serve_concurrent(Cursor::new(input), &mut out).unwrap();
    let ids: Vec<Value> = replies(&out).iter().map(|r| r["id"].clone()).collect();
    assert_eq!(ids, vec![json!(1), json!(3)], "request 2 gets no reply");
}

#[test]
fn diagnostics_shows_the_request_state() {
    let input = lines(&[call(7, "get_selection", json!({}), json!({})), call(8, "diagnostics", json!({}), json!({}))]);
    let mut out = Vec::new();
    Server::headless().serve(&input[..], &mut out).unwrap();
    let r = replies(&out);
    let d: Value = serde_json::from_str(r[1]["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(d["requests"]["inFlight"], json!({"id": 8, "tool": "diagnostics"}), "{d}");
    assert_eq!(d["requests"]["lastFinished"]["tool"], "get_selection", "{d}");
    assert_eq!(d["requests"]["queued"], 0);
}

#[test]
fn long_tools_send_progress_to_a_client_that_asked() {
    let input = lines(&[
        call(1, "run_script", json!({"code": "1"}), json!({"progressToken": "tok"})),
        call(2, "run_script", json!({"code": "1"}), json!({})),
        call(3, "get_selection", json!({}), json!({"progressToken": 5})),
    ]);
    let mut out = Vec::new();
    Server::headless().serve(&input[..], &mut out).unwrap();
    let r = replies(&out);
    let kinds: Vec<(String, Value)> =
        r.iter().map(|m| (m["method"].as_str().unwrap_or("reply").to_string(), m["params"]["progress"].clone())).collect();
    assert_eq!(
        kinds,
        vec![
            ("notifications/progress".to_string(), json!(0)),
            ("notifications/progress".to_string(), json!(1)),
            ("reply".to_string(), Value::Null),
            ("reply".to_string(), Value::Null),
            ("reply".to_string(), Value::Null),
        ]
    );
    assert_eq!(r[0]["params"]["progressToken"], "tok");
    assert_eq!(r[1]["params"]["total"], 1);
}
