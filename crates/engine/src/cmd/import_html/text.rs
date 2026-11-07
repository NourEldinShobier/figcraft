//! Text for `import.html`: the inherited text properties, the collapsed text of an inline run
//! with its style ranges, and the text layer's properties.

use figcraft_color::Color;
use serde_json::{Map, Value, json};

use super::style::{Len, Props, len};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum LineHeight {
    Normal,
    Mult(f64),
    Px(f64),
}

/// The properties text inherits.
#[derive(Clone, PartialEq, Debug)]
pub struct Inh {
    pub color: Color,
    pub family: Option<String>,
    pub size: f64,
    pub weight: u16,
    pub italic: bool,
    pub line_height: LineHeight,
    pub align: &'static str,
    pub transform: &'static str,
    pub spacing: f64,
    pub deco: &'static str,
    /// `white-space: pre*`: line breaks and runs of spaces stay.
    pub pre: bool,
    pub nowrap: bool,
}

impl Inh {
    pub fn root() -> Self {
        Inh {
            color: Color::new(0.0, 0.0, 0.0, 1.0),
            family: None,
            size: 16.0,
            weight: 400,
            italic: false,
            line_height: LineHeight::Normal,
            align: "left",
            transform: "original",
            spacing: 0.0,
            deco: "none",
            pre: false,
            nowrap: false,
        }
    }

    /// What an element with the declarations `p` inherits from its parent's `self`.
    pub fn derive(&self, p: &Props, vw: f64) -> Inh {
        let mut i = self.clone();
        if let Some(c) = p.get("color").and_then(|v| Color::parse_css(v)) {
            i.color = c;
        }
        if let Some(v) = p.get("font-size") {
            i.size = font_size(v, self.size, vw).unwrap_or(self.size);
        }
        if let Some(v) = p.get("font-weight") {
            i.weight = match v.as_str() {
                "bold" | "bolder" => 700,
                "normal" => 400,
                "lighter" => 300,
                n => n.parse().ok().filter(|w| (1..=1000).contains(w)).unwrap_or(self.weight),
            };
        }
        if let Some(v) = p.get("font-style") {
            i.italic = matches!(v.as_str(), "italic" | "oblique");
        }
        if let Some(v) = p.get("font-family") {
            i.family = family(v).or_else(|| self.family.clone());
        }
        if let Some(v) = p.get("line-height") {
            i.line_height = line_height(v, i.size, vw).unwrap_or(self.line_height);
        }
        if let Some(v) = p.get("text-align") {
            i.align = match v.as_str() {
                "center" => "center",
                "right" | "end" => "right",
                "justify" => "justified",
                _ => "left",
            };
        }
        if let Some(v) = p.get("text-transform") {
            i.transform = match v.as_str() {
                "uppercase" => "upper",
                "lowercase" => "lower",
                "capitalize" => "title",
                _ => "original",
            };
        }
        if let Some(v) = p.get("letter-spacing") {
            i.spacing = if v == "normal" { 0.0 } else { len_px(v, i.size, vw).unwrap_or(self.spacing) };
        }
        if let Some(v) = p.get("text-decoration") {
            i.deco = if v.contains("underline") {
                "underline"
            } else if v.contains("line-through") {
                "strikethrough"
            } else {
                "none"
            };
        }
        if let Some(v) = p.get("white-space") {
            i.pre = v.starts_with("pre");
            i.nowrap = v == "nowrap" || v == "pre";
        }
        i
    }

    /// The style name ("Bold Italic") for this weight and slant.
    fn style_name(&self) -> String {
        crate::cmd::props::styled_name(crate::cmd::props::weight_name(self.weight), self.italic)
    }
}

fn len_px(v: &str, em: f64, vw: f64) -> Option<f64> {
    match len(v, em, vw)? {
        Len::Px(n) => Some(n),
        _ => None,
    }
}

