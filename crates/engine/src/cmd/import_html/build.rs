//! The conversion for `import.html`: the styled document tree becomes a tree of layers (auto
//! layout frames, text, images, rectangles), one per box, before anything is created in the
//! document. Block flow becomes a vertical auto layout, `display: flex` an auto layout in the
//! flex direction; margins become a wrapper frame with padding.

use figcraft_color::Color;
use serde_json::{Map, Value, json};

use super::Warn;
use super::css::Index;
use super::dom::{Dom, Kind as DomKind};
use super::paint;
use super::style::{self, Len, Props, Vars, len};
use super::text::{Inh, TextBuf, text_props};

/// Deepest element nesting converted; anything inside is dropped.
const MAX_DEPTH: usize = 100;
/// Tags that never draw anything.
const SKIP: &[&str] =
    &["head", "script", "style", "meta", "link", "title", "template", "noscript", "base", "source", "track", "param", "datalist", "colgroup", "col"];
/// Tags drawn as a grey rectangle, with a default size.
const PLACEHOLDERS: &[(&str, f64, f64)] = &[
    ("video", 300.0, 150.0),
    ("audio", 300.0, 54.0),
    ("canvas", 300.0, 150.0),
    ("iframe", 300.0, 150.0),
    ("object", 300.0, 150.0),
    ("embed", 300.0, 150.0),
    ("input", 180.0, 32.0),
    ("textarea", 200.0, 64.0),
    ("select", 140.0, 32.0),
    ("progress", 160.0, 16.0),
    ("meter", 80.0, 16.0),
];
const PLACEHOLDER_FILL: &str = "#E5E7EB";

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Sz {
    Fixed(f64),
    Hug,
    Fill,
}

pub enum Kind {
    Frame,
    Text,
    Rect,
    /// A raster image, base64.
    Image(String),
    /// SVG source for the SVG importer.
    Svg(String),
}

pub struct Layer {
    pub kind: Kind,
    pub name: Option<String>,
    pub text: String,
    pub w: Sz,
    pub h: Sz,
    /// Width available to the layer (for images that are wider than their container).
    pub avail: Option<f64>,
    pub props: Map<String, Value>,
    pub children: Vec<Layer>,
}

impl Layer {
    fn new(kind: Kind, name: Option<String>) -> Self {
        Layer { kind, name, text: String::new(), w: Sz::Hug, h: Sz::Hug, avail: None, props: Map::new(), children: Vec::new() }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Dir {
    Row,
    Col,
}

/// How an element sits in its parent.
#[derive(Clone, Copy)]
struct Ctx {
    /// The parent is a flex container in this direction.
    flex: Option<Dir>,
    /// Flex items stretch across the container.
    stretch: bool,
    /// Width the parent offers.
    avail: Option<f64>,
    /// The parent is as wide as its content.
    hug: bool,
    /// The parent is as tall as its content.
    hug_v: bool,
    depth: usize,
}

pub struct Styled {
    el: usize,
    props: Props,
    vars: Vars,
}

#[derive(PartialEq, Clone, Copy)]
enum Level {
    Inline,
    InlineBlock,
    Block,
}

enum Piece {
    Text(String),
    Inline(Styled),
    Item(Layer),
}

struct Flex {
    dir: Option<Dir>,
    rev: bool,
    wrap: bool,
    gap: f64,
    cross_gap: f64,
    justify: &'static str,
    align: &'static str,
    stretch: bool,
}

struct BoxModel {
    pad: [f64; 4],
    border: Option<paint::Border>,
}

impl BoxModel {
    fn extra(&self) -> (f64, f64) {
        let b = self.border.as_ref().map_or([0.0; 4], |b| b.widths);
        (self.pad[1] + self.pad[3] + b[1] + b[3], self.pad[0] + self.pad[2] + b[0] + b[2])
    }
}

struct Margins {
    e: [f64; 4],
    auto_h: bool,
}

pub struct Builder<'a> {
    pub dom: &'a Dom,
    pub index: Index,
    pub warn: Warn,
    pub vw: f64,
    /// Layers still allowed.
    pub left: usize,
    pub truncated: bool,
    /// Background of the `html` element, for a `body` that has none.
    pub canvas: Option<Color>,
}

fn is_replaced(tag: &str) -> bool {
    matches!(tag, "img" | "svg") || PLACEHOLDERS.iter().any(|p| p.0 == tag)
}

fn num(p: &Props, key: &str) -> Option<f64> {
    p.get(key)?.parse().ok().filter(|n: &f64| n.is_finite())
}

impl Builder<'_> {
    fn take(&mut self) -> bool {
        if self.left == 0 {
            self.truncated = true;
            return false;
        }
        self.left -= 1;
        true
    }

