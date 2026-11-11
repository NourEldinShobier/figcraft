//! A small Mermaid parser: flowcharts (`graph`/`flowchart`), `sequenceDiagram` and `erDiagram`.
//! Anything outside the supported subset is an error naming the line; pure styling is a warning.

use std::collections::HashMap;

use super::{Diagram, Dir, FEdge, FNode, Flow, Group, Msg, Seq, SeqItem, Shape};

/// Source accepted, in bytes.
pub const MAX_INPUT: usize = 100_000;

/// Subgraphs per flowchart and how deep they nest: the layout is quadratic in both.
const MAX_GROUPS: usize = 200;
const MAX_GROUP_DEPTH: usize = 16;

type Res<T> = Result<T, String>;

#[derive(Default)]
pub struct Warnings(pub Vec<String>);

impl Warnings {
    fn add(&mut self, m: String) {
        if !self.0.contains(&m) && self.0.len() < 50 {
            self.0.push(m);
        }
    }
}

fn at(line: usize, text: &str, why: impl std::fmt::Display) -> String {
    let shown: String = text.chars().take(80).collect();
    format!("line {line}: {why}: `{shown}`")
}

pub fn parse(src: &str) -> Res<(Diagram, Vec<String>)> {
    let mut w = Warnings::default();
    let mut lines: Vec<(usize, String)> = Vec::new();
    let mut front = false;
    for (i, raw) in src.lines().enumerate() {
        let t = raw.split("%%").next().unwrap_or("").trim();
        if t == "---" && (lines.is_empty() || front) {
            front = !front;
            w.add("front matter is ignored".into());
            continue;
        }
        if front || t.is_empty() {
            continue;
        }
        lines.push((i + 1, t.to_string()));
    }
    let Some(((hn, head), body)) = lines.split_first() else { return Err("the diagram is empty".into()) };
    let word = head.split(|c: char| c.is_whitespace() || c == ';').next().unwrap_or("");
    let d = match word.to_ascii_lowercase().as_str() {
        "graph" | "flowchart" => {
            let dir = head[word.len()..].trim().trim_end_matches(';').trim();
            let dir = if dir.is_empty() { Dir::Td } else { Dir::parse(dir).ok_or_else(|| at(*hn, head, "direction is TD, TB, LR, RL or BT"))? };
            Diagram::Flow(flow(dir, body, &mut w)?)
        }
        "sequencediagram" => Diagram::Seq(sequence(body, &mut w)?),
        "erdiagram" => Diagram::Flow(er(body, &mut w)?),
        _ => return Err(at(*hn, head, "unsupported diagram type (supported: graph/flowchart, sequenceDiagram, erDiagram)")),
    };
    Ok((d, w.0))
}

