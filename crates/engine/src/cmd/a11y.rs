//! Accessibility checks on a design: contrast of text against what is painted behind it (WCAG 2.x
//! ratio and APCA Lc, per variable mode) and a colour-vision-deficiency preview.
//!
//! The background of a text layer is found from the layers that paint before it (document order) and
//! whose box holds the middle of the text box: their solid fills are blended from the top down until
//! one is opaque, over the page colour. Gradient, image, pattern and non-normal-blend fills make the
//! background `unknown` (reported, not guessed). Boxes are bounding boxes (rotation, ellipse and
//! vector outlines are not followed), and layer masks and strokes are ignored.

use std::collections::{BTreeMap, BTreeSet};

use base64::Engine as _;
use figcraft_color::a11y::{self as ca, Cvd};
use figcraft_color::{BlendMode, Color, PaintKind};
use figcraft_doc::{Document, ExportFormat, ExportSize, Node, NodeId, NodeKind, TextStyle, VariableValue};
use serde_json::{Value, json};

use super::CommandSpec;
use crate::params::{boolean, color, get, num, num_or, req_targets, string};
use crate::{EngineError, Result, Session, bad, find_command, spec};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        spec!(
            "a11y.contrast",
            "Check contrast",
            "{fg, bg (colours), fontSize? (px, default 16), bold? | fontWeight?} for one pair, or {ids (text layers; default the selection), level? (AA default | AAA)} for layers against what is behind them  → {ratio, apcaLc, large, aa, aaa, required, apca} for a pair; {results} for layers (as a11y.checkContrast lists them, passes too)",
            false,
            contrast
        ),
        spec!(
            "a11y.checkContrast",
            "Check text contrast",
            "{ids? (layers whose text is checked, default the current page), level? (AA default | AAA), all? (also list the passing ones)}  → {level, checked, failures [{id, name, mode, fg, bg, ratio, required, apcaLc, fontSize, large, boundVariable?, fixable, suggestion {textColor, ratio}}], unknown [{id, name, reason}], passed, results? (with all)}. One entry per variable mode (`mode` is \"Collection/Mode\") when the text or what is behind it is bound to a colour variable, else `mode` is null",
            false,
            check_contrast
        ),
        spec!(
            "a11y.fixContrast",
            "Fix text contrast",
            "{ids? | root? (default the current page), level? (AA default | AAA), prefer? (text (default) | background: change the plain solid layer behind the text instead; falls back to the text colour when the background is not one opaque solid), dryRun?}  for each text layer failing a11y.checkContrast, sets the colour of the text (or background) to the nearest one, by lightness with hue and saturation kept, that reaches the required ratio (in every variable mode), as one undo step. Colours bound to a variable are not changed: they are skipped with the variable to change → {level, dryRun, changed: [{id, name, changed (text|background), layer, from, to, ratio, required}], skipped: [{id, name, reason, variable?}], unknown, remaining? (failures left after applying)}",
            true,
            fix_contrast
        ),
        spec!(
            "a11y.simulate",
            "Colour vision preview",
            "{cvd (protanopia | deuteranopia | tritanopia | achromatopsia), ids? (default the current page), scale? (default 1), path? (write the PNG there)}  → {format: png, cvd, data (base64)} or {path}. The same filter is `cvd` on file.export (PNG)",
            false,
            simulate
        ),
    ]
}

const MAX_TEXTS: usize = 2000;

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

fn is_bold(st: &TextStyle) -> bool {
    if st.variations.get("wght").is_some_and(|w| *w >= 700.0) {
        return true;
    }
    let s = st.font_style.to_ascii_lowercase();
    !s.contains("semi") && !s.contains("demi") && ["bold", "black", "heavy"].iter().any(|k| s.contains(k))
}

/// The verdict for one colour pair and text size.
fn verdict(fg: Color, bg: Color, size: f64, bold: bool) -> Value {
    let ratio = ca::wcag_ratio(fg, bg);
    let large = ca::is_large_text(size, bold);
    let (aa_req, aaa_req) = (ca::wcag_required(large, false), ca::wcag_required(large, true));
    let lc = ca::apca_lc(fg, bg);
    let need = ca::apca_required(large);
    json!({
        "fg": fg.to_css_hex(), "bg": bg.to_css_hex(), "ratio": round2(ratio), "apcaLc": round1(lc), "fontSize": size, "bold": bold, "large": large,
        "aa": ratio >= aa_req, "aaa": ratio >= aaa_req, "required": {"aa": aa_req, "aaa": aaa_req},
        "apca": {"lc": round1(lc), "required": need, "pass": lc.abs() >= need},
    })
}

