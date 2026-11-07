//! `import_html`: static HTML and CSS to layers, from a string or from a file inside the read roots.

use std::path::PathBuf;

use crate::common::Client;
use figcraft_engine::file_access::Roots;
use figcraft_mcp::{Server, TOOL_NAMES};
use serde_json::{Value, json};

const PAGE: &str = "<style>.row{display:flex;gap:16px;padding:24px;background:#ffffff}.card{flex:1;padding:16px;border:1px solid #dddddd;border-radius:8px}</style>\
<div class=row><div class=card>One</div><div class=card>Two</div></div><video src=x.mp4></video>";

#[test]
fn a_page_becomes_layers_and_unsupported_tags_are_listed() {
    assert!(TOOL_NAMES.contains(&"import_html"));
    assert!(include_str!("../../../../docs/mcp.md").contains("| `import_html {"));
    let mut c = Client::new();
    let r = c.ok("import_html", json!({"html": PAGE, "width": 800}));
    assert!(r["created"].as_u64().unwrap() >= 5, "{r}");
    assert!(r["warnings"][0].as_str().unwrap().contains("<video>"), "{r}");
    let row = c.ok("find_nodes", json!({"name": "div.row"}));
    assert_eq!(row["total"], 1, "{row}");
    let node = c.ok("get_node", json!({"id": row["ids"][0], "depth": 0}));
    assert_eq!((node["layout"]["mode"].clone(), node["layout"]["gap"].clone()), (json!("horizontal"), json!(16.0)), "{node}");
    let cards = c.ok("find_nodes", json!({"name": "div.card"}));
    assert_eq!(cards["total"], 2, "{cards}");
    c.ok("undo", json!({}));
    assert_eq!(c.ok("find_nodes", json!({"name": "div.card"}))["total"], 0, "one undo takes the import back");
}

#[test]
fn dry_run_reports_and_changes_nothing() {
    let mut c = Client::new();
    let r = c.ok("import_html", json!({"html": PAGE, "dryRun": true}));
    assert_eq!(r["dryRun"], true, "{r}");
    assert_eq!(c.ok("find_nodes", json!({"name": "div.card"}))["total"], 0);
}

#[test]
fn junk_arguments_are_errors() {
    let mut c = Client::new();
    for args in [
        json!({}),
        json!({"html": ""}),
        json!({"html": 7}),
        json!({"html": "<p>x</p>", "path": "x.html"}),
        json!({"html": "<p>x</p>", "width": 0}),
        json!({"html": "<p>x</p>", "width": 99999}),
        json!({"html": "<p>x</p>", "parent": 424242}),
        json!({"path": "does-not-exist.html"}),
    ] {
        c.err("import_html", args);
    }
}

struct Dirs(PathBuf);

impl Dirs {
    fn new() -> Self {
        let d = std::env::temp_dir().join(format!("figcraft-mcp-import-html-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for sub in ["in", "outside"] {
            std::fs::create_dir_all(d.join(sub)).unwrap();
        }
        Self(d)
    }
    fn p(&self, rel: &str) -> String {
        self.0.join(rel).to_string_lossy().into_owned()
    }
}

impl Drop for Dirs {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn call(s: &mut Server, args: Value) -> Value {
    let line = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "import_html", "arguments": args}}).to_string();
    let r: Value = serde_json::from_str(&s.handle_line(&line).unwrap()).unwrap();
    r["result"].clone()
}

#[test]
fn a_path_is_read_only_inside_the_read_roots() {
    let d = Dirs::new();
    std::fs::write(d.0.join("in/page.html"), PAGE).unwrap();
    std::fs::write(d.0.join("outside/page.html"), PAGE).unwrap();
    let mut s = Server::headless().with_roots(Some(Roots::new(&[d.p("in")], &[]).unwrap()));
    let ok = call(&mut s, json!({"path": d.p("in/page.html")}));
    assert_eq!(ok["isError"], false, "{ok}");
    let no = call(&mut s, json!({"path": d.p("outside/page.html")}));
    assert_eq!(no["isError"], true, "{no}");
    assert!(no["content"][0]["text"].as_str().unwrap().contains("automation path rejected"), "{no}");
    let no = call(&mut s, json!({"path": d.p("in/../outside/page.html")}));
    assert_eq!(no["isError"], true, "{no}");
}