fn font_size(v: &str, parent: f64, vw: f64) -> Option<f64> {
    let kw = match v {
        "xx-small" => Some(9.0),
        "x-small" => Some(10.0),
        "small" => Some(13.0),
        "medium" => Some(16.0),
        "large" => Some(18.0),
        "x-large" => Some(24.0),
        "xx-large" => Some(32.0),
        "smaller" => Some(parent / 1.2),
        "larger" => Some(parent * 1.2),
        _ => None,
    };
    let size = kw.or_else(|| match len(v, parent, vw)? {
        Len::Px(n) => Some(n),
        Len::Pct(p) => Some(parent * p / 100.0),
        Len::Auto => None,
    })?;
    (size > 0.0).then_some(size.min(5000.0))
}

fn line_height(v: &str, size: f64, vw: f64) -> Option<LineHeight> {
    if v == "normal" {
        return Some(LineHeight::Normal);
    }
    if let Ok(n) = v.parse::<f64>() {
        return Some(LineHeight::Mult(n));
    }
    match len(v, size, vw)? {
        Len::Px(n) => Some(LineHeight::Px(n)),
        Len::Pct(p) => Some(LineHeight::Mult(p / 100.0)),
        Len::Auto => None,
    }
}

/// The first family of a `font-family` list that names a font (not a generic keyword).
fn family(v: &str) -> Option<String> {
    const GENERIC: &[&str] = &[
        "sans-serif",
        "serif",
        "monospace",
        "cursive",
        "fantasy",
        "system-ui",
        "ui-sans-serif",
        "ui-serif",
        "ui-monospace",
        "-apple-system",
        "blinkmacsystemfont",
        "inherit",
        "initial",
    ];
    super::css::split_top(v, ',')
        .into_iter()
        .map(|f| f.trim().trim_matches(|c| c == '"' || c == '\'').trim().to_string())
        .find(|f| !f.is_empty() && !GENERIC.contains(&f.to_ascii_lowercase().as_str()))
}

/// The collapsed text of an inline run, each character with the style it has.
#[derive(Default)]
pub struct TextBuf {
    chars: Vec<char>,
    style: Vec<usize>,
    pub styles: Vec<Inh>,
}

impl TextBuf {
    fn style_index(&mut self, inh: &Inh) -> usize {
        match self.styles.iter().position(|s| s == inh) {
            Some(i) => i,
            None => {
                self.styles.push(inh.clone());
                self.styles.len() - 1
            }
        }
    }

    pub fn push(&mut self, text: &str, inh: &Inh) {
        let st = self.style_index(inh);
        for c in text.chars() {
            let c = if c == '\r' { continue } else { c };
            let space = c.is_ascii_whitespace();
            let keep = if inh.pre { true } else { !space || self.chars.last().is_some_and(|l| *l != ' ' && *l != '\n') };
            if keep {
                self.chars.push(if space && !inh.pre { ' ' } else { c });
                self.style.push(st);
            }
        }
    }

    pub fn line_break(&mut self, inh: &Inh) {
        while self.chars.last() == Some(&' ') {
            self.chars.pop();
            self.style.pop();
        }
        let st = self.style_index(inh);
        self.chars.push('\n');
        self.style.push(st);
    }

    /// The text without its trailing spaces and line breaks; false when nothing is left.
    pub fn finish(&mut self) -> bool {
        while matches!(self.chars.last(), Some(' ' | '\n')) {
            self.chars.pop();
            self.style.pop();
        }
        !self.chars.is_empty()
    }

    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    /// Longest line, in characters.
    pub fn longest_line(&self) -> usize {
        self.text().lines().map(|l| l.chars().count()).max().unwrap_or(0)
    }

    /// Style ranges for the characters whose style is not `base`, the style of the whole layer.
    pub fn ranges(&self, base: &Inh) -> Vec<Value> {
        const MAX_RANGES: usize = 200;
        let mut out = Vec::new();
        let mut i = 0;
        while i < self.style.len() && out.len() < MAX_RANGES {
            let st = self.style.get(i).copied().unwrap_or(0);
            let mut j = i;
            while self.style.get(j) == Some(&st) {
                j += 1;
            }
            if let Some(s) = self.styles.get(st).filter(|s| *s != base) {
                out.push(range_of(i, j, s, base));
            }
            i = j;
        }
        out
    }
}