fn level_aaa(cmd: &str, p: &Value) -> Result<bool> {
    match string(p, "level").map(|l| l.to_ascii_uppercase()).as_deref() {
        None | Some("AA") => Ok(false),
        Some("AAA") => Ok(true),
        Some(_) => Err(bad(cmd, "level: AA or AAA")),
    }
}

fn contrast(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "a11y.contrast";
    if get(p, "fg").is_some_and(|v| !v.is_null()) || get(p, "bg").is_some_and(|v| !v.is_null()) {
        let fg = color(p, "fg").ok_or_else(|| bad(cmd, "fg: a colour such as \"#767676\""))?;
        let bg = color(p, "bg").ok_or_else(|| bad(cmd, "bg: a colour such as \"#FFFFFF\""))?;
        let size = num_or(p, "fontSize", 16.0);
        if !size.is_finite() || size <= 0.0 {
            return Err(bad(cmd, "fontSize: px, above 0"));
        }
        let bold = boolean(p, "bold").unwrap_or_else(|| num(p, "fontWeight").is_some_and(|w| w >= 700.0));
        return Ok(verdict(fg, bg, size, bold));
    }
    let ids = req_targets(cmd, s, p)?;
    let report = analyze(s.doc()?, &ids, level_aaa(cmd, p)?)?;
    Ok(json!({"level": report.level, "results": report.results, "unknown": report.unknown}))
}

pub(super) fn check_contrast(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "a11y.checkContrast";
    let aaa = level_aaa(cmd, p)?;
    let ids = match get(p, "ids") {
        Some(Value::Array(_)) => req_targets(cmd, s, p)?,
        _ => Vec::new(),
    };
    let r = analyze(s.doc()?, &ids, aaa)?;
    let failures: Vec<&Value> = r.results.iter().filter(|e| e["pass"] == json!(false)).collect();
    let mut out = json!({
        "level": r.level, "checked": r.checked, "failures": failures, "unknown": r.unknown,
        "passed": r.results.len() - failures.len(),
    });
    if boolean(p, "all").unwrap_or(false) {
        out["results"] = json!(r.results);
    }
    Ok(out)
}

/// (hue in degrees, saturation, lightness) of an sRGB colour.
fn to_hsl(c: Color) -> (f32, f32, f32) {
    let (mx, mn) = (c.r.max(c.g).max(c.b), c.r.min(c.g).min(c.b));
    let (l, d) = ((mx + mn) / 2.0, mx - mn);
    if d <= f32::EPSILON {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs()).max(f32::EPSILON);
    let h = if mx == c.r {
        ((c.g - c.b) / d).rem_euclid(6.0)
    } else if mx == c.g {
        (c.b - c.r) / d + 2.0
    } else {
        (c.r - c.g) / d + 4.0
    };
    (h * 60.0, s.clamp(0.0, 1.0), l)
}

fn from_hsl(h: f32, s: f32, l: f32) -> Color {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (h.rem_euclid(360.0) / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    Color { r: (r + m).clamp(0.0, 1.0), g: (g + m).clamp(0.0, 1.0), b: (b + m).clamp(0.0, 1.0), a: 1.0 }
}

/// The colour closest to `from` in lightness (hue and saturation kept) that reaches `need` on every
/// background; None when not even black or white does.
fn nearest_colour(from: Color, bgs: &[Color], need: f64) -> Option<Color> {
    let (h, s, l) = to_hsl(from);
    // Judge the 8-bit colour that would be written.
    let at = |l: f32| {
        let [r, g, b, _] = from_hsl(h, s, l).to_rgba8();
        Color::from_rgba8(r, g, b, 255)
    };
    let ok = |c: Color| bgs.iter().all(|bg| ca::wcag_ratio(c, *bg) >= need);
    let mut best: Option<(f32, Color)> = None;
    for end in [0.0f32, 1.0] {
        if !ok(at(end)) {
            continue;
        }
        let (mut lo, mut hi) = (l, end);
        for _ in 0..24 {
            let mid = (lo + hi) / 2.0;
            if ok(at(mid)) { hi = mid } else { lo = mid }
        }
        let d = (hi - l).abs();
        if best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, at(hi)));
        }
    }
    best.map(|b| b.1)
}

fn flag(cmd: &str, p: &Value, key: &str) -> Result<bool> {
    match get(p, key) {
        None | Some(Value::Null) => Ok(false),
        Some(v) => crate::params::bool_of(v).ok_or_else(|| bad(cmd, format!("`{key}` must be true or false"))),
    }
}

/// The name of the colour variable bound to fill `index` of `n`.
fn fill_variable(doc: &Document, n: &Node, index: usize) -> Option<String> {
    let key = super::lists::paint_binding_key(n, true, index)?;
    let var = n.bound.get(key)?;
    Some(doc.variables.get(var).map_or_else(|| var.clone(), |v| v.name.clone()))
}