    pub fn styled(&mut self, el: usize, vars: &Vars) -> Styled {
        let (props, vars) = style::compute(self.dom, el, &self.index, vars, &mut self.warn);
        Styled { el, props, vars }
    }

    /// `display` of `p`: (none, flex, inline-level).
    fn display(&mut self, p: &Props) -> (bool, bool, bool) {
        let d = p.get("display").map(String::as_str);
        match d {
            Some("none") => (true, false, false),
            Some("flex") => (false, true, false),
            Some("inline-flex") => (false, true, true),
            Some("grid" | "inline-grid") => {
                self.warn.add("display: grid is not supported (laid out as a block)");
                (false, false, d == Some("inline-grid"))
            }
            Some("inline" | "inline-block") | None => (false, false, true),
            Some(_) => (false, false, false),
        }
    }

    fn level(&mut self, st: &Styled) -> Level {
        let tag = self.dom.tag(st.el).unwrap_or("").to_string();
        if tag == "br" {
            return Level::Inline;
        }
        let d = st.props.get("display").map(String::as_str);
        if is_replaced(&tag) && matches!(d, None | Some("inline" | "inline-block")) {
            return Level::InlineBlock;
        }
        match d {
            None | Some("inline") => {
                if self.has_visual_box(&st.props) || self.has_replaced(st.el, 3) {
                    Level::InlineBlock
                } else {
                    Level::Inline
                }
            }
            Some("inline-block" | "inline-flex" | "inline-grid") => Level::InlineBlock,
            Some(_) => Level::Block,
        }
    }

    /// An image, graphic or form control inside `el`, up to `depth` levels down.
    fn has_replaced(&self, el: usize, depth: usize) -> bool {
        depth > 0 && self.dom.children(el).iter().any(|c| self.dom.tag(*c).is_some_and(is_replaced) || self.has_replaced(*c, depth - 1))
    }

    /// A background, border, padding or side margin: an inline element with one is drawn as a box.
    fn has_visual_box(&self, p: &Props) -> bool {
        let bg = p.get("background-color").and_then(|c| Color::parse_css(c)).is_some_and(|c| c.a > 0.0);
        let img = p.get("background-image").is_some_and(|i| i != "none");
        let border = p.iter().any(|(k, v)| k.starts_with("border-") && k.ends_with("-style") && !matches!(v.as_str(), "none" | "hidden"));
        let spaced =
            p.iter().any(|(k, v)| (k.starts_with("padding-") || k == "margin-left" || k == "margin-right") && v != "0" && !v.starts_with("0px"));
        bg || img || border || spaced
    }

    /// A length property in px (negative or unsupported values count as 0).
    fn px(&mut self, p: &Props, key: &str, em: f64) -> f64 {
        let Some(v) = p.get(key) else { return 0.0 };
        match len(v, em, self.vw) {
            Some(Len::Px(n)) => n.clamp(0.0, 100_000.0),
            _ => {
                self.warn.add(format!("`{key}: {v}` is not supported (ignored)"));
                0.0
            }
        }
    }

    fn length(&mut self, p: &Props, key: &str, em: f64) -> Option<Len> {
        let v = p.get(key)?;
        let l = len(v, em, self.vw);
        if l.is_none() {
            self.warn.add(format!("`{key}: {v}` is not supported (ignored)"));
        }
        l
    }

    fn margins(&mut self, p: &Props, em: f64) -> Margins {
        let mut e = [0.0; 4];
        let mut auto = [false; 4];
        for (i, side) in ["top", "right", "bottom", "left"].iter().enumerate() {
            let key = format!("margin-{side}");
            let Some(v) = p.get(&key) else { continue };
            match len(v, em, self.vw) {
                Some(Len::Px(n)) if n >= 0.0 => {
                    if let Some(s) = e.get_mut(i) {
                        *s = n.min(100_000.0);
                    }
                }
                Some(Len::Auto) => {
                    if let Some(s) = auto.get_mut(i) {
                        *s = true;
                    }
                }
                Some(Len::Px(_)) => self.warn.add("negative margins are not supported (treated as 0)"),
                _ => self.warn.add(format!("`{key}: {v}` is not supported (ignored)")),
            }
        }
        let [_, r, _, l] = auto;
        if auto.iter().any(|a| *a) && !(r && l) {
            self.warn.add("margin: auto on one side only is not supported");
        }
        Margins { e, auto_h: r && l }
    }

