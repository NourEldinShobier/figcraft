//! `import.html`: static HTML and CSS to editable layers, best effort and bounded. The page is
//! parsed by a small tolerant parser, styled by a hand-written cascade (tag, `.class`, `#id` and
//! descendant selectors, the `style` attribute, custom properties) and converted box by box:
//! block flow to vertical auto layout frames, `display: flex` to auto layout frames with gap,
//! padding and alignment, text to text layers, `data:` images to image fills, inline `<svg>`
//! through the SVG importer. Nothing is fetched; what cannot be represented is listed in
//! `warnings`.

mod build;
mod css;
mod dom;
mod paint;
mod style;
mod text;

use std::collections::HashSet;

use figcraft_doc::NodeId;
use serde_json::{Map, Value, json};

use super::CommandSpec;
use crate::params::{get, num, opt_id, string};
use crate::{EngineError, Result, Session, bad, spec};
use build::{Abs, Builder, Kind, Layer, Sz};

/// Largest page accepted, in bytes.
pub const MAX_INPUT: usize = 2 * 1024 * 1024;
/// Most layers created.
pub const MAX_LAYERS: usize = 5000;
const DEFAULT_WIDTH: f64 = 1440.0;
const MAX_WARNINGS: usize = 100;

/// The warnings, each once, and the work that waits until every layer exists (it is carried here
/// because the warnings go wherever the layers are placed).
#[derive(Default)]
pub struct Warn {
    pub list: Vec<String>,
    seen: HashSet<String>,
    post: Post,
}

/// An absolutely positioned layer: its parent, its offsets and whether its size is its own.
struct AbsPlace {
    parent: NodeId,
    child: NodeId,
    at: Abs,
    w_fixed: bool,
    h_fixed: bool,
}

/// Placement that needs the layers' laid-out sizes.
#[derive(Default)]
struct Post {
    abs: Vec<AbsPlace>,
    /// Rows that hug their height, with the children that fill it once it is known.
    stretch: Vec<Vec<NodeId>>,
}

impl Warn {
    pub fn add(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        if self.list.len() < MAX_WARNINGS && self.seen.insert(msg.clone()) {
            self.list.push(msg);
        } else if self.list.len() == MAX_WARNINGS && self.seen.insert("more".into()) {
            self.list.push(format!("more than {MAX_WARNINGS} warnings: the rest are not listed"));
        }
    }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![spec!(
        "import.html",
        "Import HTML",
        "{html? (a page or fragment, at most 2 MB), path? (an .html file inside the allowed read roots; one of html and path is required), parent? (a frame or the page: default the open page), x?, y? (default: beside the existing layers), width? (viewport width in px, default 1440)}  imports static HTML + CSS as layers, one undo step: block flow becomes a vertical auto layout frame, display:flex an auto layout frame (flex-direction, gap, justify-content, align-items, flex-wrap, flex-grow), text a text layer, <img> with a data: URI an image, inline <svg> vector layers. <style> blocks and style attributes apply (tag, .class, #id, descendant, :first-child, :last-child and :nth-child(n) selectors; custom properties; @media with max-width/min-width, evaluated at the viewport width). Supported properties: display (block, flex, grid, inline, none), grid-template-columns/rows (px, fr, auto, repeat(n, ...)), gap, grid-column/grid-row (line numbers and spans), position absolute/relative with top/right/bottom/left/inset inside a positioned parent, align-items (stretch fills the cross axis of rows), width/height/min/max in px, % and em, margin, padding, background(-color, linear-gradient), border, border-radius, color, font, line-height, text-align, text-transform, letter-spacing, text-decoration, opacity, box-shadow, overflow. Nothing is fetched: other images become grey rectangles. Never fails on unsupported input: → {rootId, created (layers), warnings: [unsupported property, tag or selector, ...], truncated?} at most 5000 layers; dryRun previews",
        true,
        import_html
    )]
}

/// A page read from a file inside the allowed read roots (at most [`MAX_INPUT`] bytes).
#[cfg(not(target_arch = "wasm32"))]
fn read_html(path: &str) -> Result<String> {
    let bytes = crate::file_access::read_capped(path, MAX_INPUT as u64)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(target_arch = "wasm32")]
fn read_html(path: &str) -> Result<String> {
    Err(bad("import.html", format!("{path}: files can't be read in the browser, pass `html`")))
}

