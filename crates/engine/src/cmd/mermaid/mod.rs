//! `board.fromMermaid`: Mermaid text (flowchart, sequence diagram, ER diagram) drawn on a board as
//! shapes joined by connectors that stay attached, inside one section, as one undo step.
//! The parser is `parse`, the layered layout `layout`; this file turns the result into commands.

mod layout;
mod parse;
#[cfg(test)]
mod tests;

use figcraft_doc::NodeId;
use serde_json::{Value, json};

use super::CommandSpec;
use crate::params::{get, id_of, num, opt_id, string};
use crate::{Result, Session, bad, spec};

/// Most nodes (flowchart/ER) or participants, and most edges or messages, one diagram may have.
pub const MAX_NODES: usize = 200;
pub const MAX_EDGES: usize = 500;
const MARGIN: f64 = 24.0;
const CMD: &str = "board.fromMermaid";

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Dir {
    Td,
    Lr,
    Rl,
    Bt,
}

impl Dir {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_uppercase().as_str() {
            "TD" | "TB" => Some(Self::Td),
            "LR" => Some(Self::Lr),
            "RL" => Some(Self::Rl),
            "BT" => Some(Self::Bt),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Shape {
    Rect,
    Rounded,
    Stadium,
    Circle,
    Diamond,
    Database,
    Hexagon,
    Subroutine,
    ParaR,
    ParaL,
}

impl Shape {
    fn board(self) -> &'static str {
        match self {
            Self::Rect => "rectangle",
            Self::Rounded | Self::Stadium => "roundedRectangle",
            Self::Circle => "ellipse",
            Self::Diamond => "diamond",
            Self::Database => "database",
            Self::Hexagon => "hexagon",
            Self::Subroutine => "predefinedProcess",
            Self::ParaR => "parallelogramRight",
            Self::ParaL => "parallelogramLeft",
        }
    }
}

pub struct FNode {
    pub key: String,
    pub label: String,
    pub shape: Shape,
    pub group: Option<usize>,
}

pub struct FEdge {
    pub from: usize,
    pub to: usize,
    pub label: Option<String>,
    pub dotted: bool,
    pub thick: bool,
    pub arrow: bool,
}

pub struct Group {
    pub title: String,
    pub parent: Option<usize>,
}

pub struct Flow {
    pub dir: Dir,
    pub nodes: Vec<FNode>,
    pub edges: Vec<FEdge>,
    pub groups: Vec<Group>,
}

pub struct Msg {
    pub from: usize,
    pub to: usize,
    pub text: String,
    pub dashed: bool,
    pub arrow: bool,
}

pub enum SeqItem {
    Msg(Msg),
    /// kind: 0 over, 1 left of, 2 right of.
    Note {
        kind: u8,
        who: Vec<usize>,
        text: String,
    },
}

pub struct Seq {
    pub title: Option<String>,
    pub autonumber: bool,
    pub people: Vec<(String, String)>,
    pub items: Vec<SeqItem>,
}

pub enum Diagram {
    Flow(Flow),
    Seq(Seq),
}

pub fn specs() -> Vec<CommandSpec> {
    vec![spec!(
        "board.fromMermaid",
        "Diagram from Mermaid",
        "{mermaid (text, at most 100 KB), parent? (the page or a section; default the page), x?, y? (default: beside the existing layers), direction?: TD|TB|LR|RL|BT (flowcharts and ER; default the diagram's own)}  draws Mermaid on a board as shapes joined by attached connectors inside one section, one undo step. Supports flowchart/graph (shapes [] () {} (()) ([]) [()] [[]] {{}} [//] [\\\\], links --> --- -.-> ==> with |label| or -- label -->, subgraph, chains), sequenceDiagram (participant, actor, ->> -->> -> -->, Note over/left of/right of, title, autonumber) and erDiagram (entities with attributes, relationships with cardinality). Styling lines (style, classDef, class, linkStyle, click) are ignored with a warning; any other unsupported syntax is an error naming the line. At most 200 nodes, 500 edges and 200 subgraphs nested 16 deep → {rootId, nodes: [{name, id}], edges: [{from, to, id}], warnings}",
        true,
        from_mermaid
    )]
}

fn from_mermaid(s: &mut Session, p: &Value) -> Result<Value> {
    let src = match get(p, "mermaid") {
        Some(Value::String(t)) => t.as_str(),
        _ => return Err(bad(CMD, "`mermaid` (the diagram text) is required")),
    };
    if src.len() > parse::MAX_INPUT {
        return Err(bad(CMD, format!("`mermaid` is {} bytes: at most {} are accepted", src.len(), parse::MAX_INPUT)));
    }
    let dir = match string(p, "direction") {
        None => None,
        Some(d) => Some(Dir::parse(&d).ok_or_else(|| bad(CMD, "`direction` is TD, TB, LR, RL or BT"))?),
    };
    if get(p, "direction").is_some_and(|v| !v.is_null() && !v.is_string()) {
        return Err(bad(CMD, "`direction` is TD, TB, LR, RL or BT"));
    }
    for k in ["x", "y"] {
        if get(p, k).is_some_and(|v| !v.is_null() && v.as_f64().is_none_or(|n| !n.is_finite())) {
            return Err(bad(CMD, format!("`{k}` must be a number")));
        }
    }
    let parent = opt_id(CMD, p, "parent")?;
    let (diagram, mut warnings) = parse::parse(src).map_err(|e| bad(CMD, e))?;
    let (count, edges) = match &diagram {
        Diagram::Flow(f) => (f.nodes.len(), f.edges.len()),
        Diagram::Seq(q) => (q.people.len(), q.items.len()),
    };
    if count > MAX_NODES || edges > MAX_EDGES {
        return Err(bad(CMD, format!("{count} nodes and {edges} edges: at most {MAX_NODES} nodes and {MAX_EDGES} edges are drawn")));
    }
    if count == 0 {
        return Err(bad(CMD, "the diagram has no nodes"));
    }
    let at = (num(p, "x"), num(p, "y"));
    let r = s.grouped("Mermaid diagram", |s| match &diagram {
        Diagram::Flow(f) => draw_flow(s, f, dir.unwrap_or(f.dir), parent, at, &mut warnings),
        Diagram::Seq(q) => {
            if dir.is_some() {
                warnings.push("`direction` is ignored for sequence diagrams".into());
            }
            draw_seq(s, q, parent, at)
        }
    })?;
    let mut out = r;
    out["warnings"] = json!(warnings);
    Ok(out)
}

/// Size of the shape for `label`: wide enough for the longest line, tall enough for the wrapped lines.
fn node_size(label: &str, shape: Shape) -> (f64, f64) {
    let lens: Vec<usize> = label.lines().map(|l| l.chars().count()).collect();
    let longest = lens.iter().copied().max().unwrap_or(1).max(1) as f64;
    let mut w = (longest * 9.0 + 48.0).clamp(110.0, 280.0);
    let per = ((w - 40.0) / 9.0).floor().max(1.0);
    let rows: f64 = lens.iter().map(|l| (*l as f64 / per).ceil().max(1.0)).sum::<f64>().max(1.0);
    let mut h = (rows * 22.0 + 40.0).max(64.0);
    match shape {
        Shape::Diamond => {
            w = (w * 1.5).min(420.0);
            h = (h * 1.6).max(100.0);
        }
        Shape::Circle => {
            let d = w.max(h * 1.4).min(300.0);
            (w, h) = (d, d);
        }
        Shape::Hexagon | Shape::ParaR | Shape::ParaL => w += 40.0,
        Shape::Database => h += 30.0,
        _ => {}
    }
    (w, h)
}

fn new_id(v: &Value) -> Result<NodeId> {
    v.get("id").and_then(id_of).ok_or_else(|| bad(CMD, "a command returned no id"))
}

/// The section holding the whole diagram, at `at` (default beside the existing layers).
fn root_section(
    s: &mut Session,
    name: &str,
    size: (f64, f64),
    parent: Option<NodeId>,
    at: (Option<f64>, Option<f64>),
) -> Result<(NodeId, (f64, f64))> {
    let pos = match at {
        (None, None) if parent.is_none() => super::create::free_spot(s, size.0, size.1)?,
        (x, y) => (x.unwrap_or(0.0), y.unwrap_or(0.0)),
    };
    let mut params = json!({"type": "section", "x": pos.0, "y": pos.1, "width": size.0.ceil(), "height": size.1.ceil(), "name": name});
    if let Some(pa) = parent {
        params["parent"] = json!(pa);
    }
    let id = new_id(&s.execute("node.create", &params)?)?;
    let origin = s.doc()?.world_bounds(id).map_or(pos, |b| (b.x0, b.y0));
    Ok((id, origin))
}

type Box4 = [f64; 4];

fn merge(a: &mut Option<Box4>, b: Box4) {
    *a = Some(match a {
        Some(x) => [x[0].min(b[0]), x[1].min(b[1]), x[2].max(b[2]), x[3].max(b[3])],
        None => b,
    });
}

fn overlaps(a: Box4, b: Box4) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

fn chain_of(f: &Flow, mut g: Option<usize>) -> Vec<usize> {
    let mut c = Vec::new();
    while let Some(i) = g {
        c.push(i);
        g = f.groups.get(i).and_then(|x| x.parent);
    }
    c.reverse();
    c
}

fn colour(shape: Shape) -> &'static str {
    match shape {
        Shape::Diamond => "yellow",
        Shape::Database => "gray",
        _ => "#DDEBFF",
    }
}