    fn flex_spec(&mut self, p: &Props, inh: &Inh, flex: bool) -> Flex {
        if !flex {
            let align = match inh.align {
                "center" => "center",
                "right" => "max",
                _ => "min",
            };
            return Flex { dir: None, rev: false, wrap: false, gap: 0.0, cross_gap: 0.0, justify: "min", align, stretch: false };
        }
        let d = p.get("flex-direction").map_or("row", String::as_str);
        let dir = if d.starts_with("column") { Dir::Col } else { Dir::Row };
        let (main_gap, cross_gap) = {
            let (row, col) = (self.px(p, "row-gap", inh.size), self.px(p, "column-gap", inh.size));
            if dir == Dir::Row { (col, row) } else { (row, col) }
        };
        let justify = match p.get("justify-content").map_or("", String::as_str) {
            "center" => "center",
            "flex-end" | "end" | "right" => "max",
            "space-between" => "spaceBetween",
            "space-around" => "spaceAround",
            "space-evenly" => "spaceEvenly",
            _ => "min",
        };
        let (align, stretch) = match p.get("align-items").map_or("stretch", String::as_str) {
            "center" => ("center", false),
            "flex-end" | "end" | "self-end" => ("max", false),
            "baseline" if dir == Dir::Row => ("baseline", false),
            "flex-start" | "start" | "self-start" | "baseline" => ("min", false),
            _ => ("min", true),
        };
        Flex {
            dir: Some(dir),
            rev: d.ends_with("reverse"),
            wrap: p.get("flex-wrap").is_some_and(|w| w.starts_with("wrap")),
            gap: main_gap,
            cross_gap,
            justify,
            align,
            stretch,
        }
    }

    fn box_model(&mut self, p: &Props, inh: &Inh) -> BoxModel {
        let pad = ["top", "right", "bottom", "left"].map(|s| self.px(p, &format!("padding-{s}"), inh.size));
        let border = paint::border(p, inh.color, inh.size, self.vw, &mut self.warn);
        BoxModel { pad, border }
    }

    /// The size of an element on one axis.
    fn axis(&mut self, p: &Props, horiz: bool, inh: &Inh, extra: f64, ctx: &Ctx, inline: bool) -> Sz {
        let key = if horiz { "width" } else { "height" };
        let main = match ctx.flex {
            Some(Dir::Row) => horiz,
            Some(Dir::Col) => !horiz,
            None => false,
        };
        if main && num(p, "flex-grow").is_some_and(|g| g > 0.0) {
            // Filling a container that hugs its content has no size to fill.
            return if (horiz && ctx.hug) || (!horiz && ctx.hug_v) { Sz::Hug } else { Sz::Fill };
        }
        let add = if p.get("box-sizing").is_some_and(|b| b == "border-box") { 0.0 } else { extra };
        match self.length(p, key, inh.size) {
            Some(Len::Px(v)) => Sz::Fixed((v + add).clamp(0.0, 100_000.0)),
            Some(Len::Pct(v)) if v >= 99.99 => Sz::Fill,
            Some(Len::Pct(v)) if horiz => match ctx.avail {
                Some(a) => Sz::Fixed((a * v / 100.0 + add).max(0.0)),
                None => Sz::Fill,
            },
            Some(Len::Pct(_)) => {
                self.warn.add("percentage heights are not supported (the box hugs its content)");
                Sz::Hug
            }
            _ if horiz => self.default_width(p, inh, add, ctx, inline),
            _ => match ctx.flex {
                Some(Dir::Row) if ctx.stretch && !ctx.hug_v => Sz::Fill,
                _ => Sz::Hug,
            },
        }
    }

    fn default_width(&mut self, p: &Props, inh: &Inh, add: f64, ctx: &Ctx, inline: bool) -> Sz {
        match ctx.flex {
            Some(Dir::Row) => match self.length(p, "flex-basis", inh.size) {
                Some(Len::Px(b)) => Sz::Fixed(b + add),
                _ => Sz::Hug,
            },
            Some(Dir::Col) if ctx.stretch && !ctx.hug => Sz::Fill,
            Some(Dir::Col) => Sz::Hug,
            None if inline || ctx.hug => Sz::Hug,
            None => Sz::Fill,
        }
    }

