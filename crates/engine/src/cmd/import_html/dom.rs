//! A small tolerant HTML parser for `import.html`: elements, attributes, text, raw-text elements
//! (`style`, `script`, ...) and inline `<svg>` kept as source. Not the HTML5 tree builder (no
//! foster parenting, no table repair): it closes `p`, `li`, `td` and friends the way pages are
//! written, ignores stray end tags and caps depth and size, so any input yields a tree.

/// Most elements and text nodes kept; the rest of the document is dropped.
pub const MAX_DOM_NODES: usize = 60_000;
/// Nesting deeper than this is flattened into the deepest level.
const MAX_DEPTH: usize = 200;

pub enum Kind {
    Root,
    Element { tag: String, attrs: Vec<(String, String)>, raw: Option<String> },
    Text(String),
}

pub struct Node {
    pub kind: Kind,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
}

pub struct Dom {
    pub nodes: Vec<Node>,
    /// The document ended before the end of the input (node cap).
    pub truncated: bool,
}

impl Dom {
    pub fn tag(&self, i: usize) -> Option<&str> {
        match &self.nodes.get(i)?.kind {
            Kind::Element { tag, .. } => Some(tag),
            _ => None,
        }
    }

    pub fn attr(&self, i: usize, name: &str) -> Option<&str> {
        match &self.nodes.get(i)?.kind {
            Kind::Element { attrs, .. } => attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str()),
            _ => None,
        }
    }

    /// The source of an inline `<svg>`.
    pub fn raw(&self, i: usize) -> Option<&str> {
        match &self.nodes.get(i)?.kind {
            Kind::Element { raw, .. } => raw.as_deref(),
            _ => None,
        }
    }

    pub fn children(&self, i: usize) -> &[usize] {
        self.nodes.get(i).map(|n| n.children.as_slice()).unwrap_or(&[])
    }

    pub fn parent(&self, i: usize) -> Option<usize> {
        self.nodes.get(i)?.parent
    }

    /// The text of all text nodes under `i`.
    pub fn text_of(&self, i: usize) -> String {
        let mut out = String::new();
        let mut stack = vec![i];
        while let Some(n) = stack.pop() {
            match self.nodes.get(n).map(|n| &n.kind) {
                Some(Kind::Text(t)) => out.push_str(t),
                _ => stack.extend(self.children(n).iter().rev()),
            }
        }
        out
    }

    /// First element named `tag` in document order.
    pub fn find(&self, tag: &str) -> Option<usize> {
        (0..self.nodes.len()).find(|i| self.tag(*i) == Some(tag))
    }
}

const VOID: &[&str] = &["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track", "wbr"];
const RAW_TEXT: &[&str] = &["style", "script", "textarea", "title"];
const BLOCKS: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "div",
    "dl",
    "fieldset",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hr",
    "main",
    "nav",
    "ol",
    "p",
    "pre",
    "section",
    "table",
    "ul",
    "figure",
    "details",
];

/// Whether the start tag `new` ends the open element `open` (HTML's optional end tags).
fn closes(open: &str, new: &str) -> bool {
    match open {
        "p" => BLOCKS.contains(&new),
        "li" => new == "li",
        "dt" | "dd" => matches!(new, "dt" | "dd"),
        "td" | "th" => matches!(new, "td" | "th" | "tr"),
        "tr" => new == "tr",
        "option" => new == "option",
        _ => false,
    }
}

pub fn parse(html: &str) -> Dom {
    let lower = html.to_ascii_lowercase();
    let mut p = Parser {
        src: html,
        lower: &lower,
        dom: Dom { nodes: vec![Node { kind: Kind::Root, parent: None, children: Vec::new() }], truncated: false },
        stack: vec![0],
    };
    p.run();
    p.dom
}

struct Parser<'a> {
    src: &'a str,
    /// `src` in ASCII lower case: same byte offsets.
    lower: &'a str,
    dom: Dom,
    /// Open elements, the root first.
    stack: Vec<usize>,
}

