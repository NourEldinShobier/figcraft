//! Stability audit: the commands agents use most, on hostile input (huge, deeply nested, junk
//! typed) must answer or refuse, never crash or run away.

use figcraft_doc::{NodeId, NodeKind};
use serde_json::{Value, json};

use super::*;

/// A page with `depth` frames inside each other, a text and a rectangle in the innermost one.
fn deep(depth: usize) -> Session {
    let mut s = Session::new();
    let page = s.doc().unwrap().pages[0];
    let doc = s.doc_mut().unwrap();
    let mut parent: NodeId = page;
    for i in 0..depth {
        parent = doc.create(parent, NodeKind::Frame, &format!("Frame {i}"), 1.0, 1.0, 100.0, 100.0).unwrap();
    }
    doc.create(parent, NodeKind::Rectangle, "Rect", 0.0, 0.0, 10.0, 10.0).unwrap();
    doc.create(parent, NodeKind::Text(Box::new(figcraft_doc::TextData::new("hello"))), "Text", 0.0, 0.0, 50.0, 20.0).unwrap();
    s
}

/// Run `f` on a thread with a 1 MiB stack (the Windows main-thread default), where an overflow
/// would abort the whole test process.
fn small_stack(f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new().stack_size(1 << 20).spawn(f).unwrap().join().unwrap();
}