fn import_html(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "import.html";
    let html = match (get(p, "html").filter(|v| !v.is_null()), string(p, "path")) {
        (Some(_), Some(_)) => return Err(bad(CMD, "pass `html` or `path`, not both")),
        (Some(Value::String(h)), None) => h.clone(),
        (Some(_), None) => return Err(bad(CMD, "`html` must be a string")),
        (None, Some(path)) => read_html(&path)?,
        (None, None) => return Err(bad(CMD, "`html` (the page) or `path` (an .html file) is required")),
    };
    if html.len() > MAX_INPUT {
        return Err(bad(CMD, format!("`html` is {} bytes: at most {MAX_INPUT} (2 MB) are accepted", html.len())));
    }
    if html.trim().is_empty() {
        return Err(bad(CMD, "`html` is empty"));
    }
    let vw = match get(p, "width") {
        None | Some(Value::Null) => DEFAULT_WIDTH,
        Some(v) => v.as_f64().filter(|w| (1.0..=20000.0).contains(w)).ok_or_else(|| bad(CMD, "`width` must be a number from 1 to 20000"))?,
    };
    let parent = opt_id(CMD, p, "parent")?;
    for k in ["x", "y"] {
        if get(p, k).is_some_and(|v| !v.is_null() && v.as_f64().is_none_or(|n| !n.is_finite())) {
            return Err(bad(CMD, format!("`{k}` must be a number")));
        }
    }

    let (root_layer, warn, truncated) = convert(&html, vw);
    let Some(layer) = root_layer else { return Err(bad(CMD, "nothing in the page could be imported")) };
    let page = s.page()?;
    let parent = parent.unwrap_or(page);
    let pos = match (num(p, "x"), num(p, "y")) {
        (None, None) if parent == page => super::create::free_spot(s, vw, 800.0)?,
        (x, y) => (x.unwrap_or(0.0), y.unwrap_or(0.0)),
    };
    let mut warn = warn;
    let id = place(s, &layer, parent, pos, &mut warn)?;
    finish(s, id, std::mem::take(&mut warn.post))?;
    s.ds_mut()?.selection = vec![id];
    let created = count(s.doc()?, id);
    let mut out = json!({"rootId": id, "created": created, "warnings": warn.list});
    if truncated {
        out["truncated"] = json!(true);
        if let Some(w) = out["warnings"].as_array_mut() {
            w.push(json!(format!("the page has more than {MAX_LAYERS} layers: the rest was not imported")));
        }
    }
    Ok(out)
}

/// Parse and convert `html`: the layer tree, the warnings and whether the layer cap cut the page short.
fn convert(html: &str, vw: f64) -> (Option<Layer>, Warn, bool) {
    let dom = dom::parse(html);
    let mut warn = Warn::default();
    let mut rules = Vec::new();
    for i in 0..dom.nodes.len() {
        match dom.tag(i) {
            Some("style") => css::parse_sheet(&dom.text_of(i), &mut rules, &mut warn, vw),
            Some("link") if dom.attr(i, "rel").is_some_and(|r| r.to_ascii_lowercase().contains("stylesheet")) => {
                warn.add("<link rel=stylesheet> is not fetched (style sheets must be in <style> blocks)");
            }
            _ => {}
        }
    }
    if dom.truncated {
        warn.add(format!("the page has more than {} nodes: the rest is ignored", dom::MAX_DOM_NODES));
    }
    let (body, html_el) = (dom.find("body"), dom.find("html"));
    let root = body.or(html_el).unwrap_or(0);
    let title = dom.find("title").map(|t| dom.text_of(t).split_whitespace().collect::<Vec<_>>().join(" ")).filter(|t| !t.is_empty());
    let mut b = Builder { dom: &dom, index: css::Index::new(rules), warn, vw, left: MAX_LAYERS, truncated: false, canvas: None };
    let mut layer = b.build_root(root, html_el);
    if let Some(l) = layer.as_mut() {
        l.name = Some(title.map_or_else(|| "HTML".to_string(), |t| t.chars().take(60).collect()));
    }
    (layer, b.warn, b.truncated)
}

fn count(doc: &figcraft_doc::Document, id: NodeId) -> usize {
    let (mut n, mut stack) = (0, vec![id]);
    while let Some(i) = stack.pop() {
        n += 1;
        if let Some(node) = doc.node(i) {
            stack.extend(node.children.iter().copied());
        }
    }
    n
}

fn sizing(s: Sz) -> &'static str {
    match s {
        Sz::Fixed(_) => "fixed",
        Sz::Hug => "hug",
        Sz::Fill => "fill",
    }
}

fn fixed(s: Sz) -> Option<f64> {
    match s {
        Sz::Fixed(v) => Some(v),
        _ => None,
    }
}