fn draw_flow(s: &mut Session, f: &Flow, dir: Dir, parent: Option<NodeId>, at: (Option<f64>, Option<f64>), warns: &mut Vec<String>) -> Result<Value> {
    let sizes: Vec<(f64, f64)> = f.nodes.iter().map(|n| node_size(&n.label, n.shape)).collect();
    let chains: Vec<Vec<usize>> = f.nodes.iter().map(|n| chain_of(f, n.group)).collect();
    let pairs: Vec<(usize, usize)> = f.edges.iter().map(|e| (e.from, e.to)).collect();
    let vertical = matches!(dir, Dir::Td | Dir::Bt);
    let pos = layout::layout(&sizes, &pairs, &chains, vertical, matches!(dir, Dir::Rl | Dir::Bt));
    let rect = |i: usize| -> Box4 {
        let ((x, y), (w, h)) = (pos[i], sizes[i]);
        [x, y, x + w, y + h]
    };
    // Subgraph boxes, innermost first (children come after their parents in `groups`).
    let (pad, title) = (24.0, 34.0);
    let mut gbox: Vec<Option<Box4>> = vec![None; f.groups.len()];
    for g in (0..f.groups.len()).rev() {
        for (i, n) in f.nodes.iter().enumerate() {
            if n.group == Some(g) {
                merge(&mut gbox[g], rect(i));
            }
        }
        if let Some(b) = gbox[g] {
            let padded = [b[0] - pad, b[1] - pad - title, b[2] + pad, b[3] + pad];
            gbox[g] = Some(padded);
            if let Some(pa) = f.groups[g].parent {
                merge(&mut gbox[pa], padded);
            }
        } else {
            warns.push(format!("subgraph `{}` is empty and is not drawn", f.groups[g].title));
        }
    }
    for (g, b) in gbox.iter().enumerate() {
        let Some(b) = b else { continue };
        for (i, c) in chains.iter().enumerate() {
            if !c.contains(&g) && overlaps(*b, rect(i)) {
                warns.push(format!("subgraph `{}` overlaps node `{}`", f.groups[g].title, f.nodes[i].key));
            }
        }
    }
    let mut all: Option<Box4> = None;
    (0..f.nodes.len()).for_each(|i| merge(&mut all, rect(i)));
    gbox.iter().flatten().for_each(|b| merge(&mut all, *b));
    let all = all.unwrap_or([0.0; 4]);
    let (dx, dy) = (MARGIN - all[0], MARGIN - all[1]);
    let size = (all[2] - all[0] + 2.0 * MARGIN, all[3] - all[1] + 2.0 * MARGIN);
    let (root, _) = root_section(s, "Diagram", size, parent, at)?;
    // Sections for subgraphs, parents first. Origins are in diagram coordinates.
    let mut sec: Vec<Option<(NodeId, (f64, f64))>> = vec![None; f.groups.len()];
    for (g, b) in gbox.iter().enumerate() {
        let Some(b) = b else { continue };
        let (bx, by) = (b[0] + dx, b[1] + dy);
        let (pid, (px, py)) = f.groups[g].parent.and_then(|pa| sec[pa]).unwrap_or((root, (0.0, 0.0)));
        let params = json!({"type": "section", "parent": pid, "x": bx - px, "y": by - py, "width": (b[2] - b[0]).ceil(), "height": (b[3] - b[1]).ceil(), "name": f.groups[g].title});
        sec[g] = Some((new_id(&s.execute("node.create", &params)?)?, (bx, by)));
    }
    let mut ids = Vec::with_capacity(f.nodes.len());
    for (i, n) in f.nodes.iter().enumerate() {
        let (pid, (px, py)) = n.group.and_then(|g| sec[g]).unwrap_or((root, (0.0, 0.0)));
        let params = json!({"shape": n.shape.board(), "x": pos[i].0 + dx - px, "y": pos[i].1 + dy - py, "width": sizes[i].0.ceil(), "height": sizes[i].1.ceil(),
            "text": n.label, "color": colour(n.shape), "parent": pid});
        ids.push(new_id(&s.execute("board.shape", &params)?)?);
    }
    let mut edge_out = Vec::new();
    for e in &f.edges {
        if e.from == e.to {
            warns.push(format!("the link from `{0}` to itself is not drawn", f.nodes[e.from].key));
            continue;
        }
        let (a, b) = (rect(e.from), rect(e.to));
        let (lo, hi) = if vertical { (1, 3) } else { (0, 2) };
        let (fs, ts) = if b[lo] >= a[hi] {
            if vertical { ("bottom", "top") } else { ("right", "left") }
        } else if b[hi] <= a[lo] {
            if vertical { ("top", "bottom") } else { ("left", "right") }
        } else {
            ("auto", "auto")
        };
        let mut params = json!({"from": ids[e.from], "to": ids[e.to], "fromSide": fs, "toSide": ts, "style": "elbow"});
        if !e.arrow {
            params["endCap"] = json!("none");
        }
        if let Some(l) = &e.label {
            params["label"] = json!(l);
        }
        let cid = new_id(&s.execute("board.connector", &params)?)?;
        style_line(s, cid, e.dotted, e.thick)?;
        edge_out.push(json!({"from": f.nodes[e.from].key, "to": f.nodes[e.to].key, "id": cid}));
    }
    s.set_selection(vec![root]);
    let nodes: Vec<Value> = f.nodes.iter().zip(&ids).map(|(n, id)| json!({"name": n.key, "id": id})).collect();
    Ok(json!({"rootId": root, "nodes": nodes, "edges": edge_out}))
}