/// Statements of a line: split at `;` outside quotes and brackets.
fn statements(line: &str) -> Vec<&str> {
    let (mut out, mut depth, mut quote, mut start) = (Vec::new(), 0i32, false, 0);
    for (i, c) in line.char_indices() {
        match c {
            '"' => quote = !quote,
            '[' | '(' | '{' if !quote => depth += 1,
            ']' | ')' | '}' if !quote => depth -= 1,
            ';' if !quote && depth <= 0 => {
                out.push(&line[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&line[start..]);
    out.into_iter().map(str::trim).filter(|s| !s.is_empty()).collect()
}

fn clean_label(s: &str) -> String {
    let s = s.trim();
    let s = s.strip_prefix('"').and_then(|x| x.strip_suffix('"')).unwrap_or(s);
    let mut out = s.to_string();
    for br in ["<br/>", "<br />", "<br>", "<BR/>", "<BR>"] {
        out = out.replace(br, "\n");
    }
    out.replace("&quot;", "\"").replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").trim().to_string()
}

// ---------------------------------------------------------------- flowchart

struct FlowState {
    f: Flow,
    index: HashMap<String, usize>,
    stack: Vec<usize>,
}

fn flow(dir: Dir, body: &[(usize, String)], w: &mut Warnings) -> Res<Flow> {
    let mut st = FlowState { f: Flow { dir, nodes: vec![], edges: vec![], groups: vec![] }, index: HashMap::new(), stack: vec![] };
    for (n, line) in body {
        for s in statements(line) {
            flow_stmt(&mut st, s, *n, w)?;
        }
    }
    if !st.stack.is_empty() {
        return Err("a subgraph is missing its `end`".into());
    }
    Ok(st.f)
}

fn flow_stmt(st: &mut FlowState, s: &str, n: usize, w: &mut Warnings) -> Res<()> {
    let word = s.split_whitespace().next().unwrap_or("");
    match word {
        "subgraph" => {
            let rest = s[8..].trim();
            let title = match (rest.find('['), rest.ends_with(']')) {
                (Some(p), true) => clean_label(&rest[p + 1..rest.len() - 1]),
                _ => clean_label(rest),
            };
            if title.is_empty() {
                return Err(at(n, s, "subgraph needs a title"));
            }
            if st.f.groups.len() >= MAX_GROUPS || st.stack.len() >= MAX_GROUP_DEPTH {
                return Err(at(n, s, format!("at most {MAX_GROUPS} subgraphs, nested {MAX_GROUP_DEPTH} deep")));
            }
            st.f.groups.push(Group { title, parent: st.stack.last().copied() });
            st.stack.push(st.f.groups.len() - 1);
        }
        "end" if s == "end" => {
            st.stack.pop().ok_or_else(|| at(n, s, "`end` without a subgraph"))?;
        }
        "direction" => w.add("`direction` inside a subgraph is ignored".into()),
        "classDef" | "class" | "style" | "linkStyle" => w.add(format!("`{word}` styling is ignored")),
        "click" => w.add("`click` is ignored".into()),
        "accTitle" | "accDescr" => {}
        _ => chain(st, s, n, w)?,
    }
    Ok(())
}

fn chain(st: &mut FlowState, s: &str, n: usize, w: &mut Warnings) -> Res<()> {
    let (mut prev, mut rest) = node_ref(st, s, w).map_err(|e| at(n, s, e))?;
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            return Ok(());
        }
        if rest.starts_with('&') {
            return Err(at(n, s, "`&` (several nodes in one statement) is not supported"));
        }
        let (op, r) = edge_op(rest).map_err(|e| at(n, s, e))?;
        let (next, r) = node_ref(st, r.trim_start(), w).map_err(|e| at(n, s, e))?;
        st.f.edges.push(FEdge { from: prev, to: next, label: op.label, dotted: op.dotted, thick: op.thick, arrow: op.arrow });
        prev = next;
        rest = r;
    }
}

const OPENERS: [(&str, &str, Shape); 10] = [
    ("((", "))", Shape::Circle),
    ("([", "])", Shape::Stadium),
    ("[(", ")]", Shape::Database),
    ("[[", "]]", Shape::Subroutine),
    ("{{", "}}", Shape::Hexagon),
    ("[/", "/]", Shape::ParaR),
    ("[\\", "\\]", Shape::ParaL),
    ("[", "]", Shape::Rect),
    ("(", ")", Shape::Rounded),
    ("{", "}", Shape::Diamond),
];

/// A node mention at the start of `s`: registers it, returns its index and the rest.
fn node_ref<'a>(st: &mut FlowState, s: &'a str, w: &mut Warnings) -> Res<(usize, &'a str)> {
    let end = s.char_indices().find(|(_, c)| !(c.is_alphanumeric() || *c == '_')).map_or(s.len(), |(i, _)| i);
    if end == 0 {
        return Err("expected a node id".into());
    }
    let key = &s[..end];
    let mut rest = &s[end..];
    let mut shaped = None;
    if let Some((open, close, shape)) = OPENERS.iter().find(|(o, ..)| rest.starts_with(o)) {
        let inner = rest[open.len()..].trim_start();
        let (text, after) = if let Some(q) = inner.strip_prefix('"') {
            let e = q.find('"').ok_or("unterminated quote")?;
            let after = q[e + 1..].trim_start();
            (q[..e].to_string(), after.strip_prefix(close).ok_or_else(|| format!("expected `{close}` after the label of {key}"))?)
        } else {
            let e = inner.find(close).ok_or_else(|| format!("unsupported or unterminated shape for {key}"))?;
            (inner[..e].to_string(), &inner[e + close.len()..])
        };
        shaped = Some((*shape, clean_label(&text)));
        rest = after;
    }
    if let Some(r) = rest.strip_prefix(":::") {
        w.add("`:::class` styling is ignored".into());
        rest = r.trim_start_matches(|c: char| c.is_alphanumeric() || c == '_');
    }
    if rest.starts_with('@') {
        return Err("`@{ ... }` node attributes are not supported".into());
    }
    let group = st.stack.last().copied();
    let idx = match st.index.get(key) {
        Some(i) => *i,
        None => {
            st.f.nodes.push(FNode { key: key.to_string(), label: key.to_string(), shape: Shape::Rect, group });
            st.index.insert(key.to_string(), st.f.nodes.len() - 1);
            st.f.nodes.len() - 1
        }
    };
    let node = &mut st.f.nodes[idx];
    if let Some((shape, text)) = shaped {
        node.shape = shape;
        if !text.is_empty() {
            node.label = text;
        }
    }
    if node.group.is_none() {
        node.group = group;
    }
    Ok((idx, rest))
}

struct Op {
    label: Option<String>,
    dotted: bool,
    thick: bool,
    arrow: bool,
}

fn edge_op(r: &str) -> Res<(Op, &str)> {
    let b = r.as_bytes();
    let blank = |i: usize| b.get(i).is_some_and(u8::is_ascii_whitespace);
    let (mut label, mut i, arrow);
    let (dotted, thick) = (b.starts_with(b"-."), b.first() == Some(&b'='));
    if !(dotted || thick || b.first() == Some(&b'-')) {
        return Err(format!("expected a link such as `-->`, got `{}`", r.chars().take(20).collect::<String>()));
    }
    label = None;
    if dotted {
        i = 2;
        if blank(i) {
            let end = r[i..].find(".-").ok_or("unterminated dotted link label")? + i;
            label = Some(clean_label(&r[i..end]));
            i = end + 2;
        } else {
            while b.get(i) == Some(&b'.') {
                i += 1;
            }
            if b.get(i) != Some(&b'-') {
                return Err("malformed dotted link (expected `-.->` or `-.-`)".into());
            }
            i += 1;
        }
        arrow = b.get(i) == Some(&b'>');
        i += usize::from(arrow);
    } else {
        let ch = b[0];
        i = 0;
        while b.get(i) == Some(&ch) {
            i += 1;
        }
        let run = i;
        if b.get(i) == Some(&b'>') {
            if run < 2 {
                return Err("malformed link".into());
            }
            arrow = true;
            i += 1;
        } else if run == 2 && matches!(b.get(i), Some(b'x' | b'o')) && (b.get(i + 1).is_none() || blank(i + 1)) {
            return Err("cross and circle link ends (`--x`, `--o`) are not supported".into());
        } else if run >= 3 {
            arrow = false;
        } else if run == 2 && blank(i) {
            let marker = String::from_utf8(vec![ch; 2]).unwrap_or_default();
            let end = r[i..].find(&marker).ok_or("unterminated link label")? + i;
            label = Some(clean_label(&r[i..end]));
            i = end;
            while b.get(i) == Some(&ch) {
                i += 1;
            }
            arrow = b.get(i) == Some(&b'>');
            i += usize::from(arrow);
        } else {
            return Err("malformed link (expected `-->`, `---`, `==>` or `-.->`)".into());
        }
    }
    let mut rest = &r[i..];
    let t = rest.trim_start();
    if let Some(l) = t.strip_prefix('|') {
        let e = l.find('|').ok_or("unterminated `|label|`")?;
        label = Some(clean_label(&l[..e]));
        rest = &l[e + 1..];
    }
    Ok((Op { label: label.filter(|l| !l.is_empty()), dotted, thick, arrow }, rest))
}

// ---------------------------------------------------------------- sequence

fn person(q: &mut Seq, id: &str) -> usize {
    match q.people.iter().position(|(k, _)| k == id) {
        Some(i) => i,
        None => {
            q.people.push((id.to_string(), id.to_string()));
            q.people.len() - 1
        }
    }
}

fn sequence(body: &[(usize, String)], w: &mut Warnings) -> Res<Seq> {
    let mut q = Seq { title: None, autonumber: false, people: vec![], items: vec![] };
    for (n, line) in body {
        let n = *n;
        let line = line.trim_end_matches(';').trim();
        let low = line.to_ascii_lowercase();
        let first = low.split_whitespace().next().unwrap_or("");
        match first {
            "participant" | "actor" => {
                let rest = line[first.len()..].trim();
                let (id, label) = match rest.split_once(" as ") {
                    Some((i, l)) => (i.trim(), clean_label(l)),
                    None => (rest, rest.to_string()),
                };
                if id.is_empty() || id.contains(char::is_whitespace) {
                    return Err(at(n, line, "participant needs a single-word id"));
                }
                let i = person(&mut q, id);
                q.people[i].1 = label;
            }
            "title" => q.title = Some(clean_label(&line[5..])),
            "autonumber" => q.autonumber = true,
            "activate" | "deactivate" => w.add("activations are ignored".into()),
            "loop" | "alt" | "else" | "opt" | "par" | "and" | "critical" | "option" | "break" | "rect" | "end" | "box" | "create" | "destroy"
            | "link" | "links" => {
                return Err(at(n, line, format!("`{first}` blocks are not supported in sequence diagrams")));
            }
            "note" => {
                let (head, text) = line[4..].split_once(':').ok_or_else(|| at(n, line, "a note needs `: text`"))?;
                let head = head.trim();
                let hl = head.to_ascii_lowercase();
                let (kind, names) = if let Some(r) = hl.strip_prefix("over ") {
                    (0, &head[head.len() - r.len()..])
                } else if let Some(r) = hl.strip_prefix("left of ") {
                    (1, &head[head.len() - r.len()..])
                } else if let Some(r) = hl.strip_prefix("right of ") {
                    (2, &head[head.len() - r.len()..])
                } else {
                    return Err(at(n, line, "a note is `Note over A,B:`, `Note left of A:` or `Note right of A:`"));
                };
                let who: Vec<usize> = names.split(',').map(|x| person(&mut q, x.trim())).collect();
                q.items.push(SeqItem::Note { kind, who, text: clean_label(text) });
            }
            _ => {
                let (head, text) = line.split_once(':').ok_or_else(|| at(n, line, "unsupported statement"))?;
                let arrows = ["-->>", "->>", "--)", "-)", "--x", "-x", "-->", "->"];
                let (pos, arrow) = arrows
                    .iter()
                    .filter_map(|a| head.find(a).map(|p| (p, *a)))
                    .min_by_key(|(p, a)| (*p, std::cmp::Reverse(a.len())))
                    .ok_or_else(|| at(n, line, "unsupported statement (messages use ->>, -->>, ->, -->)"))?;
                if arrow.ends_with('x') {
                    return Err(at(n, line, "cross-ended messages (`-x`) are not supported"));
                }
                let from = head[..pos].trim();
                let to = head[pos + arrow.len()..].trim().trim_start_matches(['+', '-']).trim();
                if from.is_empty() || to.is_empty() || from.contains(char::is_whitespace) || to.contains(char::is_whitespace) {
                    return Err(at(n, line, "a message is `From->>To: text`"));
                }
                let (from, to) = (person(&mut q, from), person(&mut q, to));
                q.items.push(SeqItem::Msg(Msg {
                    from,
                    to,
                    text: clean_label(text),
                    dashed: arrow.starts_with("--"),
                    arrow: arrow.ends_with(">>") || arrow.ends_with(')'),
                }));
            }
        }
    }
    if q.people.is_empty() {
        return Err("the sequence diagram has no participants".into());
    }
    Ok(q)
}

// ---------------------------------------------------------------- entity relationship

fn entity(f: &mut Flow, k: &str) -> usize {
    match f.nodes.iter().position(|x| x.key == k) {
        Some(i) => i,
        None => {
            f.nodes.push(FNode { key: k.to_string(), label: k.to_string(), shape: Shape::Rect, group: None });
            f.nodes.len() - 1
        }
    }
}

fn card(c: &str) -> Option<&'static str> {
    match c {
        "||" => Some("1"),
        "|o" | "o|" => Some("0..1"),
        "}o" | "o{" => Some("0..N"),
        "}|" | "|{" => Some("1..N"),
        _ => None,
    }
}