/// Create `l` and its children under `parent` at (x, y); returns its id.
fn place(s: &mut Session, l: &Layer, parent: NodeId, at: (f64, f64), warn: &mut Warn) -> Result<NodeId> {
    let mut props = l.props.clone();
    let sized = |props: &mut Map<String, Value>| {
        props.insert("sizingH".into(), json!(sizing(l.w)));
        props.insert("sizingV".into(), json!(sizing(l.h)));
    };
    let id = match &l.kind {
        Kind::Image(b64) => return place_image(s, l, b64, parent, at, warn),
        Kind::Svg(src) => return place_svg(s, l, src, parent, at, warn),
        Kind::Frame | Kind::Rect => {
            sized(&mut props);
            create(s, if matches!(l.kind, Kind::Frame) { "frame" } else { "rectangle" }, l, parent, at, props, None)?
        }
        Kind::Text => {
            sized(&mut props);
            props.insert("textAutoResize".into(), json!(if l.w == Sz::Hug { "widthAndHeight" } else { "height" }));
            create(s, "text", l, parent, at, props, Some(&l.text))?
        }
    };
    let mut fills = Vec::new();
    for child in &l.children {
        let cid = place(s, child, id, (0.0, 0.0), warn)?;
        if let Some(at) = child.abs {
            warn.post.abs.push(AbsPlace { parent: id, child: cid, at, w_fixed: fixed(child.w).is_some(), h_fixed: fixed(child.h).is_some() });
        } else if l.stretch && child.stretchable() {
            fills.push(cid);
        }
    }
    if !fills.is_empty() {
        warn.post.stretch.push(fills);
    }
    Ok(id)
}

/// Lay out the frames under `root` bottom-up, as the engine does after every command.
fn layout_subtree(doc: &mut figcraft_doc::Document, root: NodeId) {
    let (mut frames, mut stack) = (Vec::new(), vec![(root, 0usize)]);
    while let Some((id, depth)) = stack.pop() {
        let Some(n) = doc.node(id) else { continue };
        if n.layout.is_some() {
            frames.push((depth, id));
        }
        stack.extend(n.children.iter().map(|c| (*c, depth + 1)));
    }
    frames.sort_by_key(|(depth, _)| std::cmp::Reverse(*depth));
    for (_, id) in frames {
        figcraft_layout::layout_frame(doc, id, &crate::measure::TextMeasure);
    }
}

