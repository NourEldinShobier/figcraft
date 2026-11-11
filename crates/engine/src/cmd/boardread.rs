//! `board.read`: a whiteboard board as compact JSON an agent can reason about: sections with their
//! stickies, shapes, texts and stamps, connectors with their endpoints and labels, and tables as
//! rows of cell text.

use figcraft_color::PaintKind;
use figcraft_doc::{BOARD_COLORS, BoardItem, Document, End, Node, NodeId, NodeKind};
use serde_json::{Value, json};

use super::CommandSpec;
use crate::board::{label_of, require, text_of};
use crate::params::get;
use crate::{Result, Session, bad, spec};

/// Objects of each list (items, connectors, tables) one call returns unless `limit` says otherwise.
const LISTED: usize = 300;
const MAX_LIMIT: usize = 5000;
/// Longest note, label or cell text returned.
const TEXT_CHARS: usize = 200;
const CELL_CHARS: usize = 80;
/// Layers one call walks.
const MAX_WALK: usize = 500_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![spec!(
        "board.read",
        "Read the board",
        "{page? (id, name or \"all\"; default the open page), limit? (objects listed per list, default 300, up to 5000)}  reads a board file → {summary: {sections, stickies, shapes, texts, stamps, connectors, tables, ink}, sections: [{id, name, items: [{id, type (sticky|shape|text|stamp), text?, color?, shape?, author?, on?, x, y, width, height}]}], connectors: [{id, from, to, label?, fromText?, toText?}], tables: [{id, section?, rows: [[text]]}], truncated?}. `from`/`to` are a layer id or {x, y}; items outside any section are in the first entry, whose id and name are null. Only on boards",
        false,
        read
    )]
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max { s.to_string() } else { format!("{}…", s.chars().take(max).collect::<String>()) }
}

fn r(v: f64) -> f64 {
    if v.is_finite() { (v * 10.0).round() / 10.0 } else { 0.0 }
}

fn characters(doc: &Document, text: Option<NodeId>) -> Option<String> {
    match &doc.node(text?)?.kind {
        NodeKind::Text(t) => Some(t.characters.clone()),
        _ => None,
    }
}

/// The text of a sticky, shape or label, or of a plain text layer.
fn text_in(doc: &Document, id: NodeId) -> Option<String> {
    characters(doc, text_of(doc, id)).or_else(|| characters(doc, Some(id)))
}

fn color_name(n: &Node) -> Option<String> {
    let paint = n.fills.iter().find(|p| p.visible)?;
    let PaintKind::Solid { color } = &paint.kind else { return None };
    let hex = color.to_css_hex();
    Some(BOARD_COLORS.iter().find(|(_, c)| c.to_css_hex() == hex).map_or(hex, |(name, _)| (*name).to_string()))
}