impl Parser<'_> {
    fn run(&mut self) {
        let mut pos = 0;
        while pos < self.src.len() && !self.dom.truncated {
            let Some(rest) = self.src.get(pos..) else { break };
            match rest.find('<') {
                None => {
                    self.text(rest);
                    break;
                }
                Some(0) => pos += self.markup(pos),
                Some(n) => {
                    self.text(rest.get(..n).unwrap_or(""));
                    pos += n;
                }
            }
        }
    }

    /// Handle the construct at `pos` (a `<`); returns the bytes it covers (at least 1).
    fn markup(&mut self, pos: usize) -> usize {
        let rest = self.src.get(pos..).unwrap_or("");
        if rest.starts_with("<!--") {
            return rest.find("-->").map_or(rest.len(), |e| e + 3);
        }
        if rest.starts_with("<!") || rest.starts_with("<?") {
            return rest.find('>').map_or(rest.len(), |e| e + 1);
        }
        if let Some(after) = rest.strip_prefix("</") {
            let end = after.find('>').map_or(after.len(), |e| e + 1);
            let name: String = after.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-').collect::<String>().to_ascii_lowercase();
            self.end_tag(&name);
            return 2 + end;
        }
        let name: String =
            rest.chars().skip(1).take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | ':')).collect::<String>().to_ascii_lowercase();
        if name.is_empty() || !rest.chars().nth(1).is_some_and(|c| c.is_ascii_alphabetic()) {
            self.text("<");
            return 1;
        }
        let (attrs, used, self_closing) = parse_attrs(rest.get(1 + name.len()..).unwrap_or(""));
        let total = 1 + name.len() + used;
        self.start_tag(pos, &name, attrs, self_closing, total)
    }

    fn start_tag(&mut self, pos: usize, name: &str, attrs: Vec<(String, String)>, self_closing: bool, total: usize) -> usize {
        while let Some(top) = self.stack.last().and_then(|t| self.dom.tag(*t)).map(str::to_string) {
            if !closes(&top, name) {
                break;
            }
            self.stack.pop();
        }
        if name == "svg" {
            let end = self.svg_end(pos + total).unwrap_or(self.src.len());
            let raw = self.src.get(pos..end).unwrap_or("").to_string();
            self.push(Kind::Element { tag: "svg".into(), attrs, raw: Some(raw) }, false);
            return end - pos;
        }
        let void = VOID.contains(&name) || self_closing && !RAW_TEXT.contains(&name);
        let id = self.push(Kind::Element { tag: name.to_string(), attrs, raw: None }, !void);
        if RAW_TEXT.contains(&name) && !void {
            let from = pos + total;
            let close = self.lower.get(from..).and_then(|l| l.find(&format!("</{name}"))).map_or(self.src.len(), |e| from + e);
            if let Some(body) = self.src.get(from..close).filter(|b| !b.is_empty()) {
                let text = if name == "textarea" || name == "title" { decode(body) } else { body.to_string() };
                self.add(id, Kind::Text(text));
            }
            self.stack.pop();
            let after = self.src.get(close..).and_then(|r| r.find('>')).map_or(self.src.len(), |e| close + e + 1);
            return after - pos;
        }
        total
    }

    /// Where the `<svg>` that opens before `from` ends (after its matching `</svg>`).
    fn svg_end(&self, from: usize) -> Option<usize> {
        let l = self.lower;
        let (mut depth, mut at) = (1usize, from);
        while depth > 0 {
            let rest = l.get(at..)?;
            let open = rest.find("<svg");
            let close = rest.find("</svg")?;
            match open.filter(|o| *o < close) {
                Some(o) => {
                    depth += 1;
                    at += o + 4;
                }
                None => {
                    depth -= 1;
                    at += close + 5;
                }
            }
        }
        Some(l.get(at..)?.find('>').map_or(l.len(), |e| at + e + 1))
    }

    fn end_tag(&mut self, name: &str) {
        let found = self.stack.iter().rposition(|i| self.dom.tag(*i) == Some(name));
        // The root (index 0) never closes.
        if let Some(at) = found.filter(|a| *a > 0) {
            self.stack.truncate(at);
        }
    }

    fn text(&mut self, raw: &str) {
        if raw.is_empty() {
            return;
        }
        let top = self.stack.last().copied().unwrap_or(0);
        // Text after text joins (a stray `<` split it).
        if let Some(Kind::Text(prev)) = self.dom.children(top).last().and_then(|l| self.dom.nodes.get(*l)).map(|n| &n.kind) {
            let joined = format!("{prev}{}", decode(raw));
            if let Some(Node { kind: Kind::Text(t), .. }) = self.dom.children(top).last().copied().and_then(|l| self.dom.nodes.get_mut(l)) {
                *t = joined;
            }
            return;
        }
        self.add(top, Kind::Text(decode(raw)));
    }

    fn add(&mut self, parent: usize, kind: Kind) -> usize {
        if self.dom.nodes.len() >= MAX_DOM_NODES {
            self.dom.truncated = true;
            return parent;
        }
        let id = self.dom.nodes.len();
        self.dom.nodes.push(Node { kind, parent: Some(parent), children: Vec::new() });
        if let Some(p) = self.dom.nodes.get_mut(parent) {
            p.children.push(id);
        }
        id
    }

    /// Add an element under the open one; `open` leaves it on the stack.
    fn push(&mut self, kind: Kind, open: bool) -> usize {
        let top = self.stack.last().copied().unwrap_or(0);
        let id = self.add(top, kind);
        if open && id != top && self.stack.len() < MAX_DEPTH {
            self.stack.push(id);
        }
        id
    }
}