fn props_of(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// Work that needs sizes: absolute layers leave the flow and are placed from their parent's edges,
/// and rows that hug their height are fixed at their tallest child's height so that the children
/// can fill it (a hugging parent can't be filled).
fn finish(s: &mut Session, root: NodeId, post: Post) -> Result<()> {
    const CMD: &str = "import.html";
    if post.abs.is_empty() && post.stretch.is_empty() {
        return Ok(());
    }
    for p in &post.abs {
        super::props::apply(CMD, s.doc_mut()?, p.child, &props_of(json!({"absolute": true})))?;
    }
    layout_subtree(s.doc_mut()?, root);
    // Filling a hugging row fixes it at the height it has now.
    for kids in &post.stretch {
        for k in kids {
            super::props::apply(CMD, s.doc_mut()?, *k, &props_of(json!({"sizingV": "fill"})))?;
        }
    }
    for p in &post.abs {
        let doc = s.doc_mut()?;
        let size = |id: NodeId| doc.node(id).map_or((0.0, 0.0), |n| (n.width, n.height));
        let ((pw, ph), (cw, ch)) = (size(p.parent), size(p.child));
        let mut props = json!({});
        let axis = |lo: Option<f64>, hi: Option<f64>, parent: f64, own: f64, fixed: bool| {
            let size = (!fixed && lo.is_some() && hi.is_some()).then(|| (parent - lo.unwrap_or(0.0) - hi.unwrap_or(0.0)).max(0.0));
            let at = match (lo, hi) {
                (Some(lo), _) => lo,
                (None, Some(hi)) => parent - hi - size.unwrap_or(own),
                _ => 0.0,
            };
            (at, size)
        };
        let (x, w) = axis(p.at.left, p.at.right, pw, cw, p.w_fixed);
        let (y, h) = axis(p.at.top, p.at.bottom, ph, ch, p.h_fixed);
        props["x"] = json!(x);
        props["y"] = json!(y);
        if let Some(w) = w {
            props["width"] = json!(w);
        }
        if let Some(h) = h {
            props["height"] = json!(h);
        }
        // Anchored to the edge it is offset from; to both edges when it is offset from both.
        let pick = |lo: Option<f64>, hi: Option<f64>, [first, last, both]: [&'static str; 3]| match (lo, hi) {
            (Some(_), Some(_)) => both,
            (None, Some(_)) => last,
            _ => first,
        };
        props["constraints"] = json!({
            "horizontal": pick(p.at.left, p.at.right, ["left", "right", "leftRight"]),
            "vertical": pick(p.at.top, p.at.bottom, ["top", "bottom", "topBottom"]),
        });
        super::props::apply(CMD, doc, p.child, &props_of(props))?;
    }
    Ok(())
}

fn create(s: &mut Session, ty: &str, l: &Layer, parent: NodeId, at: (f64, f64), props: Map<String, Value>, text: Option<&str>) -> Result<NodeId> {
    let mut params = json!({"type": ty, "parent": parent, "x": at.0, "y": at.1, "select": false, "props": props});
    if let Some(n) = &l.name {
        params["name"] = json!(n);
    }
    if let Some(t) = text {
        params["text"] = json!(t);
    }
    if let Some(w) = fixed(l.w) {
        params["width"] = json!(w);
    }
    if let Some(h) = fixed(l.h) {
        params["height"] = json!(h);
    }
    let r = super::create::create(s, &params)?;
    node_id(&r)
}

fn node_id(r: &Value) -> Result<NodeId> {
    r.get("id").and_then(Value::as_u64).map(NodeId).ok_or_else(|| EngineError::Other("import.html: a layer was created without an id".into()))
}

/// The keys of `props` that make sense on a layer the importer sizes itself.
fn visual_props(props: &Map<String, Value>) -> Map<String, Value> {
    props
        .iter()
        .filter(|(k, _)| matches!(k.as_str(), "cornerRadius" | "opacity" | "visible" | "gridColumn" | "gridColumnSpan" | "gridRow" | "gridRowSpan"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

fn place_image(s: &mut Session, l: &Layer, b64: &str, parent: NodeId, at: (f64, f64), warn: &mut Warn) -> Result<NodeId> {
    let mut params = json!({"data": b64, "parent": parent, "x": at.0, "y": at.1});
    if let Some(n) = &l.name {
        params["name"] = json!(n);
    }
    let r = match super::create::place_image(s, &params) {
        Ok(r) => r,
        Err(e) => {
            warn.add(format!("an <img> data: URI could not be read ({e}): drawn as a placeholder rectangle"));
            let mut rect = Layer {
                kind: Kind::Rect,
                name: l.name.clone(),
                text: String::new(),
                w: l.w,
                h: l.h,
                avail: l.avail,
                props: l.props.clone(),
                children: Vec::new(),
                abs: None,
                stretch: false,
            };
            rect.props.insert("fill".into(), json!("#E5E7EB"));
            (rect.w, rect.h) = (Sz::Fixed(fixed(l.w).unwrap_or(300.0)), Sz::Fixed(fixed(l.h).unwrap_or(150.0)));
            return place(s, &rect, parent, at, warn);
        }
    };
    let id = node_id(&r)?;
    let (iw, ih) = (num(&r, "width").unwrap_or(1.0).max(1.0), num(&r, "height").unwrap_or(1.0).max(1.0));
    let avail = l.avail.filter(|a| *a >= 1.0);
    let tw = match l.w {
        Sz::Fixed(v) => v,
        Sz::Fill => avail.unwrap_or(iw),
        Sz::Hug => avail.map_or(iw, |a| iw.min(a)),
    };
    let th = fixed(l.h).unwrap_or(tw * ih / iw);
    let mut props = visual_props(&l.props);
    props.insert("height".into(), json!(th));
    if l.w == Sz::Fill {
        props.insert("sizingH".into(), json!("fill"));
    } else {
        props.insert("width".into(), json!(tw));
    }
    super::props::apply("import.html", s.doc_mut()?, id, &props)?;
    Ok(id)
}

fn place_svg(s: &mut Session, l: &Layer, src: &str, parent: NodeId, at: (f64, f64), warn: &mut Warn) -> Result<NodeId> {
    let name = l.name.clone().unwrap_or_else(|| "svg".into());
    match figcraft_svg::import_svg_with_notes(s.doc_mut()?, src.as_bytes(), parent, at.0, at.1, &name) {
        Ok(imp) => {
            for n in imp.notes.iter().take(5) {
                warn.add(format!("svg: {n}"));
            }
            super::props::apply("import.html", s.doc_mut()?, imp.frame, &visual_props(&l.props))?;
            Ok(imp.frame)
        }
        Err(e) => {
            warn.add(format!("an inline <svg> could not be imported ({e}): drawn as a placeholder rectangle"));
            let mut rect = Layer {
                kind: Kind::Rect,
                name: l.name.clone(),
                text: String::new(),
                w: Sz::Fixed(fixed(l.w).unwrap_or(24.0)),
                h: Sz::Fixed(fixed(l.h).unwrap_or(24.0)),
                avail: l.avail,
                props: visual_props(&l.props),
                children: Vec::new(),
                abs: None,
                stretch: false,
            };
            rect.props.insert("fill".into(), json!("#E5E7EB"));
            place(s, &rect, parent, at, warn)
        }
    }
}

#[cfg(test)]
mod tests;