    pub fn build_root(&mut self, root: usize, html: Option<usize>) -> Option<Layer> {
        let none = Vars::default();
        let mut inh = Inh::root();
        let mut vars = none;
        if let Some(h) = html.filter(|h| *h != root) {
            let hs = self.styled(h, &vars);
            inh = inh.derive(&hs.props, self.vw);
            self.canvas = hs.props.get("background-color").and_then(|c| Color::parse_css(c)).filter(|c| c.a > 0.0);
            vars = hs.vars;
        }
        let st = self.styled(root, &vars);
        let ctx = Ctx { flex: None, stretch: false, avail: Some(self.vw), hug: false, hug_v: false, depth: 0 };
        let mut layer = self.element(&st, ctx, &inh, true)?;
        if layer.props.get("fills").and_then(Value::as_array).is_none_or(Vec::is_empty) {
            layer.props.insert("fills".into(), json!([self.canvas.map_or("#FFFFFF".to_string(), |c| c.to_css_hex())]));
        }
        Some(layer)
    }

    /// Replaced elements and other tags that are not boxes of text: images, graphics, controls, rules.
    fn special(&mut self, st: &Styled, tag: &str, ctx: &Ctx, parent: &Inh) -> Option<Option<Layer>> {
        let inh = parent.derive(&st.props, self.vw);
        let layer = match tag {
            "img" => self.image(st, ctx),
            "svg" => self.svg(st, ctx),
            "hr" => {
                let mut l = Layer::new(Kind::Rect, Some("hr".into()));
                (l.w, l.h) = (Sz::Fill, Sz::Fixed(1.0));
                l.props.insert("fill".into(), json!("#CCCCCC"));
                l.avail = ctx.avail;
                Some(l)
            }
            t => PLACEHOLDERS.iter().find(|p| p.0 == t).map(|(_, w, h)| {
                self.warn.add(format!("tag <{t}> is not supported (drawn as a placeholder rectangle)"));
                self.placeholder(st, ctx, t, (*w, *h))
            }),
        };
        let layer = layer?;
        Some(self.finish_replaced(st, ctx, layer, &inh))
    }

    fn finish_replaced(&mut self, st: &Styled, ctx: &Ctx, mut layer: Layer, inh: &Inh) -> Option<Layer> {
        if !self.take() {
            return None;
        }
        let size = (fixed(layer.w), fixed(layer.h));
        if let Some(r) = paint::radius(&st.props, inh.size, self.vw, size, &mut self.warn) {
            layer.props.insert("cornerRadius".into(), r);
        }
        self.common_props(&st.props, &mut layer.props);
        let m = self.margins(&st.props, inh.size);
        Some(self.wrap_margin(layer, &m, ctx))
    }

    fn dims(&mut self, st: &Styled, ctx: &Ctx, em: f64) -> (Sz, Sz) {
        let attr = |name: &str| self.dom.attr(st.el, name).and_then(|v| v.trim().parse::<f64>().ok()).filter(|n| *n > 0.0 && *n <= 100_000.0);
        let (aw, ah) = (attr("width"), attr("height"));
        let w = match self.length(&st.props, "width", em) {
            Some(Len::Px(v)) => Sz::Fixed(v),
            Some(Len::Pct(p)) if p >= 99.99 => Sz::Fill,
            Some(Len::Pct(p)) => ctx.avail.map_or(Sz::Hug, |a| Sz::Fixed(a * p / 100.0)),
            _ => aw.map_or(Sz::Hug, Sz::Fixed),
        };
        let h = match self.length(&st.props, "height", em) {
            Some(Len::Px(v)) => Sz::Fixed(v),
            _ => ah.map_or(Sz::Hug, Sz::Fixed),
        };
        (w, h)
    }

    fn image(&mut self, st: &Styled, ctx: &Ctx) -> Option<Layer> {
        let src = self.dom.attr(st.el, "src").unwrap_or("");
        let name = self.dom.attr(st.el, "alt").filter(|a| !a.trim().is_empty()).map(|a| a.chars().take(60).collect::<String>());
        let (w, h) = self.dims(st, ctx, 16.0);
        let mut layer = match data_uri(src) {
            Some(Data::Raster(b64)) => Layer::new(Kind::Image(b64), name.or(Some("img".into()))),
            Some(Data::Svg(text)) => Layer::new(Kind::Svg(text), name.or(Some("img".into()))),
            None => {
                self.warn.add("<img> without a data: URI is not fetched (drawn as a placeholder rectangle)");
                let mut l = Layer::new(Kind::Rect, name.or(Some("img".into())));
                l.props.insert("fill".into(), json!(PLACEHOLDER_FILL));
                l.w = if w == Sz::Hug { Sz::Fixed(300.0) } else { w };
                l.h = if h == Sz::Hug { Sz::Fixed(150.0) } else { h };
                l.avail = ctx.avail;
                return Some(l);
            }
        };
        (layer.w, layer.h, layer.avail) = (w, h, ctx.avail);
        Some(layer)
    }