fn range_of(start: usize, end: usize, s: &Inh, base: &Inh) -> Value {
    let mut r = Map::new();
    r.insert("start".into(), json!(start));
    r.insert("end".into(), json!(end));
    if s.color != base.color {
        r.insert("fill".into(), json!(s.color.to_css_hex()));
    }
    if s.weight != base.weight || s.italic != base.italic {
        r.insert("fontStyle".into(), json!(s.style_name()));
    }
    if s.size != base.size {
        r.insert("fontSize".into(), json!(s.size));
    }
    if let Some(f) = s.family.as_ref().filter(|f| base.family.as_ref() != Some(*f)) {
        r.insert("fontFamily".into(), json!(f));
    }
    if s.deco != base.deco {
        r.insert("textDecoration".into(), json!(s.deco));
    }
    Value::Object(r)
}

/// Properties of a text layer in the style `i`.
pub fn text_props(i: &Inh, props: &mut Map<String, Value>) {
    props.insert("fill".into(), json!(i.color.to_css_hex()));
    props.insert("fontSize".into(), json!(i.size));
    if let Some(f) = &i.family {
        props.insert("fontFamily".into(), json!(f));
    }
    props.insert("fontStyle".into(), json!(i.style_name()));
    match i.line_height {
        LineHeight::Normal => {}
        LineHeight::Mult(m) => {
            props.insert("lineHeight".into(), json!(format!("{}%", (m * 10000.0).round() / 100.0)));
        }
        LineHeight::Px(p) => {
            props.insert("lineHeight".into(), json!(p));
        }
    }
    props.insert("textAlign".into(), json!(i.align));
    if i.transform != "original" {
        props.insert("textCase".into(), json!(i.transform));
    }
    if i.spacing != 0.0 {
        props.insert("letterSpacing".into(), json!(i.spacing));
    }
    if i.deco != "none" {
        props.insert("textDecoration".into(), json!(i.deco));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitespace_collapses_and_breaks_stay() {
        let base = Inh::root();
        let mut b = TextBuf::default();
        b.push("  Hello \n  world ", &base);
        b.line_break(&base);
        b.push("  next", &base);
        assert!(b.finish());
        assert_eq!(b.text(), "Hello world\nnext");
        let mut pre = TextBuf::default();
        let p = Inh { pre: true, ..base };
        pre.push("a  b\r\n c\n", &p);
        assert!(pre.finish());
        assert_eq!(pre.text(), "a  b\n c");
    }

    #[test]
    fn ranges_cover_the_characters_in_another_style() {
        let base = Inh::root();
        let bold = Inh { weight: 700, ..base.clone() };
        let red = Inh { color: Color::new(1.0, 0.0, 0.0, 1.0), ..base.clone() };
        let mut b = TextBuf::default();
        b.push("one ", &base);
        b.push("two", &bold);
        b.push(" three ", &base);
        b.push("four", &red);
        assert!(b.finish());
        let r = b.ranges(&base);
        assert_eq!(r, vec![json!({"start": 4, "end": 7, "fontStyle": "Bold"}), json!({"start": 14, "end": 18, "fill": "#FF0000"})]);
    }

    #[test]
    fn inheritance_reads_the_text_properties() {
        let mut p = Props::new();
        for (k, v) in [
            ("font-size", "2em"),
            ("font-weight", "bold"),
            ("line-height", "1.5"),
            ("text-align", "center"),
            ("font-family", "\"Fira Sans\", sans-serif"),
            ("color", "#336699"),
        ] {
            p.insert(k.to_string(), v.to_string());
        }
        let i = Inh::root().derive(&p, 1000.0);
        assert_eq!((i.size, i.weight, i.align), (32.0, 700, "center"));
        assert_eq!((i.family.as_deref(), i.line_height, i.color.to_css_hex().as_str()), (Some("Fira Sans"), LineHeight::Mult(1.5), "#336699"));
        let mut q = Props::new();
        q.insert("font-size".into(), "50%".into());
        assert_eq!(i.derive(&q, 1000.0).size, 16.0);
    }
}
