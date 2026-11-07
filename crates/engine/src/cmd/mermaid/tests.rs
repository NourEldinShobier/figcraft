use figcraft_doc::{BoardItem, BoardShape, End, NodeId, NodeKind};
use serde_json::{Value, json};

use crate::Session;
use crate::board_tests::{board, connector, id, node};

fn draw(s: &mut Session, src: &str) -> Value {
    s.execute("board.fromMermaid", &json!({"mermaid": src})).unwrap_or_else(|e| panic!("{e}"))
}

fn err(s: &mut Session, src: &str) -> String {
    s.execute("board.fromMermaid", &json!({"mermaid": src})).unwrap_err().to_string()
}

fn node_id(r: &Value, name: &str) -> NodeId {
    let n = r["nodes"].as_array().unwrap().iter().find(|n| n["name"] == name).unwrap_or_else(|| panic!("no node {name} in {r}"));
    id(n, "id")
}

fn pos(s: &Session, nid: NodeId) -> (f64, f64) {
    let b = s.doc().unwrap().world_bounds(nid).unwrap();
    (b.x0, b.y0)
}

/// The text of a shape's centred text layer.
fn shape_text(s: &Session, shape: NodeId) -> String {
    let doc = s.doc().unwrap();
    let t = crate::board::text_of(doc, shape).expect("a shape with text");
    match &doc.node(t).unwrap().kind {
        NodeKind::Text(td) => td.characters.clone(),
        _ => panic!("not text"),
    }
}

/// The text of connector `c`'s label.
fn label(s: &Session, c: NodeId) -> Option<String> {
    let doc = s.doc().unwrap();
    let l = crate::board::label_of(doc, c)?;
    match &doc.node(crate::board::text_of(doc, l)?)?.kind {
        NodeKind::Text(td) => Some(td.characters.clone()),
        _ => None,
    }
}

fn page_children(s: &Session) -> usize {
    s.doc().unwrap().node(s.page().unwrap()).unwrap().children.len()
}

const FLOW: &str = "flowchart TD
  A[Start] --> B{Valid?}
  B -->|yes| C(Home)
  B -- no --> D[(Users)]
  D -.-> A
  C ==> E((Done))
  E --- A";

#[test]
fn a_flowchart_becomes_shapes_and_attached_connectors() {
    let mut s = board();
    let r = draw(&mut s, FLOW);
    assert_eq!(r["nodes"].as_array().unwrap().len(), 5, "{r}");
    assert_eq!(r["edges"].as_array().unwrap().len(), 6, "{r}");
    let (a, b, c, d) = (node_id(&r, "A"), node_id(&r, "B"), node_id(&r, "C"), node_id(&r, "D"));
    assert!(pos(&s, b).1 > pos(&s, a).1, "B is below A");
    let mid = |n: NodeId| s.doc().unwrap().world_bounds(n).unwrap().center().y;
    assert!(mid(c) > mid(b) && (mid(c) - mid(d)).abs() < 1.0, "the two branches share a row");
    assert_ne!(pos(&s, c).0, pos(&s, d).0);
    let e1 = connector(&s, id(&r["edges"][1], "id"));
    assert!(matches!(e1.from, End::Node { node, .. } if node == b) && matches!(e1.to, End::Node { node, .. } if node == c), "{e1:?}");
    assert_eq!(label(&s, id(&r["edges"][1], "id")).as_deref(), Some("yes"));
    assert_eq!(label(&s, id(&r["edges"][2], "id")).as_deref(), Some("no"));
    let dotted = node(&s, id(&r["edges"][3], "id"));
    assert!(!dotted.stroke.dashes.is_empty(), "-.-> is dashed");
    let open = node(&s, id(&r["edges"][5], "id"));
    assert_eq!(open.stroke.cap_end, figcraft_doc::StrokeCap::None, "--- has no arrow");
    // The shapes' kinds and the text.
    assert_eq!(shape_text(&s, b), "Valid?");
    assert!(matches!(node(&s, b).board, Some(BoardItem::Shape { shape: BoardShape::Diamond })));
    // A moved node drags its connector along.
    s.execute("node.set", &json!({"ids": [a], "props": {"x": 900}})).unwrap();
    let a_now = pos(&s, a);
    assert!(a_now.0 > 800.0);
}