    fn svg(&mut self, st: &Styled, ctx: &Ctx) -> Option<Layer> {
        let raw = self.dom.raw(st.el)?;
        let (w, h) = self.dims(st, ctx, 16.0);
        let mut layer = Layer::new(Kind::Svg(with_size(raw, w, h)), Some("svg".into()));
        layer.avail = ctx.avail;
        Some(layer)
    }

    fn placeholder(&mut self, st: &Styled, ctx: &Ctx, tag: &str, default: (f64, f64)) -> Layer {
        let (w, h) = self.dims(st, ctx, 16.0);
        let mut l = Layer::new(Kind::Rect, Some(tag.to_string()));
        l.w = if w == Sz::Hug { Sz::Fixed(default.0) } else { w };
        l.h = if h == Sz::Hug { Sz::Fixed(default.1) } else { h };
        l.props.insert("fill".into(), json!(PLACEHOLDER_FILL));
        l.avail = ctx.avail;
        l
    }

    /// Opacity, visibility and clipping.
    fn common_props(&mut self, p: &Props, out: &mut Map<String, Value>) {
        if let Some(o) = num(p, "opacity") {
            out.insert("opacity".into(), json!(o.clamp(0.0, 1.0)));
        }
        if p.get("visibility").is_some_and(|v| v == "hidden" || v == "collapse") {
            out.insert("visible".into(), json!(false));
        }
    }

    fn element(&mut self, st: &Styled, ctx: Ctx, parent: &Inh, root: bool) -> Option<Layer> {
        if ctx.depth > MAX_DEPTH {
            self.warn.add(format!("elements nested deeper than {MAX_DEPTH} levels are dropped"));
            return None;
        }
        let tag = self.dom.tag(st.el).unwrap_or("").to_string();
        if let Some(done) = self.special(st, &tag, &ctx, parent) {
            return done;
        }
        if matches!(st.props.get("position").map(String::as_str), Some("absolute" | "fixed")) {
            self.warn.add("position: absolute and fixed are laid out in the normal flow");
        }
        let inh = parent.derive(&st.props, self.vw);
        let (_, is_flex, inline_level) = self.display(&st.props);
        let flex = self.flex_spec(&st.props, &inh, is_flex);
        let bx = self.box_model(&st.props, &inh);
        let (eh, ev) = bx.extra();
        let mut m = self.margins(&st.props, inh.size);
        let inline = inline_level && ctx.flex.is_none();
        let (mut w, mut h) = (self.axis(&st.props, true, &inh, eh, &ctx, inline), self.axis(&st.props, false, &inh, ev, &ctx, false));
        let mut pad = bx.pad;
        if root {
            // The page's margin is space inside the page frame.
            (w, h) = (Sz::Fixed(self.vw), if matches!(h, Sz::Fixed(_)) { h } else { Sz::Hug });
            for (p, e) in pad.iter_mut().zip(m.e) {
                *p += e;
            }
            m = Margins { e: [0.0; 4], auto_h: false };
        }
        // This layer's slot is taken before its children's, so a full page keeps the top of the tree.
        if !self.take() {
            return None;
        }
        let avail = self.inner_width(&st.props, w, &ctx, &m, eh);
        let cctx = Ctx { flex: flex.dir, stretch: flex.stretch, avail, hug: w == Sz::Hug, hug_v: h == Sz::Hug, depth: ctx.depth + 1 };
        let mut kids = self.children(st.el, &inh, &cctx, &st.vars);
        if flex.rev {
            kids.reverse();
        }
        let size = (fixed(w), fixed(h));
        let fills = paint::fills(&st.props, inh.color, size, &mut self.warn);
        let mut props = Map::new();
        let boxy = self.frame_props(&st.props, &inh, &mut props, (&bx, &pad), size);
        let lone_text = kids.len() == 1 && matches!(kids.first().map(|k| &k.kind), Some(Kind::Text));
        let layer = if lone_text && !root && !boxy && fills.is_empty() && flex.dir.is_none() {
            self.left += 1;
            let mut t = kids.remove(0);
            if w != Sz::Hug {
                t.w = w;
            }
            self.common_props(&st.props, &mut t.props);
            t
        } else {
            props.insert("fills".into(), json!(fills));
            self.frame_layout(&mut props, &flex, &pad);
            let mut l = Layer::new(Kind::Frame, Some(element_name(self.dom, st.el, &tag)));
            (l.w, l.h, l.props, l.children) = (w, h, props, kids);
            self.min_max(&st.props, &mut l.props, &inh, (eh, ev));
            l
        };
        let mut layer = layer;
        layer.avail = ctx.avail;
        Some(if root { layer } else { self.wrap_margin(layer, &m, &ctx) })
    }