#[allow(clippy::cognitive_complexity)] // shortcut: one pass over the failing texts; split it when a third strategy lands.
fn fix_contrast(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "a11y.fixContrast";
    const KEYS: &[&str] = &["ids", "id", "root", "level", "prefer", "dryRun"];
    if let Some(k) = p.as_object().and_then(|o| o.keys().find(|k| !KEYS.contains(&k.as_str()))) {
        return Err(bad(cmd, format!("unknown key `{k}` (known: {})", KEYS.join(", "))));
    }
    let aaa = level_aaa(cmd, p)?;
    let dry = flag(cmd, p, "dryRun")?;
    let prefer_bg = match string(p, "prefer").as_deref() {
        None | Some("text") => false,
        Some("background") => true,
        Some(_) => return Err(bad(cmd, "prefer: text or background")),
    };
    let roots = match crate::params::opt_id(cmd, p, "root")? {
        Some(_) if matches!(get(p, "ids"), Some(Value::Array(_))) => return Err(bad(cmd, "pass `ids` or `root`, not both")),
        Some(r) => vec![r],
        None => match get(p, "ids") {
            Some(Value::Array(_)) => req_targets(cmd, s, p)?,
            None | Some(Value::Null) => Vec::new(),
            Some(_) => return Err(bad(cmd, "`ids` must be an array of layer ids")),
        },
    };
    let report = analyze(s.doc()?, &roots, aaa)?;
    let mut order: Vec<NodeId> = Vec::new();
    let mut by_text: BTreeMap<NodeId, Vec<&Value>> = BTreeMap::new();
    for e in &report.results {
        let Some(id) = crate::params::id_of(&e["id"]) else { continue };
        if !by_text.contains_key(&id) {
            order.push(id);
        }
        by_text.entry(id).or_default().push(e);
    }
    let hex = |e: &Value, k: &str| e[k].as_str().and_then(Color::from_hex);
    let (mut changed, mut skipped) = (Vec::new(), Vec::new());
    let mut plan: Vec<(NodeId, Color)> = Vec::new();
    let mut moved_backgrounds: BTreeSet<NodeId> = BTreeSet::new();
    {
        let doc = s.doc()?;
        for tid in order {
            let entries = &by_text[&tid];
            if entries.iter().all(|e| e["pass"] == json!(true)) {
                continue;
            }
            let Some(node) = doc.node(tid) else { continue };
            let name = &node.name;
            let skip = |reason: String, variable: Option<String>| {
                let mut v = json!({"id": tid, "name": name, "reason": reason});
                if let Some(var) = variable {
                    v["variable"] = json!(var);
                }
                v
            };
            let need = entries.first().and_then(|e| e["requiredRatio"].as_f64()).unwrap_or(4.5);
            let bgs: Vec<Color> = entries.iter().filter_map(|e| hex(e, "bg")).collect();
            let Some(fg) = entries.iter().find(|e| e["pass"] == json!(false)).and_then(|e| hex(e, "fg")) else { continue };
            let fg_solid = ca::over(fg, bgs.first().copied().unwrap_or(Color::WHITE)).with_alpha(1.0);
            let top = node.fills.iter().rposition(|f| f.visible);
            // The background to change instead: one plain opaque solid layer, not bound to a variable.
            let mut target: Option<(NodeId, Color, Color, &'static str)> = None;
            if prefer_bg && fg.a >= 0.999 {
                let used: Vec<NodeId> = entries
                    .iter()
                    .flat_map(|e| e["background"]["ids"].as_array().cloned().unwrap_or_default())
                    .filter_map(|v| crate::params::id_of(&v))
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect();
                if let [bid] = used.as_slice()
                    && let Some(bn) = doc.node(*bid)
                {
                    if let Some(var) = (0..bn.fills.len()).find_map(|i| fill_variable(doc, bn, i)) {
                        skipped
                            .push(skip(format!("the background layer {bid} is bound to variable `{var}`; change that variable's value"), Some(var)));
                        continue;
                    }
                    if let [paint] = bn.fills.as_slice()
                        && let PaintKind::Solid { color } = &paint.kind
                        && paint.visible
                        && bn.opacity >= 0.999
                        && paint.opacity >= 0.999
                        && color.a >= 0.999
                    {
                        if moved_backgrounds.contains(bid) {
                            skipped.push(skip(format!("its background {bid} was already changed for another text; run again to re-check"), None));
                            continue;
                        }
                        target = Some((*bid, *color, fg_solid, "background"));
                    }
                }
            }
            if let Some((bid, from, against, _)) = target {
                match nearest_colour(from, &[against], need) {
                    Some(to) => {
                        moved_backgrounds.insert(bid);
                        let ratio = round2(ca::wcag_ratio(to, against));
                        changed.push(json!({"id": tid, "name": name, "changed": "background", "layer": bid, "from": from.to_css_hex(), "to": to.to_css_hex(), "ratio": ratio, "required": need}));
                        plan.push((bid, to));
                    }
                    None => skipped.push(skip(format!("no background colour reaches {need}:1 against this text colour"), None)),
                }
                continue;
            }
            if let Some(var) = top.and_then(|i| fill_variable(doc, node, i)) {
                skipped.push(skip(format!("the text colour is bound to variable `{var}`; change that variable's value, or unbind it"), Some(var)));
                continue;
            }
            if node.fills.len() != 1 || node.opacity < 0.999 {
                skipped.push(skip("the text has several fills or a layer opacity below 100%; set its colour by hand".into(), None));
                continue;
            }
            match nearest_colour(fg_solid, &bgs, need) {
                Some(to) => {
                    let ratio = round2(bgs.iter().map(|bg| ca::wcag_ratio(to, *bg)).fold(f64::MAX, f64::min));
                    changed.push(json!({"id": tid, "name": name, "changed": "text", "layer": tid, "from": fg_solid.to_css_hex(), "to": to.to_css_hex(), "ratio": ratio, "required": need}));
                    plan.push((tid, to));
                }
                None => skipped.push(skip(format!("no text colour reaches {need}:1 on this background"), None)),
            }
        }
    }
    let mut out = json!({"level": report.level, "dryRun": dry, "changed": changed, "skipped": skipped, "unknown": report.unknown});
    if !dry && !plan.is_empty() {
        let set = find_command("node.set").ok_or_else(|| EngineError::UnknownCommand("node.set".into()))?;
        for (id, to) in plan {
            (set.run)(s, &json!({"ids": [id], "props": {"fill": to.to_css_hex()}}))?;
        }
        let after = analyze(s.doc()?, &roots, aaa)?;
        out["remaining"] = json!(after.results.iter().filter(|e| e["pass"] == json!(false)).count());
    }
    Ok(out)
}

struct Report {
    level: &'static str,
    checked: usize,
    results: Vec<Value>,
    unknown: Vec<Value>,
}

/// What a layer's fill is in a set of variable modes.
enum Fill {
    Solid(Color),
    Complex,
}

type Modes = BTreeMap<String, String>;

fn fill_of(doc: &Document, id: NodeId, node: &Node, index: usize, modes: &Modes) -> Option<Fill> {
    let paint = node.fills.get(index).filter(|f| f.visible)?;
    if paint.blend != BlendMode::Normal {
        return Some(Fill::Complex);
    }
    let PaintKind::Solid { color } = &paint.kind else { return Some(Fill::Complex) };
    let mut c = color.with_alpha(color.a * paint.opacity);
    if let Some(var) = super::lists::paint_binding_key(node, true, index).and_then(|k| node.bound.get(k)) {
        let mut m = modes.clone();
        m.extend(doc.modes_for(id));
        if let Some(VariableValue::Color(v)) = doc.variables.resolve(var, &m) {
            let own = if paint.color_alpha > 0.0 { (paint.opacity / paint.color_alpha).min(1.0) } else { paint.opacity };
            c = v.with_alpha(v.a * own);
        }
    }
    Some(Fill::Solid(c.with_alpha(c.a * node.opacity)))
}

/// The collections a node's colour bindings reach (through aliases too).
fn bound_collections(doc: &Document, node: &Node, out: &mut BTreeSet<String>) {
    for (field, var) in &node.bound {
        if !matches!(super::lists::list_binding(field), Some((super::lists::BoundList::Fills, _, "color"))) {
            continue;
        }
        let mut cur = var.clone();
        for _ in 0..32 {
            let Some(v) = doc.variables.get(&cur) else { break };
            out.insert(v.collection.clone());
            match v.values.values().find_map(|x| if let VariableValue::Alias(a) = x { Some(a.clone()) } else { None }) {
                Some(next) => cur = next,
                None => break,
            }
        }
    }
}

struct Painter {
    id: NodeId,
    rect: figcraft_geom::Rect,
}

#[allow(clippy::cognitive_complexity)] // shortcut: one pass over every case; split it when the next case lands.
fn analyze(doc: &Document, roots: &[NodeId], aaa: bool) -> Result<Report> {
    let page = match roots.first() {
        Some(r) => doc.page_of(*r).or_else(|| doc.node(*r).filter(|n| n.is_page()).map(|n| n.id)),
        None => doc.pages.first().copied(),
    };
    let page = page
        .ok_or_else(|| EngineError::Other(roots.first().map_or("the file has no page".into(), |r| format!("no layer or page {r} in this file"))))?;
    let order = doc.subtree(page);
    let position: BTreeMap<NodeId, usize> = order.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let shown = |id: NodeId| doc.ancestors(id).iter().all(|a| doc.node(*a).is_none_or(|n| n.visible)) && doc.node(id).is_some_and(|n| n.visible);
    let scan: Vec<NodeId> = if roots.is_empty() { vec![page] } else { roots.to_vec() };
    let mut texts: Vec<NodeId> = Vec::new();
    for r in scan {
        for id in doc.subtree(r) {
            if doc.node(id).is_some_and(|n| matches!(n.kind, NodeKind::Text(_))) && shown(id) {
                texts.push(id);
            }
        }
    }
    texts.sort_by_key(|i| position.get(i).copied().unwrap_or(usize::MAX));
    texts.dedup();
    texts.truncate(MAX_TEXTS);
    // Layers that paint a box, in paint order.
    let painters: Vec<Painter> = order
        .iter()
        .filter_map(|id| {
            let n = doc.node(*id)?;
            if n.is_page() || matches!(n.kind, NodeKind::Text(_)) || !n.fills.iter().any(|f| f.visible) || !shown(*id) {
                return None;
            }
            Some(Painter { id: *id, rect: doc.world_bounds(*id)? })
        })
        .collect();
    let page_bg = match doc.node(page).map(|n| &n.kind) {
        Some(NodeKind::Page { background, .. }) => *background,
        _ => Color::WHITE,
    };
    let level = if aaa { "AAA" } else { "AA" };
    let mut report = Report { level, checked: texts.len(), results: Vec::new(), unknown: Vec::new() };
    for tid in texts {
        let (Some(node), Some(rect), Some(&pos)) = (doc.node(tid), doc.world_bounds(tid), position.get(&tid)) else { continue };
        let NodeKind::Text(data) = &node.kind else { continue };
        let (cx, cy) = ((rect.x0 + rect.x1) / 2.0, (rect.y0 + rect.y1) / 2.0);
        // Top-most first.
        let behind: Vec<&Painter> = painters
            .iter()
            .filter(|b| position.get(&b.id).is_some_and(|i| *i < pos) && b.rect.x0 <= cx && cx <= b.rect.x1 && b.rect.y0 <= cy && cy <= b.rect.y1)
            .rev()
            .collect();
        // Which collections matter, hence which modes to try.
        let mut involved = BTreeSet::new();
        bound_collections(doc, node, &mut involved);
        for b in &behind {
            if let Some(n) = doc.node(b.id) {
                bound_collections(doc, n, &mut involved);
            }
        }
        let mut scenarios: Vec<(Option<String>, Modes)> = Vec::new();
        for cid in &involved {
            if let Some(c) = doc.variables.collection(cid) {
                for m in &c.modes {
                    scenarios.push((Some(format!("{}/{}", c.name, m.name)), BTreeMap::from([(c.id.clone(), m.id.clone())])));
                }
            }
        }
        if scenarios.is_empty() {
            scenarios.push((None, Modes::new()));
        }
        // Text fills are drawn bottom to top: the top visible one is the colour readers see.
        let Some(fill_index) = node.fills.iter().rposition(|f| f.visible) else { continue };
        let bound_var = super::lists::paint_binding_key(node, true, fill_index)
            .and_then(|k| node.bound.get(k))
            .and_then(|v| doc.variables.get(v))
            .map(|v| v.name.clone());
        let size = data.style.font_size;
        let bold = is_bold(&data.style);
        let large = ca::is_large_text(size, bold);
        let need = ca::wcag_required(large, aaa);
        let excerpt: String = data.characters.chars().take(40).collect();
        for (label, modes) in scenarios {
            let Some(Fill::Solid(fg)) = fill_of(doc, tid, node, fill_index, &modes) else {
                report.unknown.push(json!({"id": tid, "name": node.name, "mode": label, "reason": "the text fill is a gradient, image or blended"}));
                continue;
            };
            // Blend what is behind from the top down until something is opaque.
            let mut layers: Vec<Color> = Vec::new();
            let mut used: Vec<NodeId> = Vec::new();
            let mut unknown: Option<NodeId> = None;
            'stack: for b in &behind {
                let Some(bn) = doc.node(b.id) else { continue };
                for i in (0..bn.fills.len()).rev() {
                    match fill_of(doc, b.id, bn, i, &modes) {
                        None => {}
                        Some(Fill::Complex) => {
                            unknown = Some(b.id);
                            break 'stack;
                        }
                        Some(Fill::Solid(c)) => {
                            layers.push(c);
                            if !used.contains(&b.id) {
                                used.push(b.id);
                            }
                            if c.a >= 0.999 {
                                break 'stack;
                            }
                        }
                    }
                }
            }
            if let Some(u) = unknown {
                report.unknown.push(json!({
                    "id": tid, "name": node.name, "mode": label,
                    "reason": format!("the layer behind it ({u}) has a gradient, image, pattern or blended fill: check it by eye or pass fg/bg")
                }));
                continue;
            }
            let bg = layers.iter().rev().fold(page_bg, |under, c| ca::over(*c, under));
            let mut e = verdict(fg, bg, size, bold);
            let pass = ca::wcag_ratio(fg, bg) >= need;
            e["id"] = json!(tid);
            e["name"] = json!(node.name);
            e["text"] = json!(excerpt);
            e["mode"] = json!(label);
            e["level"] = json!(level);
            e["pass"] = json!(pass);
            e["requiredRatio"] = json!(need);
            e["background"] = json!({"ids": used, "source": if layers.is_empty() { "page" } else { "layers" }});
            if let Some(v) = &bound_var {
                e["boundVariable"] = json!(v);
            }
            if !pass {
                match ca::fix_text_colour(fg, bg, need) {
                    Some(c) => {
                        e["fixable"] = json!(true);
                        e["suggestion"] = json!({"textColor": c.to_css_hex(), "ratio": round2(ca::wcag_ratio(c, bg))});
                    }
                    None => e["fixable"] = json!(false),
                }
            }
            report.results.push(e);
        }
    }
    Ok(report)
}

