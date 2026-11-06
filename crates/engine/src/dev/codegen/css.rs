use std::collections::{HashMap, HashSet};

use figcraft_doc::VariableId;

use super::{
    ContentAlign, CrossAlign, Document, EffectKind, LayoutMode, LetterSpacing, LineHeight, MainAlign, Node, NodeId, NodeKind, PADDING, PaintKind,
    Sizing, TextAlignH, TextCase, TextDecoration, Variable, css_borders, num, num6,
};

/// Most grid tracks written out (the layout handles the same number).
const MAX_TRACKS: usize = 1000;

// ------------------------------------------------------------------ CSS

/// The CSS custom properties generated code uses: name → fallback value, in first-use order.
#[derive(Default)]
pub(crate) struct Tokens {
    vars: Vec<(String, String)>,
    /// Name variables as Tailwind theme variables (`--color-accent`), not by their CSS name.
    pub tailwind: bool,
    /// The file's theme names (computed on first use).
    theme: Option<crate::tailwind::Theme>,
    /// The file's CSS names (computed on first use).
    names: Option<CssNames>,
}

impl Tokens {
    /// `var(--name, value)`, remembering the variable.
    fn var(&mut self, doc: &Document, variable: &Variable, value: String) -> String {
        let name = if self.tailwind {
            self.theme.get_or_insert_with(|| crate::tailwind::Theme::new(doc)).name(variable)
        } else {
            self.names.get_or_insert_with(|| CssNames::new(doc)).name(variable)
        };
        let out = format!("var({name}, {value})");
        if !self.vars.iter().any(|(n, _)| *n == name) {
            self.vars.push((name, value));
        }
        out
    }

    /// A `:root` rule declaring the variables (empty without any).
    pub fn root_rule(&self) -> String {
        if self.vars.is_empty() {
            return String::new();
        }
        let decls: String = self.vars.iter().map(|(n, v)| format!("  {n}: {v};\n")).collect();
        format!(":root {{\n{decls}}}\n")
    }
}