    fn inner_width(&mut self, p: &Props, w: Sz, ctx: &Ctx, m: &Margins, eh: f64) -> Option<f64> {
        let outer = match w {
            Sz::Fixed(v) => Some(v),
            _ => ctx.avail.map(|a| a - m.e[1] - m.e[3]),
        };
        let max = match self.length(p, "max-width", 16.0) {
            Some(Len::Px(v)) => Some(v),
            _ => None,
        };
        let outer = match (outer, max) {
            (Some(o), Some(mx)) => Some(o.min(mx)),
            (o, _) => o,
        };
        outer.map(|o| (o - eh).max(0.0))
    }

    /// Border, radius, shadow, clip and opacity; whether any of them (or a fixed size, or padding) makes a visible box.
    fn frame_props(
        &mut self,
        p: &Props,
        inh: &Inh,
        out: &mut Map<String, Value>,
        bx: (&BoxModel, &[f64; 4]),
        size: (Option<f64>, Option<f64>),
    ) -> bool {
        let mut boxy = bx.1.iter().any(|v| *v > 0.0) || size.0.is_some() || size.1.is_some();
        if let Some(b) = &bx.0.border {
            b.props(out);
            boxy = true;
        }
        if let Some(r) = paint::radius(p, inh.size, self.vw, size, &mut self.warn) {
            out.insert("cornerRadius".into(), r);
        }
        if let Some(s) = p.get("box-shadow") {
            let fx = paint::shadows(s, inh.color, inh.size, self.vw, &mut self.warn);
            boxy |= !fx.is_empty();
            out.insert("effects".into(), json!(fx));
        }
        let clip = p.get("overflow").is_some_and(|o| o == "hidden");
        boxy |= clip;
        out.insert("clip".into(), json!(clip));
        self.common_props(p, out);
        boxy | out.contains_key("opacity") | out.contains_key("visible")
    }

    fn frame_layout(&mut self, props: &mut Map<String, Value>, f: &Flex, pad: &[f64; 4]) {
        props.insert("layout".into(), json!(if f.dir == Some(Dir::Row) { "horizontal" } else { "vertical" }));
        props.insert("gap".into(), json!(f.gap));
        if f.wrap {
            props.insert("wrap".into(), json!(true));
            props.insert("crossGap".into(), json!(f.cross_gap));
        }
        props.insert("mainAlign".into(), json!(f.justify));
        props.insert("crossAlign".into(), json!(f.align));
        if pad.iter().any(|v| *v > 0.0) {
            props.insert("padding".into(), json!(pad));
        }
    }

    fn min_max(&mut self, p: &Props, out: &mut Map<String, Value>, inh: &Inh, extra: (f64, f64)) {
        let add = if p.get("box-sizing").is_some_and(|b| b == "border-box") { (0.0, 0.0) } else { extra };
        for (key, prop, add) in [
            ("min-width", "minWidth", add.0),
            ("max-width", "maxWidth", add.0),
            ("min-height", "minHeight", add.1),
            ("max-height", "maxHeight", add.1),
        ] {
            match self.length(p, key, inh.size) {
                Some(Len::Px(v)) => {
                    out.insert(prop.into(), json!(v + add));
                }
                Some(Len::Pct(_)) | Some(Len::Auto) | None => {}
            }
        }
    }

    /// Margins as a frame around `layer` whose padding is the margin; `margin: 0 auto` centres a block.
    fn wrap_margin(&mut self, layer: Layer, m: &Margins, ctx: &Ctx) -> Layer {
        let centre = m.auto_h && ctx.flex.is_none();
        if m.auto_h && ctx.flex.is_some() {
            self.warn.add("margin: auto inside a flex container is not supported");
        }
        if m.e.iter().all(|v| *v == 0.0) && !centre {
            return layer;
        }
        if !self.take() {
            return layer;
        }
        let name = layer.name.as_ref().map_or("margin".to_string(), |n| format!("{n} margin"));
        let mut wrapper = Layer::new(Kind::Frame, Some(name));
        wrapper.props.insert("fills".into(), json!([]));
        wrapper.props.insert("clip".into(), json!(false));
        wrapper.props.insert("layout".into(), json!(if centre { "horizontal" } else { "vertical" }));
        wrapper.props.insert("gap".into(), json!(0));
        wrapper.props.insert("crossAlign".into(), json!("min"));
        wrapper.props.insert("mainAlign".into(), json!(if centre { "center" } else { "min" }));
        wrapper.props.insert("padding".into(), json!(m.e));
        let hug_unless_fill = |s: Sz| if s == Sz::Fill { Sz::Fill } else { Sz::Hug };
        wrapper.w = if centre { Sz::Fill } else { hug_unless_fill(layer.w) };
        wrapper.h = hug_unless_fill(layer.h);
        wrapper.avail = layer.avail;
        wrapper.children.push(layer);
        wrapper
    }