/// Attributes up to the closing `>`: (name, value) pairs, the bytes used and whether it ended `/>`.
fn parse_attrs(s: &str) -> (Vec<(String, String)>, usize, bool) {
    let b = s.as_bytes();
    let (mut i, mut attrs, mut self_closing) = (0usize, Vec::new(), false);
    while let Some(&c) = b.get(i) {
        match c {
            b'>' => return (attrs, i + 1, self_closing),
            b'/' => {
                self_closing = b.get(i + 1) == Some(&b'>');
                i += 1;
            }
            c if c.is_ascii_whitespace() => i += 1,
            _ => {
                self_closing = false;
                let (name, value, used) = one_attr(s.get(i..).unwrap_or(""));
                if !name.is_empty() && !attrs.iter().any(|(k, _)| *k == name) {
                    attrs.push((name, value));
                }
                i += used.max(1);
            }
        }
    }
    (attrs, s.len(), self_closing)
}

fn one_attr(s: &str) -> (String, String, usize) {
    let name_end = s.find(|c: char| c.is_whitespace() || matches!(c, '=' | '>' | '/')).unwrap_or(s.len());
    let name = s.get(..name_end).unwrap_or("").to_ascii_lowercase();
    let after = s.get(name_end..).unwrap_or("");
    let trimmed = after.trim_start();
    let Some(val) = trimmed.strip_prefix('=') else { return (name, String::new(), name_end) };
    let val = val.trim_start();
    let used_before = s.len() - val.len();
    let quote = val.chars().next().filter(|c| matches!(c, '"' | '\''));
    match quote {
        Some(q) => {
            let inner = val.get(1..).unwrap_or("");
            let end = inner.find(q).unwrap_or(inner.len());
            (name, decode(inner.get(..end).unwrap_or("")), used_before + 1 + end + 1)
        }
        None => {
            let end = val.find(|c: char| c.is_whitespace() || c == '>').unwrap_or(val.len());
            (name, decode(val.get(..end).unwrap_or("")), used_before + end)
        }
    }
}