/// A variable's CSS custom property name: "Colors/Brand 500" → `--colors-brand-500`.
pub fn css_var_name(variable: &str) -> String {
    let mut out = String::from("--");
    for c in variable.chars().flat_map(char::to_lowercase).take(80) {
        if c.is_alphanumeric() || c == '_' {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_end_matches('-');
    if trimmed.len() <= 2 { "--variable".into() } else { trimmed.to_string() }
}

/// A variable's CSS custom property: its Web code syntax (`var(--x)` or `--x`) when it has one,
/// else made from its name.
pub fn css_var_of(v: &Variable) -> String {
    let Some(web) = v.code_syntax.get("WEB").map(|w| w.trim()) else { return css_var_name(&v.name) };
    let inner = web.strip_prefix("var(").map_or(web, |r| r.split([',', ')']).next().unwrap_or("").trim());
    let valid = inner.len() > 2 && inner.starts_with("--") && inner.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_');
    if valid { inner.chars().take(80).collect() } else { css_var_name(inner) }
}

/// The file's variables' CSS custom properties, unique in the file: [`css_var_of`], and where an
/// earlier variable already has that name, the name with a numeric suffix (`--brand-primary-2`).
pub struct CssNames {
    names: HashMap<VariableId, String>,
}

impl CssNames {
    pub fn new(doc: &Document) -> Self {
        let vars = &doc.variables.variables;
        let mut names = HashMap::with_capacity(vars.len());
        // Every plain name is reserved first, so a suffixed name never takes another's plain one;
        // the next suffix per base keeps this linear.
        let plain: Vec<String> = vars.iter().map(css_var_of).collect();
        let mut taken: HashSet<String> = HashSet::with_capacity(vars.len());
        let all: HashSet<&str> = plain.iter().map(String::as_str).collect();
        let mut next: HashMap<&str, usize> = HashMap::new();
        for (v, base) in vars.iter().zip(&plain) {
            let mut name = base.clone();
            if !taken.insert(name.clone()) {
                let k = next.entry(base.as_str()).or_insert(1);
                loop {
                    *k += 1;
                    name = format!("{base}-{k}");
                    if !all.contains(name.as_str()) && taken.insert(name.clone()) {
                        break;
                    }
                }
            }
            names.insert(v.id.clone(), name);
        }
        CssNames { names }
    }

    /// The custom property for `v` (its [`css_var_of`] name for a variable not in the file).
    pub fn name(&self, v: &Variable) -> String {
        self.names.get(&v.id).cloned().unwrap_or_else(|| css_var_of(v))
    }
}

/// `value`, or a reference to the variable bound to the first of `fields` that has one.
pub(super) fn tok(doc: &Document, n: &Node, fields: &[&str], value: String, t: &mut Tokens) -> String {
    match fields.iter().find_map(|f| crate::devinspect::bound_variable(doc, n, f)) {
        Some(v) => t.var(doc, v, value),
        None => value,
    }
}

pub(super) fn css(doc: &Document, id: NodeId) -> String {
    let Some(n) = doc.node(id) else { return String::new() };
    let mut t = Tokens::default();
    // A child of an auto layout frame also says how the frame places it (flex or grid item, or
    // absolute); a frame with ignore-auto-layout children is their containing block.
    let parent = n.parent.and_then(|p| doc.node(p)).filter(|p| p.layout.is_some());
    let in_flow = parent.is_some() && !n.child_layout.absolute;
    let hug = if parent.is_some() { sized_by_parent(n, parent, false, in_flow) } else { (false, false) };
    let mut decls = if parent.is_some() { placement_decls(n, parent, false, in_flow) } else { String::new() };
    if n.layout.is_some() && !decls.contains("position:") && n.children.iter().any(|c| doc.node(*c).is_some_and(|k| k.child_layout.absolute)) {
        decls.push_str("position: relative;\n");
    }
    decls.push_str(&css_decls(doc, n, &mut t, hug));
    format!("/* {} */\n{decls}", n.name.replace("*/", "* /"))
}

/// Which of the width and height `n` leaves to its parent: a child that fills a flex or grid
/// parent, or an ignore-auto-layout child pinned to both sides (or scaled) on that axis, has none
/// of its own, so the flex, stretch or offsets size it.
pub(super) fn sized_by_parent(n: &Node, parent: Option<&Node>, root: bool, in_flow: bool) -> (bool, bool) {
    if root {
        return (false, false);
    }
    if in_flow {
        let layout = parent.is_some_and(|p| p.layout.is_some());
        return (layout && n.child_layout.sizing_h == Sizing::Fill, layout && n.child_layout.sizing_v == Sizing::Fill);
    }
    let (h, v) = pins(n, parent);
    let (pw, ph) = parent.map_or((0.0, 0.0), |p| (p.width, p.height));
    (matches!(h, Pin::Both | Pin::Scale) && pw > 0.0, matches!(v, Pin::Both | Pin::Scale) && ph > 0.0)
}

/// How a child is anchored to its parent on one axis (the constraint, as CSS offsets).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pin {
    Start,
    End,
    Both,
    Center,
    Scale,
}

/// The horizontal and vertical pins of `n` at its position; a rotated layer or one without a
/// parent stays at its top left corner.
fn pins(n: &Node, parent: Option<&Node>) -> (Pin, Pin) {
    if parent.is_none() || figcraft_geom::rotation_degrees(n.transform).abs() > 0.01 {
        return (Pin::Start, Pin::Start);
    }
    let h = match n.constraints.horizontal {
        figcraft_doc::HConstraint::Left => Pin::Start,
        figcraft_doc::HConstraint::Right => Pin::End,
        figcraft_doc::HConstraint::LeftRight => Pin::Both,
        figcraft_doc::HConstraint::Center => Pin::Center,
        figcraft_doc::HConstraint::Scale => Pin::Scale,
    };
    let v = match n.constraints.vertical {
        figcraft_doc::VConstraint::Top => Pin::Start,
        figcraft_doc::VConstraint::Bottom => Pin::End,
        figcraft_doc::VConstraint::TopBottom => Pin::Both,
        figcraft_doc::VConstraint::Center => Pin::Center,
        figcraft_doc::VConstraint::Scale => Pin::Scale,
    };
    (h, v)
}

/// The offsets of one axis for a layer at `pos` of `size` in a parent of `parent` pixels, as lines
/// of `near`/`far` offsets (and `margin` to centre).
fn axis_decls(out: &mut String, pin: Pin, [near, far, margin]: [&str; 3], pos: f64, size: f64, parent: f64) {
    let after = parent - pos - size;
    match pin {
        Pin::End => out.push_str(&format!("{far}: {}px;\n", num(after))),
        Pin::Both => out.push_str(&format!("{near}: {}px;\n{far}: {}px;\n", num(pos), num(after))),
        Pin::Center if parent > 0.0 => out.push_str(&format!("{near}: 50%;\n{margin}: {}px;\n", num(pos - parent / 2.0))),
        Pin::Scale if parent > 0.0 => out.push_str(&format!("{near}: {}%;\n{far}: {}%;\n", num(pos / parent * 100.0), num(after / parent * 100.0))),
        _ => out.push_str(&format!("{near}: {}px;\n", num(pos))),
    }
}

/// How `n` sits in its parent: the flex or grid item properties of an auto layout child, or the
/// position (`position: absolute` and offsets from its constraints, against a positioned parent)
/// of a layer placed at its coordinates. The root is the positioned parent.
pub(super) fn placement_decls(n: &Node, parent: Option<&Node>, root: bool, in_flow: bool) -> String {
    let mut out = String::new();
    if root {
        out.push_str("position: relative;\n");
    } else if in_flow {
        if let Some(l) = parent.and_then(|p| p.layout.as_ref()) {
            out.push_str(&item_decls(n, l));
        }
    } else {
        let c = n.transform.as_coeffs();
        out.push_str("position: absolute;\n");
        let (h, v) = pins(n, parent);
        let (pw, ph) = parent.map_or((0.0, 0.0), |p| (p.width, p.height));
        axis_decls(&mut out, h, ["left", "right", "margin-left"], c[4], n.width, pw);
        axis_decls(&mut out, v, ["top", "bottom", "margin-top"], c[5], n.height, ph);
        let rot = figcraft_geom::rotation_degrees(n.transform);
        if rot.abs() > 0.01 {
            out.push_str(&format!("transform: rotate({}deg);\ntransform-origin: 0 0;\n", num(-rot)));
        }
    }
    out
}

/// The flex or grid item declarations of `n` in auto layout frame `l`.
fn item_decls(n: &Node, l: &figcraft_doc::AutoLayout) -> String {
    let cl = &n.child_layout;
    let mut out = String::new();
    if l.mode == LayoutMode::Grid {
        // Placement: the anchor cell and spans; a child without either is auto-placed.
        let line = |key: &str, anchor: Option<u32>, span: u32| match (anchor, span) {
            (Some(a), 1) => format!("{key}: {};\n", u64::from(a) + 1),
            (Some(a), s) => format!("{key}: {} / span {s};\n", u64::from(a) + 1),
            (None, 1) => String::new(),
            (None, s) => format!("{key}: span {s};\n"),
        };
        out.push_str(&line("grid-column", cl.grid_column, cl.column_span()));
        out.push_str(&line("grid-row", cl.grid_row, cl.row_span()));
        // In its cell: filling it, or aligned where the child or the frame says (the frame's
        // alignment is on the container).
        let cell = |a: figcraft_doc::CellAlign| match a {
            figcraft_doc::CellAlign::Min => "start",
            figcraft_doc::CellAlign::Center => "center",
            figcraft_doc::CellAlign::Max => "end",
        };
        let h = if cl.sizing_h == Sizing::Fill { Some("stretch") } else { cl.grid_align_h.map(cell) };
        let v = if cl.sizing_v == Sizing::Fill { Some("stretch") } else { cl.grid_align_v.map(cell) };
        if let Some(h) = h {
            out.push_str(&format!("justify-self: {h};\n"));
        }
        if let Some(v) = v {
            out.push_str(&format!("align-self: {v};\n"));
        }
        return out;
    }
    let vertical = l.mode == LayoutMode::Vertical;
    let (main, cross) = if vertical { (cl.sizing_v, cl.sizing_h) } else { (cl.sizing_h, cl.sizing_v) };
    match main {
        // Equal shares of the free space; `min-*: 0` lets a filling child be narrower than its content.
        Sizing::Fill => {
            out.push_str("flex: 1 1 0;\n");
            let (min, key) = if vertical { (cl.min_h, "min-height") } else { (cl.min_w, "min-width") };
            if min.is_none() {
                out.push_str(&format!("{key}: 0;\n"));
            }
        }
        // Auto layout never shrinks a fixed size.
        Sizing::Fixed => out.push_str("flex-shrink: 0;\n"),
        Sizing::Hug => {}
    }
    if cross == Sizing::Fill {
        out.push_str("align-self: stretch;\n");
    }
    out
}

/// The `aspect-ratio` of a layer with locked proportions: the stored ratio, else its size.
fn aspect_ratio_css(n: &Node) -> Option<String> {
    if !n.constrain_proportions {
        return None;
    }
    match n.aspect_ratio.filter(|r| r.is_finite() && *r > 0.0) {
        Some(r) => Some(num6(r)),
        None if n.width.is_finite() && n.height.is_finite() && n.width > 0.0 && n.height > 0.0 => {
            Some(format!("{} / {}", num(n.width), num(n.height)))
        }
        None => None,
    }
}

/// The CSS declarations for `n`'s own look and size, one per line; `hug` leaves out the width
/// and/or height (they follow the content). Bound variables become `var(--name, value)`.
pub(crate) fn css_decls(doc: &Document, n: &Node, t: &mut Tokens, hug: (bool, bool)) -> String {
    let px = |v: f64| format!("{}px", num(v));
    let mut out = String::new();
    let mut put = |key: &str, value: String| out.push_str(&format!("{key}: {value};\n"));
    if !hug.0 {
        put("width", tok(doc, n, &["width"], px(n.width), t));
    }
    if !hug.1 {
        put("height", tok(doc, n, &["height"], px(n.height), t));
    }
    let cl = &n.child_layout;
    for (key, limit) in [("min-width", cl.min_w), ("max-width", cl.max_w), ("min-height", cl.min_h), ("max-height", cl.max_h)] {
        if let Some(v) = limit.filter(|v| v.is_finite() && *v >= 0.0) {
            put(key, px(v));
        }
    }
    if let Some(r) = aspect_ratio_css(n) {
        put("aspect-ratio", r);
    }
    // Padding and strokes count inside the size, as they do in auto layout.
    let in_layout = n.parent.and_then(|p| doc.node(p)).is_some_and(|p| p.layout.is_some());
    if n.layout.is_some() || in_layout {
        put("box-sizing", "border-box".into());
    }
    if let Some(l) = &n.layout {
        layout_decls(doc, n, l, t, &mut put);
    }
    if matches!(n.kind, NodeKind::Ellipse { .. }) {
        put("border-radius", "50%".into());
    } else if let Some(r) = n.corners.uniform().filter(|r| *r > 0.0) {
        put("border-radius", tok(doc, n, &["cornerRadius"], px(r), t));
    } else if n.corners.radii.iter().any(|r| *r > 0.0) {
        let fields = ["topLeftRadius", "topRightRadius", "bottomRightRadius", "bottomLeftRadius"];
        let corners: Vec<String> = n.corners.radii.iter().zip(fields).map(|(r, f)| tok(doc, n, &[f, "cornerRadius"], px(*r), t)).collect();
        put("border-radius", corners.join(" "));
    }
    if n.opacity < 1.0 {
        put("opacity", tok(doc, n, &["opacity"], num(f64::from(n.opacity)), t));
    }
    if !matches!(n.blend, figcraft_color::BlendMode::Normal | figcraft_color::BlendMode::PassThrough) {
        put("mix-blend-mode", n.blend.css().into());
    }
    // Rotation turns counter-clockwise about the top-left corner (the position given); CSS
    // rotate turns clockwise.
    let rot = figcraft_geom::rotation_degrees(n.transform);
    if rot.abs() > 0.01 && rot.is_finite() {
        put("transform", format!("rotate({}deg)", num(-rot)));
        put("transform-origin", "top left".into());
    }
    for (i, p) in n.fills.iter().enumerate().filter(|(_, p)| p.visible) {
        match &p.kind {
            PaintKind::Solid { color } => {
                let key = if matches!(n.kind, NodeKind::Text(_)) { "color" } else { "background" };
                let c = color.with_alpha(color.a * p.opacity).to_css();
                // A variable binds the first paint's colour.
                put(key, if i == 0 { tok(doc, n, &["fill"], c, t) } else { c });
            }
            PaintKind::Gradient(g) => put("background", css_gradient(g, n.width, n.height, p.opacity)),
            PaintKind::Image(_) => put("background", "url(image.png) center / cover".into()),
            PaintKind::Pattern(_) => put("background", "url(pattern.png) repeat".into()),
        }
    }
    for (key, value) in css_borders(doc, n, t) {
        put(&key, value);
    }
    effect_decls(n, &mut put);
    if let NodeKind::Text(tx) = &n.kind {
        text_decls(doc, n, tx, t, &mut put);
    }
    out
}

/// The flex or grid declarations of auto layout `l` on `n`.
fn layout_decls(doc: &Document, n: &Node, l: &figcraft_doc::AutoLayout, t: &mut Tokens, put: &mut impl FnMut(&str, String)) {
    let px = |v: f64| format!("{}px", num(v));
    let vertical = l.mode == LayoutMode::Vertical;
    let grid = l.mode == LayoutMode::Grid;
    put("display", if grid { "grid" } else { "flex" }.into());
    if grid {
        // Columns and rows: fixed px, fill `Nfr`, hug `auto`; the gaps are column-gap (gap) and row-gap (cross gap).
        put("grid-template-columns", track_list(&l.column_tracks, l.grid_columns));
        put("grid-template-rows", track_list(&l.row_tracks, l.grid_rows));
        put("column-gap", tok(doc, n, &["gap"], px(l.gap), t));
        put("row-gap", tok(doc, n, &["crossGap"], px(l.cross_gap), t));
    } else {
        put("flex-direction", if vertical { "column" } else { "row" }.into());
        if l.wrap {
            put("flex-wrap", "wrap".into());
        }
        // An auto gap (space between/around/evenly) is the free space, not the gap value; the lines
        // of a wrapped frame spread the same way (`contentAlign`).
        let main = (!l.main_align.is_auto_gap()).then(|| tok(doc, n, &["gap"], px(l.gap), t));
        if l.wrap {
            let cross = (l.content_align != ContentAlign::SpaceBetween).then(|| tok(doc, n, &["crossGap"], px(l.cross_gap), t));
            // Between items in a line is the column gap of a row, the row gap of a column.
            let (column, row) = if vertical { (cross, main) } else { (main, cross) };
            if let Some(v) = column {
                put("column-gap", v);
            }
            if let Some(v) = row {
                put("row-gap", v);
            }
        } else if let Some(v) = main {
            put("gap", v);
        }
    }
    let [pt, pr, pb, pl] = l.padding;
    let sides = [
        tok(doc, n, &PADDING[0], px(pt), t),
        tok(doc, n, &PADDING[1], px(pr), t),
        tok(doc, n, &PADDING[2], px(pb), t),
        tok(doc, n, &PADDING[3], px(pl), t),
    ];
    put("padding", sides.join(" "));
    if grid {
        // The frame's alignment in each cell (children can override it).
        let h = match l.main_align {
            MainAlign::Center => "center",
            MainAlign::Max => "end",
            _ => "start",
        };
        let v = match l.cross_align {
            CrossAlign::Center => "center",
            CrossAlign::Max => "end",
            _ => "start",
        };
        put("justify-items", h.into());
        put("align-items", v.into());
        return;
    }
    let justify = match l.main_align {
        MainAlign::Min => "flex-start",
        MainAlign::Center => "center",
        MainAlign::Max => "flex-end",
        MainAlign::SpaceBetween => "space-between",
        MainAlign::SpaceAround => "space-around",
        MainAlign::SpaceEvenly => "space-evenly",
    };
    put("justify-content", justify.into());
    let align = match l.cross_align {
        CrossAlign::Min => "flex-start",
        CrossAlign::Center => "center",
        CrossAlign::Max => "flex-end",
        CrossAlign::Baseline => "baseline",
    };
    put("align-items", align.into());
    if l.wrap {
        // The lines: spread, or packed where the items align (baseline: at the start).
        let spread = l.content_align == ContentAlign::SpaceBetween;
        put(
            "align-content",
            if spread {
                "space-between"
            } else if align == "baseline" {
                "flex-start"
            } else {
                align
            }
            .into(),
        );
    }
}

/// A grid's track sizes for `count` tracks: fixed `Npx`, fill `Nfr`, hug `auto`; tracks past the
/// list are `1fr`.
fn track_list(tracks: &[figcraft_doc::GridTrack], count: u32) -> String {
    let count = (count as usize).clamp(1, MAX_TRACKS);
    (0..count)
        .map(|i| {
            let track = tracks.get(i).copied().unwrap_or_default();
            match track.sizing {
                Sizing::Fixed => format!("{}px", num(track.value.max(0.0))),
                Sizing::Hug => "auto".into(),
                Sizing::Fill => format!("{}fr", num(track.value.max(0.0))),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Shadows and blurs of `n`.
fn effect_decls(n: &Node, put: &mut impl FnMut(&str, String)) {
    let shadows: Vec<String> = n
        .effects
        .iter()
        .filter(|e| e.visible)
        .filter_map(|e| match &e.kind {
            EffectKind::DropShadow(s) => {
                Some(format!("{}px {}px {}px {}px {}", num(s.offset.0), num(s.offset.1), num(s.radius), num(s.spread), s.color.to_css()))
            }
            EffectKind::InnerShadow(s) => {
                Some(format!("inset {}px {}px {}px {}px {}", num(s.offset.0), num(s.offset.1), num(s.radius), num(s.spread), s.color.to_css()))
            }
            _ => None,
        })
        .collect();
    if !shadows.is_empty() {
        put("box-shadow", shadows.join(", "));
    }
    for e in n.effects.iter().filter(|e| e.visible) {
        match e.kind {
            // CSS blurs are uniform: a progressive blur has no CSS equivalent and is left out.
            EffectKind::LayerBlur(ref b) if !b.is_progressive() => put("filter", format!("blur({}px)", num(b.radius / 2.0))),
            EffectKind::BackgroundBlur(ref b) if !b.is_progressive() => put("backdrop-filter", format!("blur({}px)", num(b.radius / 2.0))),
            // CSS has no refraction: the frost is the nearest.
            EffectKind::Glass(ref g) => put("backdrop-filter", format!("blur({}px)", num(g.sanitized().frost / 2.0))),
            _ => {}
        }
    }
}

/// The type declarations of text layer `n`.
fn text_decls(doc: &Document, n: &Node, tx: &figcraft_doc::TextData, t: &mut Tokens, put: &mut impl FnMut(&str, String)) {
    let px = |v: f64| format!("{}px", num(v));
    let s = &tx.style;
    put(
        "font-family",
        tok(
            doc,
            n,
            &["fontFamily"],
            format!(
                "\"{}\"",
                s.font_family.replace('\\', "\\\\").replace('"', "\\\"").replace('<', "\\3C ").replace("\r\n", "\\A ").replace(['\n', '\r'], "\\A ")
            ),
            t,
        ),
    );
    put("font-size", tok(doc, n, &["fontSize"], px(s.font_size), t));
    put("font-weight", s.weight().to_string());
    if s.italic() {
        put("font-style", "italic".into());
    }
    match s.line_height {
        LineHeight::Pixels(v) => put("line-height", tok(doc, n, &["lineHeight"], px(v), t)),
        LineHeight::Percent(p) => put("line-height", format!("{}%", num(p))),
        LineHeight::Auto => {}
    }
    match s.letter_spacing {
        LetterSpacing::Pixels(v) if v != 0.0 => put("letter-spacing", tok(doc, n, &["letterSpacing"], px(v), t)),
        LetterSpacing::Percent(p) if p != 0.0 => put("letter-spacing", format!("{}em", num(p / 100.0))),
        _ => {}
    }
    match tx.align_h {
        TextAlignH::Left => {}
        TextAlignH::Center => put("text-align", "center".into()),
        TextAlignH::Right => put("text-align", "right".into()),
        TextAlignH::Justified => put("text-align", "justify".into()),
    }
    match tx.wrap_style {
        figcraft_doc::WrapStyle::Auto => {}
        figcraft_doc::WrapStyle::Balance => put("text-wrap", "balance".into()),
        figcraft_doc::WrapStyle::Pretty => put("text-wrap", "pretty".into()),
    }
    match s.case {
        TextCase::Original => {}
        TextCase::Upper => put("text-transform", "uppercase".into()),
        TextCase::Lower => put("text-transform", "lowercase".into()),
        TextCase::Title => put("text-transform", "capitalize".into()),
        TextCase::SmallCaps => put("font-variant", "small-caps".into()),
    }
    match s.decoration {
        TextDecoration::None => {}
        TextDecoration::Underline => put("text-decoration", "underline".into()),
        TextDecoration::Strikethrough => put("text-decoration", "line-through".into()),
    }
    decoration_decls(s, put);
    if tx.leading_trim == figcraft_doc::LeadingTrim::CapHeight {
        put("text-box", "trim-both cap alphabetic".into());
    }
}

fn decoration_decls(s: &figcraft_doc::TextStyle, put: &mut impl FnMut(&str, String)) {
    let px = |v: f64| format!("{}px", num(v));
    if s.decoration != TextDecoration::None {
        let l = s.look();
        let len = |m: LineHeight| match m {
            LineHeight::Auto => None,
            LineHeight::Pixels(v) => Some(px(v)),
            LineHeight::Percent(p) => Some(format!("{}%", num(p))),
        };
        match l.style {
            figcraft_doc::DecorationStyle::Solid => {}
            figcraft_doc::DecorationStyle::Dotted => put("text-decoration-style", "dotted".into()),
            figcraft_doc::DecorationStyle::Wavy => put("text-decoration-style", "wavy".into()),
        }
        if let Some(v) = len(l.thickness) {
            put("text-decoration-thickness", v);
        }
        if let Some(c) = l.color {
            put("text-decoration-color", c.to_css());
        }
        if s.decoration == TextDecoration::Underline {
            if let Some(v) = len(l.offset) {
                put("text-underline-offset", v);
            }
            // Browsers skip ink by default; the canvas only when asked.
            if !l.skip_ink {
                put("text-decoration-skip-ink", "none".into());
            }
        }
    }
}

/// A gradient fill on a `w`×`h` box as a CSS background: linear along its handles' direction
/// (CSS angles: 0deg up, 90deg right), radial with its handles' radii, angular as a conic
/// gradient from its start direction.
fn css_gradient(g: &figcraft_color::Gradient, w: f64, h: f64, opacity: f32) -> String {
    let stops = g
        .sorted_stops()
        .iter()
        .map(|s| format!("{} {}%", s.color.with_alpha(s.color.a * opacity).to_css(), (s.position * 100.0).round()))
        .collect::<Vec<_>>()
        .join(", ");
    let (dx, dy) = ((g.to.0 - g.from.0) * w, (g.to.1 - g.from.1) * h);
    let angle = if dx == 0.0 && dy == 0.0 { 180.0 } else { dx.atan2(-dy).to_degrees().rem_euclid(360.0) };
    let pct = |v: f64| format!("{}%", num(v * 100.0));
    let at = format!("at {} {}", pct(g.from.0), pct(g.from.1));
    match g.kind {
        figcraft_color::GradientKind::Linear => format!("linear-gradient({}deg, {stops})", num(angle)),
        figcraft_color::GradientKind::Radial | figcraft_color::GradientKind::Diamond => {
            let (rx, ry) = (((g.width.0 - g.from.0) * w).hypot((g.width.1 - g.from.1) * h), dx.hypot(dy));
            let rel = |r: f64, side: f64| if side > 0.0 { pct(r / side) } else { "50%".into() };
            format!("radial-gradient({} {} {at}, {stops})", rel(rx, w), rel(ry, h))
        }
        figcraft_color::GradientKind::Angular => format!("conic-gradient(from {}deg {at}, {stops})", num(angle)),
    }
}
