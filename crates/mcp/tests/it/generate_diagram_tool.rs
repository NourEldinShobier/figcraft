//! `generate_diagram`: Mermaid text drawn on a board as attached shapes and connectors.

use crate::common::Client;
use figcraft_mcp::TOOL_NAMES;
use serde_json::json;

#[test]
fn a_flowchart_is_drawn_on_a_board_and_undone_in_one_step() {
    assert!(TOOL_NAMES.contains(&"generate_diagram"));
    assert!(include_str!("../../../../docs/mcp.md").contains("| `generate_diagram {"));
    let mut c = Client::new();
    c.ok("run_command", json!({"command": "file.newBoard", "params": {"name": "Ideas"}}));
    let r = c.ok("generate_diagram", json!({"mermaid": "flowchart LR\n  A[Start] -->|go| B{Ok?}\n  B -->|no| A\n  B --> C((End))"}));
    assert_eq!(r["nodes"].as_array().unwrap().len(), 3, "{r}");
    assert_eq!(r["edges"].as_array().unwrap().len(), 3, "{r}");
    let found = c.ok("find_nodes", json!({"name": "Diagram"}));
    assert_eq!(found["total"], 1, "{found}");
    c.ok("undo", json!({}));
    assert_eq!(c.ok("find_nodes", json!({"name": "Diagram"}))["total"], 0, "one undo takes the diagram back");
}

#[test]
fn errors_name_the_line_and_junk_is_refused() {
    let mut c = Client::new();
    c.ok("run_command", json!({"command": "file.newBoard", "params": {}}));
    for args in [
        json!({}),
        json!({"mermaid": "graph TD
A-->B
A --x B"}),
        json!({"mermaid": "gantt
 a"}),
        json!({"mermaid": "graph TD
A-->B", "direction": "sideways"}),
        json!({"mermaid": 5}),
    ] {
        c.err("generate_diagram", args);
    }
    let e = c.err(
        "generate_diagram",
        json!({"mermaid": "graph TD
A-->B
A --x B"}),
    );
    assert!(e.contains("line 3"), "{e}");
}