    /// The layers for the children of `parent`: blocks and flex items one each, inline content gathered into text.
    fn children(&mut self, parent: usize, inh: &Inh, ctx: &Ctx, vars: &Vars) -> Vec<Layer> {
        let dom = self.dom;
        let (mut out, mut run) = (Vec::new(), Vec::new());
        for &c in dom.children(parent) {
            if self.left == 0 {
                self.truncated = true;
                break;
            }
            match dom.nodes.get(c).map(|n| &n.kind) {
                Some(DomKind::Text(t)) => run.push(Piece::Text(t.clone())),
                Some(DomKind::Element { .. }) => self.child_element(c, &mut run, &mut out, inh, ctx, vars),
                _ => {}
            }
        }
        self.flush(&mut run, &mut out, inh, ctx);
        out
    }

    fn child_element(&mut self, c: usize, run: &mut Vec<Piece>, out: &mut Vec<Layer>, inh: &Inh, ctx: &Ctx, vars: &Vars) {
        let tag = self.dom.tag(c).unwrap_or("");
        if SKIP.contains(&tag) || self.dom.attr(c, "hidden").is_some() || (tag == "br" && ctx.flex.is_some()) {
            return;
        }
        let st = self.styled(c, vars);
        if st.props.get("display").is_some_and(|d| d == "none") {
            return;
        }
        let level = if ctx.flex.is_some() { Level::Block } else { self.level(&st) };
        match level {
            Level::Inline => run.push(Piece::Inline(st)),
            Level::InlineBlock => run.extend(self.element(&st, *ctx, inh, false).map(Piece::Item)),
            Level::Block => {
                self.flush(run, out, inh, ctx);
                out.extend(self.element(&st, *ctx, inh, false));
            }
        }
    }

    /// Turn the pending inline run into layers: one text layer, or a wrapping row when it holds boxes.
    fn flush(&mut self, run: &mut Vec<Piece>, out: &mut Vec<Layer>, inh: &Inh, ctx: &Ctx) {
        if run.is_empty() {
            return;
        }
        let items = run.iter().any(|p| matches!(p, Piece::Item(_)));
        let wants_hug = items || ctx.hug || ctx.flex == Some(Dir::Row) || (ctx.flex == Some(Dir::Col) && !ctx.stretch);
        let (mut line, mut buf) = (Vec::new(), TextBuf::default());
        for piece in std::mem::take(run) {
            match piece {
                Piece::Text(t) => buf.push(&t, inh),
                Piece::Inline(st) => self.collect_inline(&mut buf, &st, inh),
                Piece::Item(l) => {
                    line.extend(self.text_layer(&mut buf, inh, ctx, wants_hug));
                    line.push(l);
                }
            }
        }
        line.extend(self.text_layer(&mut buf, inh, ctx, wants_hug));
        if !items || line.len() < 2 {
            out.extend(line);
        } else if self.take() {
            let mut row = Layer::new(Kind::Frame, Some("line".into()));
            row.w = if ctx.hug { Sz::Hug } else { Sz::Fill };
            row.avail = ctx.avail;
            let align = match inh.align {
                "center" => "center",
                "right" => "max",
                _ => "min",
            };
            for (k, v) in [
                ("fills", json!([])),
                ("clip", json!(false)),
                ("layout", json!("horizontal")),
                ("wrap", json!(true)),
                ("gap", json!(4)),
                ("crossGap", json!(4)),
                ("crossAlign", json!("center")),
                ("mainAlign", json!(align)),
            ] {
                row.props.insert(k.into(), v);
            }
            row.children = line;
            out.push(row);
        } else {
            out.extend(line);
        }
    }

    fn collect_inline(&mut self, buf: &mut TextBuf, st: &Styled, parent: &Inh) {
        let inh = parent.derive(&st.props, self.vw);
        if self.dom.tag(st.el) == Some("br") {
            buf.line_break(&inh);
            return;
        }
        let dom = self.dom;
        for &c in dom.children(st.el) {
            match dom.nodes.get(c).map(|n| &n.kind) {
                Some(DomKind::Text(t)) => buf.push(t, &inh),
                Some(DomKind::Element { tag, .. }) if !SKIP.contains(&tag.as_str()) && dom.attr(c, "hidden").is_none() => {
                    let cs = self.styled(c, &st.vars);
                    if cs.props.get("display").is_none_or(|d| d != "none") {
                        self.collect_inline(buf, &cs, &inh);
                    }
                }
                _ => {}
            }
        }
    }