#[test]
fn the_whole_diagram_is_one_undo_step() {
    let mut s = board();
    let before = page_children(&s);
    let r = draw(&mut s, FLOW);
    assert!(page_children(&s) >= before + 7, "a section and six connectors (and labels) beside it: {r}");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(page_children(&s), before);
    let dry = s.execute("board.fromMermaid", &json!({"mermaid": FLOW, "dryRun": true})).unwrap();
    assert_eq!(dry["dryRun"], true, "{dry}");
    assert_eq!(page_children(&s), before);
}

#[test]
fn direction_decides_the_axis() {
    let mut s = board();
    let r = s.execute("board.fromMermaid", &json!({"mermaid": "graph TD\nA-->B", "direction": "LR"})).unwrap();
    let (a, b) = (pos(&s, node_id(&r, "A")), pos(&s, node_id(&r, "B")));
    assert!(b.0 > a.0, "{a:?} {b:?}");
    let r = draw(&mut s, "flowchart RL\nA-->B");
    assert!(pos(&s, node_id(&r, "B")).0 < pos(&s, node_id(&r, "A")).0, "RL runs right to left");
    let r = draw(&mut s, "flowchart BT\nA-->B");
    assert!(pos(&s, node_id(&r, "B")).1 < pos(&s, node_id(&r, "A")).1, "BT runs bottom to top");
}

#[test]
fn a_cycle_and_a_self_loop_do_not_break_the_layout() {
    let mut s = board();
    let r = draw(&mut s, "graph LR\nA-->B\nB-->C\nC-->A\nA-->A");
    assert_eq!(r["edges"].as_array().unwrap().len(), 3, "{r}");
    assert!(r["warnings"].to_string().contains("itself"), "{r}");
}

#[test]
fn subgraphs_are_nested_sections_around_their_members() {
    let mut s = board();
    let r = draw(
        &mut s,
        "flowchart LR
  subgraph one [Front]
    A --> B
    subgraph inner
      C
    end
  end
  subgraph two
    D
  end
  B --> D
  A --> C
  C --> D",
    );
    let (a, c, d) = (node_id(&r, "A"), node_id(&r, "C"), node_id(&r, "D"));
    let doc = s.doc().unwrap();
    let parent_name = |n: NodeId| doc.node(doc.node(n).unwrap().parent.unwrap()).unwrap().name.clone();
    assert_eq!((parent_name(a), parent_name(c), parent_name(d)), ("Front".to_string(), "inner".to_string(), "two".to_string()));
    let root = id(&r, "rootId");
    let front = doc.node(a).unwrap().parent.unwrap();
    assert_eq!(doc.node(front).unwrap().parent, Some(root));
    let (fb, ab) = (doc.world_bounds(front).unwrap(), doc.world_bounds(a).unwrap());
    assert!(fb.x0 <= ab.x0 && fb.y0 <= ab.y0 && fb.x1 >= ab.x1 && fb.y1 >= ab.y1, "the section contains its member");
    assert!(r["warnings"].as_array().unwrap().iter().all(|w| !w.as_str().unwrap().contains("overlaps")), "{r}");
}

#[test]
fn a_sequence_diagram_has_lifelines_and_messages_in_order() {
    let mut s = board();
    let r = draw(
        &mut s,
        "sequenceDiagram
  autonumber
  participant C as Client
  C->>S: POST /login
  Note over C,S: TLS
  S-->>C: 200 token
  C->S: bye",
    );
    assert_eq!(r["nodes"].as_array().unwrap().len(), 3, "two participants and a note: {r}");
    let msgs = r["edges"].as_array().unwrap();
    assert_eq!(msgs.len(), 3, "{r}");
    let (m1, m2, m3) = (id(&msgs[0], "id"), id(&msgs[1], "id"), id(&msgs[2], "id"));
    assert!(pos(&s, m2).1 > pos(&s, m1).1);
    assert!(!node(&s, m2).stroke.dashes.is_empty() && node(&s, m1).stroke.dashes.is_empty(), "-->> is dashed");
    assert_eq!(node(&s, m3).stroke.cap_end, figcraft_doc::StrokeCap::None, "-> has no arrowhead");
    assert_eq!(label(&s, m1).as_deref(), Some("1. POST /login"));
    assert_eq!(shape_text(&s, node_id(&r, "C")), "Client");
}