const READS: &[(&str, &str)] = &[
    ("node.find", r#"{"limit":10}"#),
    ("node.find", r#"{"type":"text","limit":10}"#),
    ("a11y.audit", r#"{}"#),
    ("a11y.checkContrast", r#"{}"#),
    ("a11y.checkTargets", r#"{}"#),
    ("a11y.checkNonText", r#"{}"#),
    ("a11y.checkAlt", r#"{}"#),
    ("a11y.readingOrder", r#"{}"#),
    ("lint.layout", r#"{}"#),
    ("lint.tokens", r#"{}"#),
    ("ds.index", r#"{}"#),
    ("text.extract", r#"{}"#),
    ("board.read", r#"{}"#),
    ("family.list", r#"{}"#),
    ("doc.diff", r#"{}"#),
    ("inspect.overflow", r#"{}"#),
    ("script.run", r#"{"code":"find(#{limit: 5}).len()"}"#),
];

#[test]
fn reads_survive_a_deep_tree() {
    for depth in [3000usize] {
        for (cmd, params) in READS {
            let (cmd, params) = (cmd.to_string(), params.to_string());
            small_stack(move || {
                let mut s = deep(depth);
                let _ = s.execute(&cmd, &serde_json::from_str::<Value>(&params).unwrap());
            });
        }
    }
}

fn world() -> Session {
    let mut s = Session::new();
    let f = s.execute("frame.create", &json!({"x": 0, "y": 0, "width": 300, "height": 200, "name": "Card"})).unwrap();
    let f = params::id_of(&f["id"]).unwrap();
    s.execute("shape.rectangle", &json!({"x": 10, "y": 10, "width": 50, "height": 50, "parent": f.0})).ok();
    s.execute("text.create", &json!({"text": "Hello", "x": 20, "y": 80})).ok();
    s.execute("frame.create", &json!({"x": 400, "y": 0, "width": 100, "height": 100, "name": "Button"})).ok();
    s
}

/// [`world`] with its text layer open for editing.
fn world_editing() -> Session {
    let mut s = world();
    let found = s.execute("node.find", &json!({"type": "text"})).unwrap();
    let text = found["ids"][0].clone();
    s.execute("text.edit.begin", &json!({"id": text})).unwrap();
    s
}

fn nest(open: &str, close: &str, n: usize, leaf: &str) -> String {
    format!("{}{leaf}{}", open.repeat(n), close.repeat(n))
}

/// Hostile values for every parameter, and extra ones for parameters that hold a language.
fn hostile(name: &str) -> Vec<Value> {
    let mut v = vec![
        json!(null),
        json!(1e308),
        json!(-1e308),
        json!(1e15),
        json!(-1),
        json!(0),
        json!(4294967296u64),
        json!(10_000_000),
        json!(100_000),
        json!(""),
        json!("\u{0}\u{202e}\u{1F4A5}"),
        json!("x".repeat(20_000)),
        json!([1, 2, 3]),
        json!({"x": 1, "id": 2}),
        json!(true),
        // Nested as deep as a JSON parser lets a line be.
        serde_json::from_str(&nest("[", "]", 120, "1")).unwrap(),
        serde_json::from_str(&nest("{\"a\":", "}", 120, "1")).unwrap(),
        Value::Array((0..1_000u64).map(Value::from).collect()),
        Value::Array((0..200u64).map(|i| json!({"id": i, "characters": "x", "command": "select.all"})).collect()),
    ];
    let strings: Vec<String> = match name {
        "html" => vec![
            nest("<div>", "</div>", 20_000, "x"),
            nest("<div style=\"display:flex\">", "</div>", 5_000, "<p>x</p>"),
            format!("<style>{}</style><div class=a>x</div>", "a,b,c,.d>e+f~g{width:calc(1px + 2%);margin:1px 2px 3px 4px}".repeat(5_000)),
            format!("<div style=\"{}\">x</div>", "width:calc(".repeat(5_000)),
            format!("<table>{}</table>", "<tr><td>x<td>y".repeat(20_000)),
            "<div style=\"display:grid;grid-template-columns:repeat(99999999,1fr)\"><p>a</p><p>b</p></div>".into(),
            "<div style=\"width:99999999999px;height:99999999999px\"><img width=99999999 height=99999999 src=x></div>".into(),
            "<svg><path d=\"M0 0 L1e308 1e308\"/></svg>".repeat(2_000),
            format!("<p>{}", "e\u{301}".repeat(500_000)),
        ],
        "mermaid" => vec![
            format!("graph TD\n{}", (0..5_000).map(|i| format!("A{i} --> A{}\n", i + 1)).collect::<String>()),
            format!("graph TD\n{}x\n{}", "subgraph s\n".repeat(5_000), "end\n".repeat(5_000)),
            format!("sequenceDiagram\n{}", "A->>B: hello\n".repeat(9_000)),
            format!("erDiagram\n{}", (0..3_000).map(|i| format!("T{i} ||--o{{ T{} : has\n", i + 1)).collect::<String>()),
            format!("graph LR\nA[{}]\n", "(".repeat(50_000)),
            format!("graph TD\nA --> B\n{}", "A --> B\n".repeat(12_000)),
            "graph TD\nA[\"x\"] -->|".repeat(5_000),
        ],
        "code" => vec![
            nest("(", ")", 5_000, "1"),
            nest("[", "]", 5_000, "1"),
            nest("#{a: ", "}", 5_000, "1"),
            "loop { }".into(),
            "let a = []; loop { a.push(a); }".into(),
            "let s = \"x\"; loop { s += s; }".into(),
            "fn f(n) { f(n + 1) } f(0)".into(),
            "let i = 0; while i < 1000000000 { cmd(\"select.all\"); i += 1; }".into(),
            "let m = #{}; for i in 0..100000 { m[`k${i}`] = i; } m.len()".into(),
            format!("let x = {}1{};", "if true { ".repeat(3_000), " }".repeat(3_000)),
            "for i in 0..100000 { cmd(\"shape.rectangle\", #{x: i, y: 0, width: 1, height: 1}); }".into(),
        ],
        "nameRegex" | "find" | "regex" | "query" | "pattern" | "text" | "name" | "filter" | "match" => vec![
            "(a*)*b".into(),
            "(((((((((((a{1000}){1000}){1000}){1000}){1000}){1000}){1000}){1000}){1000}){1000}){1000}".into(),
            nest("(", ")", 10_000, "a"),
            "[".repeat(10_000),
            r"\p{L}{1000}{1000}".into(),
            format!("(?i){}", "a|".repeat(100_000)),
            ".*".repeat(50_000),
        ],
        "characters" => vec!["x ".repeat(500_000), "\n".repeat(200_000), "a\u{301}".repeat(200_000)],
        "css" | "style" => vec!["a{".repeat(100_000), "@media ".repeat(10_000)],
        _ => vec![],
    };
    v.extend(strings.into_iter().map(Value::from));
    if matches!(name, "where" | "list") {
        v.extend([
            json!({"nameRegex": "(a*)*b"}),
            json!({"nameRegex": nest("(", ")", 5_000, "a")}),
            json!({"limit": 1e308}),
            json!({"limit": 10_000_000, "type": "rectangle"}),
            json!({"name": "x".repeat(10_000), "text": "y"}),
            json!({"root": 99999999, "page": "all"}),
        ]);
    }
    v
}

#[test]
fn every_param_survives_hostile_values() {
    let skip_cmd =
        |id: &str| id.starts_with("file.") || id.starts_with("collab.") || id.starts_with("export.") || id == "text.rescanFonts" || id == "doc.open";
    let skip_param =
        |n: &str| matches!(n, "path" | "paths" | "dir" | "directory" | "file" | "url" | "server" | "link" | "bind" | "port" | "sheet" | "output");
    let mut slow: Vec<String> = Vec::new();
    for (spec, editing) in all_commands().iter().flat_map(|c| [(c, false), (c, true)]) {
        if skip_cmd(spec.id) || (editing && !spec.id.starts_with("text.")) {
            continue;
        }
        let Some(names) = crate::cmd::param_names(spec.id) else { continue };
        for name in names.iter().copied().chain(["ids", "where"]) {
            if skip_param(name) {
                continue;
            }
            for value in hostile(name) {
                let mut s = if editing { world_editing() } else { world() };
                let params = json!({ name: value.clone() });
                let t = std::time::Instant::now();
                let r = s.execute(spec.id, &params);
                let took = t.elapsed();
                if let Err(EngineError::Internal { cmd, msg }) = &r {
                    panic!("{cmd} panicked on {name}={:.200} (editing text: {editing}): {msg}", value.to_string());
                }
                if took.as_secs() >= 10 {
                    slow.push(format!("{} {name}={:.60} took {took:?}", spec.id, value.to_string()));
                }
            }
        }
    }
    assert!(slow.is_empty(), "left: {}", slow.join("\nleft: "));
}

/// One frame with `n` overlapping children of mixed kinds.
fn wide(n: usize) -> Session {
    let mut s = Session::new();
    let page = s.doc().unwrap().pages[0];
    let doc = s.doc_mut().unwrap();
    let f = doc.create(page, NodeKind::Frame, "Wide", 0.0, 0.0, 1000.0, 1000.0).unwrap();
    for i in 0..n {
        let (x, y) = ((i % 50) as f64 * 3.0, (i / 50 % 50) as f64 * 3.0);
        match i % 3 {
            0 => doc.create(f, NodeKind::Rectangle, &format!("Button {i}"), x, y, 40.0, 20.0),
            1 => doc.create(f, NodeKind::Text(Box::new(figcraft_doc::TextData::new("hello world"))), &format!("Label {i}"), x, y, 80.0, 20.0),
            _ => doc.create(f, NodeKind::Frame, &format!("Card {i}"), x, y, 60.0, 60.0),
        }
        .unwrap();
    }
    s
}

#[test]
fn reads_stay_fast_on_a_wide_file() {
    let mut slow = Vec::new();
    for (cmd, params) in READS {
        let mut s = wide(20_000);
        let t = std::time::Instant::now();
        let _ = s.execute(cmd, &serde_json::from_str::<Value>(params).unwrap());
        if t.elapsed().as_secs() >= 10 {
            slow.push(format!("{cmd} {params} took {:?}", t.elapsed()));
        }
    }
    assert!(slow.is_empty(), "left: {}", slow.join("\nleft: "));
}

/// What an agent needs in the text of a refusal to fix its next call.
#[test]
fn refusals_say_what_to_do_next() {
    let cases: &[(&str, Value, &[&str])] = &[
        ("fill.remove", json!({"index": -1}), &["index", "-1", "0"]),
        ("a11y.fixContrast", json!({"root": 99999999}), &["99999999"]),
        ("text.edit.insert", json!({"text": "x"}), &["text.edit.begin"]),
        ("command.each", json!({"command": "zz"}), &["list_commands"]),
        ("annotation.add", json!({"id": 99999999}), &["99999999"]),
        ("effect.add", json!({"effect": "zz"}), &["dropShadow", "type"]),
        ("grid.add", json!({"grid": "zz"}), &["object", "list_commands"]),
        ("page.delete", json!({}), &["add another page"]),
        ("variable.import", json!({"variables": "zz"}), &["path", "JSON"]),
    ];
    for (cmd, params, wanted) in cases {
        let mut s = world();
        let msg = s.execute(cmd, params).expect_err(cmd).to_string();
        for w in *wanted {
            assert!(msg.contains(w), "{cmd} {params}: `{w}` missing from: {msg}");
        }
    }
}

#[test]
fn the_command_log_does_not_keep_big_parameters() {
    let mut s = Session::new();
    let code = format!("1 // {}", "x".repeat(60_000));
    for _ in 0..3 {
        s.execute("script.run", &json!({"code": code})).unwrap();
    }
    s.execute("shape.rectangle", &json!({"x": 1, "y": 2, "width": 3, "height": 4})).unwrap();
    let kept: usize = s.journal.iter().map(|(_, p)| p.to_string().len()).sum();
    assert!(kept < 20_000, "the log holds {kept} bytes of parameters");
    // Small parameters are still logged as given.
    assert_eq!(s.journal.last().map(|(c, p)| (c.as_str(), p["width"].clone())), Some(("shape.rectangle", json!(3))));
}
