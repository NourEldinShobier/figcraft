//! Every argument of every tool, given hostile values, gets an answer: no panic (reported as an
//! "internal error" reply) and no run-away call.

use crate::common::Client;
use figcraft_mcp::tool_definitions;
use serde_json::{Value, json};

fn nest(open: &str, close: &str, n: usize, leaf: &str) -> String {
    format!("{}{leaf}{}", open.repeat(n), close.repeat(n))
}

fn hostile(prop: &str) -> Vec<Value> {
    let mut v = vec![
        json!(null),
        json!(1e308),
        json!(-1e308),
        json!(-1),
        json!(0),
        json!(10_000_000),
        json!(4294967296u64),
        json!(""),
        json!("\u{0}\u{202e}\u{1F4A5}"),
        json!("x".repeat(20_000)),
        json!([1, 2, 3]),
        json!({"x": 1, "id": 2}),
        json!(true),
        serde_json::from_str(&nest("[", "]", 100, "1")).unwrap(),
        serde_json::from_str(&nest("{\"a\":", "}", 100, "1")).unwrap(),
        Value::Array((0..3_000u64).map(Value::from).collect()),
    ];
    let text: Vec<String> = match prop {
        "html" => vec![nest("<div>", "</div>", 20_000, "x"), format!("<div style=\"{}\">x</div>", "width:calc(".repeat(3_000))],
        "mermaid" | "source" => vec![
            format!("graph TD\n{}x\n{}", "subgraph s\n".repeat(3_000), "end\n".repeat(3_000)),
            format!("graph TD\n{}", (0..3_000).map(|i| format!("A{i} --> A{}\n", i + 1)).collect::<String>()),
        ],
        "code" => vec![nest("(", ")", 3_000, "1"), "loop { }".into(), "let s = \"x\"; loop { s += s; }".into()],
        "nameRegex" | "name" | "text" | "query" | "find" => vec!["(a*)*b".into(), nest("(", ")", 5_000, "a"), "[".repeat(5_000)],
        _ => vec![],
    };
    v.extend(text.into_iter().map(Value::from));
    v
}

/// Arguments that name a file: the tools would write or read it.
fn names_a_file(prop: &str) -> bool {
    matches!(prop, "path" | "paths" | "dir" | "directory" | "file" | "root" | "output" | "sheet" | "url" | "variables")
}

#[test]
fn no_argument_value_panics_or_hangs_a_tool() {
    let (mut bad, mut calls) = (Vec::new(), 0usize);
    for def in tool_definitions() {
        let Some(name) = def["name"].as_str() else { continue };
        let Some(props) = def["inputSchema"]["properties"].as_object() else { continue };
        for prop in props.keys().filter(|p| !names_a_file(p)) {
            for value in hostile(prop) {
                let mut c = Client::new();
                c.create(json!({"type": "frame", "width": 100, "height": 100}));
                c.create(json!({"type": "text", "text": "hi"}));
                calls += 1;
                let started = std::time::Instant::now();
                let reply = c.rpc("tools/call", json!({"name": name, "arguments": {prop: value.clone()}}));
                let text = reply.to_string();
                let shown: String = value.to_string().chars().take(60).collect();
                if text.contains("internal error in") {
                    bad.push(format!("{name}.{prop}={shown}: {}", text.chars().take(200).collect::<String>()));
                }
                if started.elapsed().as_secs() >= 10 {
                    bad.push(format!("{name}.{prop}={shown} took {:?}", started.elapsed()));
                }
            }
        }
    }
    assert!(calls > 1000, "left: only {calls} calls were made");
    assert!(bad.is_empty(), "left: {}", bad.join("\nleft: "));
}