#[test]
fn an_er_diagram_has_entities_with_attributes_and_labelled_relations() {
    let mut s = board();
    let r = draw(
        &mut s,
        "erDiagram
  CUSTOMER ||--o{ ORDER : places
  CUSTOMER {
    string name PK
    int age
  }
  ORDER ||..|{ LINE : contains",
    );
    assert_eq!(r["nodes"].as_array().unwrap().len(), 3, "{r}");
    let text = shape_text(&s, node_id(&r, "CUSTOMER"));
    assert!(text.contains("string name PK") && text.contains("int age"), "{text}");
    assert_eq!(label(&s, id(&r["edges"][0], "id")).as_deref(), Some("places (1 : 0..N)"));
    assert!(!node(&s, id(&r["edges"][1], "id")).stroke.dashes.is_empty(), "`..` is non-identifying (dashed)");
}

#[test]
fn unsupported_syntax_is_an_error_naming_the_line() {
    let mut s = board();
    for (src, needle) in [
        ("pie title x\n\"a\": 1", "line 1"),
        ("graph TD\nA-->B\nA --x B", "line 3"),
        ("graph TD\nA & B --> C", "line 2"),
        ("graph TD\nA-->B\n  ???", "line 3"),
        ("graph TD\nA-->|oops B", "line 2"),
        ("graph TD\nsubgraph x\nA", "missing its `end`"),
        ("graph TD\nend", "line 2"),
        ("graph XX\nA-->B", "line 1"),
        ("sequenceDiagram\nA->>B: hi\nloop x\nA->>B: again\nend", "line 3"),
        ("sequenceDiagram\nA ~~ B: hi", "line 2"),
        ("erDiagram\nA ||--?? B : x", "line 2"),
        ("erDiagram\nA {\nint", "line 3"),
        ("", "empty"),
        ("   \n%% only a comment", "empty"),
    ] {
        let e = err(&mut s, src);
        assert!(e.contains(needle), "{src:?} -> {e}");
        assert!(e.contains("board.fromMermaid"), "{e}");
    }
    assert_eq!(page_children(&s), 0, "a failed parse draws nothing");
}

#[test]
fn styling_lines_are_ignored_with_a_warning() {
    let mut s = board();
    let r = draw(&mut s, "graph TD\nA:::hot --> B\nclassDef hot fill:#f00\nstyle A fill:#fff\n%% comment\nA --> B; B --> C");
    assert_eq!(r["nodes"].as_array().unwrap().len(), 3, "{r}");
    let w = r["warnings"].to_string();
    assert!(w.contains("classDef") && w.contains("style") && w.contains(":::class"), "{w}");
}

#[test]
fn the_node_cap_is_200() {
    let mut s = board();
    let chain = |n: usize| format!("graph LR\n{}", (0..n - 1).map(|i| format!("N{i} --> N{}\n", i + 1)).collect::<String>());
    let r = draw(&mut s, &chain(200));
    assert_eq!(r["nodes"].as_array().unwrap().len(), 200);
    let e = err(&mut s, &chain(201));
    assert!(e.contains("at most 200 nodes"), "{e}");
    let many = format!("graph LR\n{}", "A-->B\n".repeat(501));
    assert!(err(&mut s, &many).contains("500 edges"));
}

#[test]
fn design_files_refuse_and_junk_parameters_are_errors() {
    let mut design = Session::new();
    design.execute("file.new", &json!({})).ok();
    assert!(design.execute("board.fromMermaid", &json!({"mermaid": "graph TD\nA-->B"})).is_err());
    let mut s = board();
    for p in [
        json!({}),
        json!({"mermaid": 7}),
        json!({"mermaid": "graph TD\nA-->B", "direction": "up"}),
        json!({"mermaid": "graph TD\nA-->B", "direction": 3}),
        json!({"mermaid": "graph TD\nA-->B", "x": "a"}),
        json!({"mermaid": "graph TD\nA-->B", "parent": 424242}),
        json!({"mermaid": "x".repeat(200_000)}),
    ] {
        assert!(s.execute("board.fromMermaid", &p).is_err(), "{p}");
    }
    assert_eq!(page_children(&s), 0);
}

#[test]
fn edge_forms_parse() {
    use super::parse::parse;
    for src in [
        "graph TD\nA-->B",
        "graph TD\nA---B",
        "graph TD\nA-.->B",
        "graph TD\nA-.-B",
        "graph TD\nA==>B",
        "graph TD\nA===B",
        "graph TD\nA---->B",
        "graph TD\nA-- text -->B",
        "graph TD\nA-. text .->B",
        "graph TD\nA== text ==>B",
        "graph TD\nA-->|t|B-->C",
        "graph TD\nA[\"quoted [text]\"]-->B([stadium])-->C[[sub]]-->D{{hex}}-->E[/par/]-->F[\\par\\]",
    ] {
        parse(src).unwrap_or_else(|e| panic!("{src:?}: {e}"));
    }
}