    fn text_layer(&mut self, buf: &mut TextBuf, inh: &Inh, ctx: &Ctx, wants_hug: bool) -> Option<Layer> {
        let mut b = std::mem::take(buf);
        if !b.finish() || !self.take() {
            return None;
        }
        let long = b.longest_line() as f64 * inh.size * 0.5;
        let hug = wants_hug && (inh.nowrap || ctx.avail.is_none_or(|a| long <= a));
        let mut l = Layer::new(Kind::Text, None);
        l.text = b.text();
        l.w = if hug { Sz::Hug } else { Sz::Fill };
        l.avail = ctx.avail;
        text_props(inh, &mut l.props);
        let ranges = b.ranges(inh);
        if !ranges.is_empty() {
            l.props.insert("ranges".into(), Value::Array(ranges));
        }
        Some(l)
    }
}

fn fixed(s: Sz) -> Option<f64> {
    match s {
        Sz::Fixed(v) => Some(v),
        _ => None,
    }
}

fn element_name(dom: &Dom, el: usize, tag: &str) -> String {
    if tag.is_empty() {
        return "Page".into();
    }
    if let Some(id) = dom.attr(el, "id").filter(|i| !i.is_empty()) {
        return format!("{tag}#{id}");
    }
    match dom.attr(el, "class").and_then(|c| c.split_whitespace().next()) {
        Some(c) => format!("{tag}.{c}"),
        None => tag.to_string(),
    }
}

enum Data {
    Raster(String),
    Svg(String),
}

/// What a `data:` URI holds: base64 raster data, or SVG source (base64 or percent-encoded).
fn data_uri(src: &str) -> Option<Data> {
    use base64::Engine as _;
    let rest = src.trim().strip_prefix("data:")?;
    let (head, payload) = rest.split_once(',')?;
    let b64 = head.ends_with(";base64");
    let mime = head.split(';').next().unwrap_or("").to_ascii_lowercase();
    if mime == "image/svg+xml" {
        let text = if b64 {
            let bytes = base64::engine::general_purpose::STANDARD.decode(payload.split_whitespace().collect::<String>()).ok()?;
            String::from_utf8(bytes).ok()?
        } else {
            percent_decode(payload)
        };
        return Some(Data::Svg(text));
    }
    (b64 && mime.starts_with("image/")).then(|| Data::Raster(payload.split_whitespace().collect()))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        let hex = |k: usize| b.get(k).and_then(|h| (*h as char).to_digit(16));
        match (c, hex(i + 1), hex(i + 2)) {
            (b'%', Some(h), Some(l)) => {
                out.push((h * 16 + l) as u8);
                i += 3;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `raw` (an `<svg>`) with the `width`/`height` the page gives it, unless the tag has its own.
fn with_size(raw: &str, w: Sz, h: Sz) -> String {
    let Some(end) = raw.find('>') else { return raw.to_string() };
    let open = raw.get(..end).unwrap_or("").to_ascii_lowercase();
    let mut attrs = String::new();
    for (name, v) in [("width", w), ("height", h)] {
        if let Sz::Fixed(px) = v
            && !open.contains(&format!(" {name}="))
        {
            attrs.push_str(&format!(" {name}=\"{px}\""));
        }
    }
    // After `<svg`, which is 4 bytes.
    format!("{}{}{}", raw.get(..4).unwrap_or(""), attrs, raw.get(4..).unwrap_or(""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_uris() {
        assert!(matches!(data_uri("data:image/png;base64,AAAA"), Some(Data::Raster(b)) if b == "AAAA"));
        assert!(matches!(data_uri("data:image/svg+xml;utf8,%3Csvg%3E"), Some(Data::Svg(s)) if s == "<svg>"));
        assert!(data_uri("https://x/y.png").is_none());
        assert!(data_uri("data:text/html;base64,AAAA").is_none());
        assert!(data_uri("data:image/png,AAAA").is_none());
    }

    #[test]
    fn svg_gets_the_size_the_page_gives_it() {
        assert_eq!(with_size("<svg viewBox='0 0 1 1'></svg>", Sz::Fixed(24.0), Sz::Hug), "<svg width=\"24\" viewBox='0 0 1 1'></svg>");
        assert_eq!(with_size("<svg width=\"5\"></svg>", Sz::Fixed(24.0), Sz::Hug), "<svg width=\"5\"></svg>");
    }
}