fn style_line(s: &mut Session, id: NodeId, dotted: bool, thick: bool) -> Result<()> {
    let mut props = serde_json::Map::new();
    if dotted {
        props.insert("dashes".into(), json!([8, 6]));
    }
    if thick {
        props.insert("strokeWeight".into(), json!(4));
    }
    if !props.is_empty() {
        s.execute("node.set", &json!({"ids": [id], "props": props}))?;
    }
    Ok(())
}

fn draw_seq(s: &mut Session, q: &Seq, parent: Option<NodeId>, at: (Option<f64>, Option<f64>)) -> Result<Value> {
    let head_h = 60.0;
    let widths: Vec<f64> =
        q.people.iter().map(|(_, l)| (l.lines().map(|x| x.chars().count()).max().unwrap_or(1) as f64 * 9.0 + 48.0).clamp(110.0, 220.0)).collect();
    // Room between lifelines: boxes, plus the longest message text between neighbours.
    let n = q.people.len();
    let mut gaps: Vec<f64> = (0..n.saturating_sub(1)).map(|k| (widths[k] + widths[k + 1]) / 2.0 + 60.0).collect();
    for it in &q.items {
        if let SeqItem::Msg(m) = it {
            let (a, b) = (m.from.min(m.to), m.from.max(m.to));
            if b > a {
                let need = (m.text.chars().count() as f64 * 8.5 + 48.0) / (b - a) as f64;
                for g in &mut gaps[a..b] {
                    *g = g.max(need).min(420.0);
                }
            }
        }
    }
    let mut cx = vec![widths[0] / 2.0 + MARGIN; n];
    for k in 1..n {
        cx[k] = cx[k - 1] + gaps[k - 1];
    }
    // Rows.
    let mut y = MARGIN + head_h + 60.0;
    let mut rows = Vec::new();
    for it in &q.items {
        let h = match it {
            SeqItem::Msg(_) => 0.0,
            SeqItem::Note { text, .. } => (text.lines().count() as f64 * 22.0 + 24.0).max(50.0),
        };
        rows.push(y);
        y += if h > 0.0 { h + 30.0 } else { 70.0 };
    }
    let bottom = y - 10.0;
    let right = cx[n - 1] + widths[n - 1] / 2.0 + MARGIN;
    let name = q.title.clone().unwrap_or_else(|| "Sequence diagram".into());
    let (root, (ox, oy)) = root_section(s, &name, (right.max(200.0), bottom + MARGIN), parent, at)?;
    let mut people = Vec::new();
    for (i, (key, label)) in q.people.iter().enumerate() {
        let params = json!({"shape": "rectangle", "x": cx[i] - widths[i] / 2.0, "y": MARGIN, "width": widths[i], "height": head_h, "text": label, "color": "#DDEBFF", "parent": root});
        let id = new_id(&s.execute("board.shape", &params)?)?;
        let line = json!({"from": id, "to": {"x": ox + cx[i], "y": oy + bottom}, "fromSide": "bottom", "style": "straight", "startCap": "none", "endCap": "none"});
        let life = new_id(&s.execute("board.connector", &line)?)?;
        style_line(s, life, true, false)?;
        people.push(json!({"name": key, "id": id}));
    }
    let mut edges = Vec::new();
    let mut number = 0;
    for (it, y) in q.items.iter().zip(rows) {
        match it {
            SeqItem::Msg(m) => {
                number += 1;
                let text = if q.autonumber { format!("{number}. {}", m.text) } else { m.text.clone() };
                let (x1, x2) = (ox + cx[m.from], ox + cx[m.to]);
                let y2 = if m.from == m.to { y + 36.0 } else { y };
                let mut params = json!({"from": {"x": x1, "y": oy + y}, "to": {"x": x2, "y": oy + y2}, "style": "straight"});
                if !m.arrow {
                    params["endCap"] = json!("none");
                }
                if !text.is_empty() {
                    params["label"] = json!(text);
                }
                let id = new_id(&s.execute("board.connector", &params)?)?;
                style_line(s, id, m.dashed, false)?;
                edges.push(json!({"from": q.people[m.from].0, "to": q.people[m.to].0, "id": id}));
            }
            SeqItem::Note { kind, who, text } => {
                let (lo, hi) = who.iter().fold((usize::MAX, 0), |(l, h), &w| (l.min(w), h.max(w)));
                let h = (text.lines().count() as f64 * 22.0 + 24.0).max(50.0);
                let (x, w) = match kind {
                    0 => ((cx[lo] + cx[hi]) / 2.0 - ((cx[hi] - cx[lo]) + 140.0).max(150.0) / 2.0, ((cx[hi] - cx[lo]) + 140.0).max(150.0)),
                    1 => (cx[lo] - 14.0 - 160.0, 160.0),
                    _ => (cx[hi] + 14.0, 160.0),
                };
                let params =
                    json!({"shape": "rectangle", "x": x.max(0.0), "y": y, "width": w, "height": h, "text": text, "color": "yellow", "parent": root});
                people.push(json!({"name": "note", "id": new_id(&s.execute("board.shape", &params)?)?}));
            }
        }
    }
    s.set_selection(vec![root]);
    Ok(json!({"rootId": root, "nodes": people, "edges": edges}))
}