const ENTITIES: &[(&str, &str)] = &[
    ("amp", "&"),
    ("lt", "<"),
    ("gt", ">"),
    ("quot", "\""),
    ("apos", "'"),
    ("nbsp", "\u{a0}"),
    ("copy", "\u{a9}"),
    ("reg", "\u{ae}"),
    ("trade", "\u{2122}"),
    ("mdash", "\u{2014}"),
    ("ndash", "\u{2013}"),
    ("hellip", "\u{2026}"),
    ("laquo", "\u{ab}"),
    ("raquo", "\u{bb}"),
    ("lsquo", "\u{2018}"),
    ("rsquo", "\u{2019}"),
    ("ldquo", "\u{201c}"),
    ("rdquo", "\u{201d}"),
    ("bull", "\u{2022}"),
    ("middot", "\u{b7}"),
    ("rarr", "\u{2192}"),
    ("larr", "\u{2190}"),
    ("times", "\u{d7}"),
    ("euro", "\u{20ac}"),
    ("pound", "\u{a3}"),
    ("deg", "\u{b0}"),
];

/// Character references: the common named ones and `&#N;` / `&#xN;`; anything else stays as written.
pub fn decode(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(rest.get(..at).unwrap_or(""));
        rest = rest.get(at..).unwrap_or("");
        let end = rest.get(..12.min(rest.len())).and_then(|w| w.find(';'));
        let replacement = end.and_then(|e| reference(rest.get(1..e)?));
        match (end, replacement) {
            (Some(e), Some(r)) => {
                out.push_str(&r);
                rest = rest.get(e + 1..).unwrap_or("");
            }
            _ => {
                out.push('&');
                rest = rest.get(1..).unwrap_or("");
            }
        }
    }
    out.push_str(rest);
    out
}

fn reference(name: &str) -> Option<String> {
    if let Some(num) = name.strip_prefix('#') {
        let code = match num.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => num.parse().ok()?,
        };
        return char::from_u32(code).filter(|c| *c != '\0').map(String::from);
    }
    ENTITIES.iter().find(|(k, _)| *k == name).map(|(_, v)| (*v).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(d: &Dom) -> Vec<String> {
        (0..d.nodes.len()).filter_map(|i| d.tag(i).map(str::to_string)).collect()
    }

    #[test]
    fn nests_elements_and_reads_attributes() {
        let d = parse("<!DOCTYPE html><div id=a class='x y' hidden><p>Hi &amp; <b>there</b></div>");
        assert_eq!(tags(&d), ["div", "p", "b"]);
        let div = d.find("div").unwrap();
        assert_eq!((d.attr(div, "id"), d.attr(div, "class"), d.attr(div, "hidden")), (Some("a"), Some("x y"), Some("")));
        assert_eq!(d.text_of(div), "Hi & there");
    }

    #[test]
    fn optional_end_tags_close_paragraphs_and_list_items() {
        let d = parse("<ul><li>a<li>b</ul><p>one<p>two<div>x</div>");
        let ul = d.find("ul").unwrap();
        assert_eq!(d.children(ul).len(), 2);
        assert_eq!(d.children(0).len(), 4, "ul, p, p, div are siblings");
    }

    #[test]
    fn style_and_script_are_raw_text_and_svg_is_kept_whole() {
        let d = parse("<style>a > b { color: red }</style><svg viewBox='0 0 1 1'><g><svg></svg></g></svg><span>after</span>");
        let style = d.find("style").unwrap();
        assert_eq!(d.text_of(style), "a > b { color: red }");
        let svg = d.find("svg").unwrap();
        assert!(matches!(&d.nodes[svg].kind, Kind::Element { raw: Some(r), .. } if r.ends_with("</svg>") && r.contains("<g>")));
        assert_eq!(d.children(0).len(), 3);
    }

    #[test]
    fn junk_input_still_yields_a_tree() {
        for junk in
            ["<", "<<<>>>", "</div></p>", "<a href=\"x", "<div", "<!--", "&#xZZ; &#99999999999; &;", "<p/><br/><img src=x/>", "\u{0}<\u{1f600}>"]
        {
            let d = parse(junk);
            assert!(!d.nodes.is_empty());
        }
        let deep = "<div>".repeat(5000);
        assert!(parse(&deep).nodes.len() <= MAX_DOM_NODES);
    }

    #[test]
    fn character_references_decode() {
        assert_eq!(decode("a&nbsp;b &#65;&#x42; &copy;&bogus; &amp"), "a\u{a0}b AB \u{a9}&bogus; &amp");
    }
}