/// `cvd` of a command's parameters: None when unset.
pub(crate) fn cvd_param(cmd: &str, p: &Value) -> Result<Option<Cvd>> {
    match get(p, "cvd") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Cvd::parse(s).map(Some).ok_or_else(|| bad(cmd, "cvd: protanopia, deuteranopia, tritanopia or achromatopsia")),
        Some(_) => Err(bad(cmd, "cvd: protanopia, deuteranopia, tritanopia or achromatopsia")),
    }
}

/// PNG `bytes` as `cvd` sees them.
pub(crate) fn simulate_png(cmd: &str, bytes: &[u8], cvd: Cvd) -> Result<Vec<u8>> {
    figcraft_render::cvd::filter_png(bytes, cvd).map_err(|e| bad(cmd, e))
}

fn simulate(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "a11y.simulate";
    let cvd = cvd_param(cmd, p)?.ok_or_else(|| bad(cmd, "cvd: protanopia, deuteranopia, tritanopia or achromatopsia"))?;
    let doc = s.doc()?;
    let mut ids = match get(p, "ids") {
        Some(Value::Array(_)) => req_targets(cmd, s, p)?,
        _ => Vec::new(),
    };
    if ids.is_empty() {
        ids = doc.get(s.page()?)?.children.clone();
    }
    if ids.is_empty() {
        return Err(EngineError::Disabled(cmd.into(), "the page is empty".into()));
    }
    let scale = num_or(p, "scale", 1.0).clamp(0.01, ExportSize::MAX_SCALE);
    let png = super::file::export_bytes(doc, &ids, ExportFormat::Png, scale)?;
    let png = simulate_png(cmd, &png, cvd)?;
    match string(p, "path") {
        Some(path) => {
            super::file::write(&path, &png)?;
            Ok(json!({"path": path, "bytes": png.len(), "format": "png", "cvd": cvd.id()}))
        }
        None => Ok(json!({"format": "png", "cvd": cvd.id(), "data": base64::engine::general_purpose::STANDARD.encode(&png)})),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::Session;
    use crate::params::id_of;

    fn ex(s: &mut Session, cmd: &str, p: serde_json::Value) -> serde_json::Value {
        match s.execute(cmd, &p) {
            Ok(v) => v,
            Err(e) => panic!("{cmd} {p}: {e}"),
        }
    }

    fn make(s: &mut Session) -> (figcraft_doc::NodeId, figcraft_doc::NodeId) {
        let f = id_of(&ex(s, "frame.create", json!({"x": 0, "y": 0, "width": 200, "height": 100, "props": {"fill": "#FFFFFF"}}))["id"]).unwrap();
        let t = id_of(&ex(s, "text.create", json!({"text": "Hello", "x": 10, "y": 10, "parent": f, "props": {"fill": "#767676"}}))["id"]).unwrap();
        (f, t)
    }

    #[test]
    fn pair_767676_on_white_is_4_54_and_passes_aa_only() {
        let mut s = Session::new();
        let r = ex(&mut s, "a11y.contrast", json!({"fg": "#767676", "bg": "#FFFFFF"}));
        assert_eq!(r["ratio"], json!(4.54));
        assert_eq!(r["aa"], json!(true));
        assert_eq!(r["aaa"], json!(false));
        let r = ex(&mut s, "a11y.contrast", json!({"fg": "#777777", "bg": "#FFFFFF"}));
        assert_eq!((r["ratio"].clone(), r["aa"].clone()), (json!(4.48), json!(false)));
        let r = ex(&mut s, "a11y.contrast", json!({"fg": "#777777", "bg": "#FFFFFF", "fontSize": 24}));
        assert_eq!((r["large"].clone(), r["aa"].clone()), (json!(true), json!(true)));
        assert_eq!(ex(&mut s, "a11y.contrast", json!({"fg": "#000", "bg": "#fff"}))["apcaLc"], json!(106.0));
        assert!(s.execute("a11y.contrast", &json!({"fg": "nope", "bg": "#fff"})).is_err());
    }

    #[test]
    fn check_finds_the_frame_behind_text_and_suggests_a_fix() {
        let mut s = Session::new();
        let (_, t) = make(&mut s);
        let r = ex(&mut s, "a11y.checkContrast", json!({"level": "AAA"}));
        assert_eq!(r["checked"], json!(1));
        let f = &r["failures"][0];
        assert_eq!(
            (f["id"].clone(), f["ratio"].clone(), f["bg"].clone(), f["fg"].clone()),
            (json!(t.0), json!(4.54), json!("#FFFFFF"), json!("#767676"))
        );
        assert_eq!(f["fixable"], json!(true));
        assert!(f["suggestion"]["ratio"].as_f64().unwrap() >= 7.0);
        let r = ex(&mut s, "a11y.checkContrast", json!({}));
        assert_eq!((r["failures"].as_array().unwrap().len(), r["passed"].clone()), (0, json!(1)));
    }

    #[test]
    fn a_text_colour_variable_is_checked_in_each_mode() {
        let mut s = Session::new();
        let (_, t) = make(&mut s);
        ex(&mut s, "variable.createCollection", json!({"name": "Theme", "modes": ["Light", "Dark"]}));
        ex(
            &mut s,
            "variable.create",
            json!({"collection": "Theme", "name": "ink", "type": "color", "values": {"Light": "#222222", "Dark": "#333333"}}),
        );
        ex(&mut s, "variable.bind", json!({"ids": [t.0], "field": "fill", "variable": "ink"}));
        let r = ex(&mut s, "a11y.checkContrast", json!({}));
        let fails = r["failures"].as_array().unwrap();
        assert_eq!(fails.len(), 0, "{r}");
        // Dark text on a white frame passes in both modes; make the frame dark in the Dark mode.
        ex(
            &mut s,
            "variable.create",
            json!({"collection": "Theme", "name": "surface", "type": "color", "values": {"Light": "#FFFFFF", "Dark": "#202020"}}),
        );
        let f = s.doc().unwrap().node(t).unwrap().parent.unwrap();
        ex(&mut s, "variable.bind", json!({"ids": [f.0], "field": "fill", "variable": "surface"}));
        let r = ex(&mut s, "a11y.checkContrast", json!({}));
        let fails = r["failures"].as_array().unwrap();
        assert_eq!(fails.len(), 1, "{r}");
        assert_eq!((fails[0]["mode"].clone(), fails[0]["boundVariable"].clone()), (json!("Theme/Dark"), json!("ink")));
        assert_eq!(r["passed"], json!(1));
    }

    #[test]
    fn gradient_behind_text_is_reported_unknown() {
        let mut s = Session::new();
        let (f, _) = make(&mut s);
        let g = figcraft_color::Gradient::default_for(figcraft_color::GradientKind::Linear, figcraft_color::Color::BLACK);
        s.doc_mut().unwrap().get_mut(f).unwrap().fills = vec![figcraft_color::Paint::gradient(g)];
        let r = ex(&mut s, "a11y.checkContrast", json!({}));
        assert_eq!((r["failures"].as_array().unwrap().len(), r["unknown"].as_array().unwrap().len()), (0, 1), "{r}");
    }

    fn fill_of_layer(s: &mut Session, id: figcraft_doc::NodeId) -> String {
        match &s.doc().unwrap().node(id).unwrap().fills[0].kind {
            figcraft_color::PaintKind::Solid { color } => color.to_css_hex(),
            _ => panic!("not solid"),
        }
    }

    #[test]
    fn fix_contrast_keeps_the_hue_and_is_one_undo_step() {
        let mut s = Session::new();
        let (_, t) = make(&mut s);
        ex(&mut s, "node.set", json!({"ids": [t], "props": {"fill": "#9FB4E8"}}));
        let dry = ex(&mut s, "a11y.fixContrast", json!({"dryRun": true}));
        assert_eq!((dry["changed"].as_array().unwrap().len(), fill_of_layer(&mut s, t)), (1, "#9FB4E8".to_string()), "{dry}");
        let r = ex(&mut s, "a11y.fixContrast", json!({}));
        assert_eq!(r["changed"][0]["changed"], json!("text"), "{r}");
        assert_eq!(r["remaining"], json!(0), "{r}");
        let now = figcraft_color::Color::from_hex(&fill_of_layer(&mut s, t)).unwrap();
        assert!(now.b > now.r && now.b > now.g, "hue kept: {now:?}");
        assert!(r["changed"][0]["ratio"].as_f64().unwrap() >= 4.5);
        assert_eq!(ex(&mut s, "a11y.checkContrast", json!({}))["failures"].as_array().unwrap().len(), 0);
        ex(&mut s, "edit.undo", json!({}));
        assert_eq!(fill_of_layer(&mut s, t), "#9FB4E8");
        // Passing text is left alone.
        ex(&mut s, "node.set", json!({"ids": [t], "props": {"fill": "#111111"}}));
        let r = ex(&mut s, "a11y.fixContrast", json!({}));
        assert_eq!((r["changed"].as_array().unwrap().len(), r["skipped"].as_array().unwrap().len()), (0, 0));
    }

    #[test]
    fn fix_contrast_reports_variable_bound_text_instead_of_unbinding_it() {
        let mut s = Session::new();
        let (_, t) = make(&mut s);
        ex(&mut s, "variable.createCollection", json!({"name": "Tokens"}));
        ex(&mut s, "variable.create", json!({"collection": "Tokens", "name": "Text/Muted", "type": "color", "value": "#BBBBBB"}));
        ex(&mut s, "variable.bind", json!({"ids": [t], "field": "fill", "variable": "Text/Muted"}));
        let r = ex(&mut s, "a11y.fixContrast", json!({}));
        assert_eq!(r["changed"].as_array().unwrap().len(), 0, "{r}");
        assert_eq!(r["skipped"][0]["variable"], json!("Text/Muted"), "{r}");
        assert!(s.doc().unwrap().node(t).unwrap().bound.values().any(|v| !v.is_empty()), "still bound");
    }

    #[test]
    fn fix_contrast_can_change_the_background_instead() {
        let mut s = Session::new();
        let (f, t) = make(&mut s);
        ex(&mut s, "node.set", json!({"ids": [t], "props": {"fill": "#FFFFFF"}}));
        ex(&mut s, "node.set", json!({"ids": [f], "props": {"fill": "#8899AA"}}));
        let r = ex(&mut s, "a11y.fixContrast", json!({"prefer": "background"}));
        assert_eq!((r["changed"][0]["changed"].clone(), r["remaining"].clone()), (json!("background"), json!(0)), "{r}");
        assert_eq!(fill_of_layer(&mut s, t), "#FFFFFF");
        assert_ne!(fill_of_layer(&mut s, f), "#8899AA");
        // Over the bare page there is no layer to change: the text colour moves instead.
        let mut s = Session::new();
        let t = id_of(&ex(&mut s, "text.create", json!({"text": "Hi", "x": 0, "y": 0, "props": {"fill": "#CCCCCC"}}))["id"]).unwrap();
        let r = ex(&mut s, "a11y.fixContrast", json!({"prefer": "background"}));
        assert_eq!(r["changed"][0]["changed"], json!("text"), "{r}");
        assert_ne!(fill_of_layer(&mut s, t), "#CCCCCC");
    }

    #[test]
    fn fix_contrast_refuses_junk() {
        let mut s = Session::new();
        make(&mut s);
        for p in [
            json!({"prefer": "both"}),
            json!({"prefer": 5}),
            json!({"level": "B"}),
            json!({"dryRun": ["x"]}),
            json!({"colour": "#000"}),
            json!({"ids": "x"}),
            json!({"root": "nope"}),
            json!({"root": 99999}),
            json!({"root": 1, "ids": [1]}),
        ] {
            assert!(s.execute("a11y.fixContrast", &p).is_err(), "{p}");
        }
    }

    #[test]
    fn simulate_returns_a_png_and_export_takes_cvd() {
        use base64::Engine as _;
        let mut s = Session::new();
        make(&mut s);
        ex(&mut s, "shape.rectangle", json!({"x": 300, "y": 0, "width": 50, "height": 50, "props": {"fill": "#FF0000"}}));
        let r = ex(&mut s, "a11y.simulate", json!({"cvd": "deuteranopia"}));
        let png = base64::engine::general_purpose::STANDARD.decode(r["data"].as_str().unwrap()).unwrap();
        assert!(png.starts_with(b"\x89PNG"));
        let plain = ex(&mut s, "file.export", json!({"format": "png"}));
        let sim = ex(&mut s, "file.export", json!({"format": "png", "cvd": "protanopia"}));
        assert_ne!(plain["data"], sim["data"]);
        assert!(s.execute("a11y.simulate", &json!({"cvd": "purple"})).is_err());
        assert!(s.execute("file.export", &json!({"format": "svg", "cvd": "protanopia"})).is_err());
    }
}