fn er(body: &[(usize, String)], w: &mut Warnings) -> Res<Flow> {
    let mut f = Flow { dir: Dir::Lr, nodes: vec![], edges: vec![], groups: vec![] };
    let mut open: Option<usize> = None;
    for (n, line) in body {
        let n = *n;
        if let Some(i) = open {
            if line == "}" {
                open = None;
            } else {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() < 2 {
                    return Err(at(n, line, "an attribute is `type name [PK|FK|UK] [\"comment\"]`"));
                }
                let keep: Vec<&str> = parts.iter().copied().take_while(|p| !p.starts_with('"')).collect();
                f.nodes[i].label.push_str(&format!("\n{}", keep.join(" ")));
            }
            continue;
        }
        if let Some(name) = line.strip_suffix('{') {
            let name = name.trim();
            if name.is_empty() || name.contains(char::is_whitespace) {
                return Err(at(n, line, "an entity block is `NAME {`"));
            }
            open = Some(entity(&mut f, name));
            continue;
        }
        let (head, label) = match line.split_once(':') {
            Some((h, l)) => (h.trim(), Some(clean_label(l))),
            None => (line.as_str(), None),
        };
        let parts: Vec<&str> = head.split_whitespace().collect();
        match parts.as_slice() {
            [one] if label.is_none() => {
                entity(&mut f, one);
            }
            [a, rel, b] => {
                let (dotted, p) = match (rel.find("--"), rel.find("..")) {
                    (Some(p), _) => (false, p),
                    (None, Some(p)) => (true, p),
                    _ => return Err(at(n, line, "a relationship is `A ||--o{ B : label`")),
                };
                let (Some(l), Some(r)) = (card(&rel[..p]), card(&rel[p + 2..])) else {
                    return Err(at(n, line, "unknown cardinality (use ||, |o, o|, }o, o{, }|, |{)"));
                };
                let (a, b) = (entity(&mut f, a), entity(&mut f, b));
                let text = match label.filter(|x| !x.is_empty()) {
                    Some(t) => format!("{t} ({l} : {r})"),
                    None => format!("{l} : {r}"),
                };
                f.edges.push(FEdge { from: a, to: b, label: Some(text), dotted, thick: false, arrow: false });
            }
            _ => return Err(at(n, line, "unsupported statement")),
        }
    }
    if open.is_some() {
        return Err("an entity block is missing its `}`".into());
    }
    if f.nodes.is_empty() {
        w.add("the diagram has no entities".into());
    }
    Ok(f)
}