fn geometry(doc: &Document, id: NodeId) -> [(&'static str, Value); 4] {
    let b = doc.bounds_of(&[id]);
    [
        ("x", json!(b.map_or(0.0, |b| r(b.x0)))),
        ("y", json!(b.map_or(0.0, |b| r(b.y0)))),
        ("width", json!(b.map_or(0.0, |b| r(b.width())))),
        ("height", json!(b.map_or(0.0, |b| r(b.height())))),
    ]
}

fn end_of(doc: &Document, end: &End) -> (Value, Option<String>) {
    match end {
        End::Node { node, .. } => (json!(node), text_in(doc, *node).map(|t| clip(&t, 40))),
        End::Point { x, y } => (json!({"x": r(*x), "y": r(*y)}), None),
    }
}

/// Rows of cell text: rows are the laid-out children of the table, cells the children of a row.
fn table_rows(doc: &Document, table: NodeId) -> Vec<Value> {
    let kids = doc.node(table).map(|n| n.children.clone()).unwrap_or_default();
    kids.into_iter()
        .filter_map(|row| doc.node(row).filter(|n| !n.child_layout.absolute))
        .map(|row| {
            let cells = row.children.iter().map(|c| {
                let text =
                    doc.node(*c).and_then(|n| n.children.iter().copied().find(|k| doc.node(*k).is_some_and(|k| matches!(k.kind, NodeKind::Text(_)))));
                json!(characters(doc, text).map(|t| clip(&t, CELL_CHARS)).unwrap_or_default())
            });
            Value::Array(cells.collect())
        })
        .collect()
}

#[derive(Default)]
struct Counts {
    sections: usize,
    stickies: usize,
    shapes: usize,
    texts: usize,
    stamps: usize,
    connectors: usize,
    tables: usize,
    ink: usize,
}

fn read(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "board.read";
    require(s, CMD)?;
    let limit = match get(p, "limit") {
        None | Some(Value::Null) => LISTED,
        Some(v) => v.as_u64().filter(|n| *n <= MAX_LIMIT as u64).ok_or_else(|| bad(CMD, "`limit` must be an integer from 0 to 5000"))? as usize,
    };
    let pages = super::edit::structure_find::pages(s, CMD, p)?;
    let doc = s.doc()?;
    let mut c = Counts::default();
    // Section entries: (id, name, items); the first is the loose items.
    let mut sections: Vec<(Option<NodeId>, Option<String>, Vec<Value>)> = vec![(None, None, Vec::new())];
    let (mut connectors, mut tables) = (Vec::new(), Vec::new());
    let mut cut = false;
    let mut walked = 0usize;
    // The stack carries the index of the section each layer sits in.
    let mut stack: Vec<(NodeId, usize)> = pages.iter().rev().map(|p| (*p, 0)).collect();
    while let Some((id, sec)) = stack.pop() {
        walked += 1;
        if walked > MAX_WALK {
            cut = true;
            break;
        }
        let Some(n) = doc.node(id) else { continue };
        let mut sec = sec;
        let mut recurse = true;
        match (&n.board, &n.kind) {
            (Some(BoardItem::Connector(conn)), _) => {
                c.connectors += 1;
                recurse = false;
                if connectors.len() < limit {
                    let ((from, ft), (to, tt)) = (end_of(doc, &conn.from), end_of(doc, &conn.to));
                    let mut o = json!({"id": id, "from": from, "to": to});
                    if let Some(l) = characters(doc, label_of(doc, id).and_then(|l| text_of(doc, l))).filter(|t| !t.is_empty()) {
                        o["label"] = json!(clip(&l, TEXT_CHARS));
                    }
                    if let Some(t) = ft {
                        o["fromText"] = json!(t);
                    }
                    if let Some(t) = tt {
                        o["toText"] = json!(t);
                    }
                    connectors.push(o);
                }
            }
            (Some(BoardItem::Table), _) => {
                c.tables += 1;
                recurse = false;
                if tables.len() < limit {
                    let section = sections.get(sec).and_then(|s| s.0);
                    tables.push(json!({"id": id, "section": section, "rows": table_rows(doc, id)}));
                }
            }
            (Some(BoardItem::Ink), _) => {
                c.ink += 1;
                recurse = false;
            }
            (Some(BoardItem::Label { .. }), _) => recurse = false,
            (Some(item @ (BoardItem::Sticky { .. } | BoardItem::Shape { .. } | BoardItem::Stamp { .. })), _) => {
                recurse = false;
                let mut o = json!({"id": id});
                match item {
                    BoardItem::Sticky { author } => {
                        c.stickies += 1;
                        o["type"] = json!("sticky");
                        if !author.is_empty() {
                            o["author"] = json!(author);
                        }
                    }
                    BoardItem::Shape { shape } => {
                        c.shapes += 1;
                        o["type"] = json!("shape");
                        o["shape"] = json!(shape.id());
                    }
                    BoardItem::Stamp { stamp, author, on } => {
                        c.stamps += 1;
                        o["type"] = json!("stamp");
                        o["stamp"] = serde_json::to_value(stamp).unwrap_or(Value::Null);
                        if !author.is_empty() {
                            o["author"] = json!(author);
                        }
                        if let Some(on) = on {
                            o["on"] = json!(on);
                        }
                    }
                    _ => {}
                }
                if !matches!(item, BoardItem::Stamp { .. }) {
                    if let Some(t) = text_in(doc, id) {
                        o["text"] = json!(clip(&t, TEXT_CHARS));
                    }
                    if let Some(col) = color_name(n) {
                        o["color"] = json!(col);
                    }
                }
                for (k, v) in geometry(doc, id) {
                    o[k] = v;
                }
                if let Some(list) = sections.get_mut(sec).map(|s| &mut s.2).filter(|l| l.len() < limit) {
                    list.push(o);
                }
            }
            (None, NodeKind::Text(t)) => {
                c.texts += 1;
                let mut o = json!({"id": id, "type": "text", "text": clip(&t.characters, TEXT_CHARS)});
                for (k, v) in geometry(doc, id) {
                    o[k] = v;
                }
                if let Some(list) = sections.get_mut(sec).map(|s| &mut s.2).filter(|l| l.len() < limit) {
                    list.push(o);
                }
            }
            (None, NodeKind::Section { .. }) => {
                c.sections += 1;
                sections.push((Some(id), Some(n.name.clone()), Vec::new()));
                sec = sections.len() - 1;
            }
            _ => {}
        }
        if recurse {
            stack.extend(n.children.iter().rev().map(|k| (*k, sec)));
        }
    }
    let listed = sections.iter().map(|s| s.2.len()).sum::<usize>();
    let total = c.stickies + c.shapes + c.texts + c.stamps;
    let mut out = json!({
        "summary": {"sections": c.sections, "stickies": c.stickies, "shapes": c.shapes, "texts": c.texts, "stamps": c.stamps, "connectors": c.connectors, "tables": c.tables, "ink": c.ink},
        "sections": sections.into_iter().filter(|s| s.0.is_some() || !s.2.is_empty()).map(|(id, name, items)| json!({"id": id, "name": name, "items": items})).collect::<Vec<_>>(),
        "connectors": connectors,
        "tables": tables,
    });
    let (lc, lt) = (out["connectors"].as_array().map_or(0, Vec::len), out["tables"].as_array().map_or(0, Vec::len));
    if cut || listed < total || lc < c.connectors || lt < c.tables {
        out["truncated"] = json!(format!(
            "listed {listed} of {total} items, {lc} of {} connectors, {lt} of {} tables{}; the summary counts everything. Raise `limit` or read one `page`",
            c.connectors,
            c.tables,
            if cut { " (the walk stopped early on a very large file)" } else { "" }
        ));
    }
    Ok(out)
}
