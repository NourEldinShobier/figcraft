//! Accessibility checks beyond text contrast: tap-target size (WCAG 2.5.8), non-text contrast
//! (1.4.11), alt text on images and icons, the order a screen reader reads a screen in, and
//! `a11y.audit`, which runs all of them (text contrast included).
//!
//! A control is a layer with interactions, or a frame, component or instance named like a button,
//! link, input and so on. Colours are the default variable mode; boxes are bounding boxes.
//! Alt text lives in layer plugin data under [`ALT_KEY`] (`node.set {props: {alt}}`; "" marks a
//! decorative layer).

use std::collections::BTreeSet;

use figcraft_color::a11y as ca;
use figcraft_color::{BlendMode, Color, PaintKind};
use figcraft_doc::{Document, LayoutMode, Node, NodeId, NodeKind};
use figcraft_geom::Rect;
use serde_json::{Value, json};

use super::CommandSpec;
use crate::params::{get, opt_id};
use crate::{Result, Session, bad, spec};

/// Plugin-data key that holds a layer's alt text.
pub const ALT_KEY: &str = "figcraft.alt";

const LISTED: usize = 100;
const MAX_NODES: usize = 50_000;
/// Frames at most this wide are phones: tap targets there need 44, elsewhere 24.
const MOBILE_WIDTH: f64 = 500.0;
const EPS: f64 = 0.01;
const NON_TEXT_RATIO: f64 = 3.0;
/// Shapes in a control up to this size count as its icon.
const MAX_ICON: f64 = 48.0;
const CONTROLS: [&str; 14] =
    ["button", "btn", "link", "tab", "toggle", "switch", "checkbox", "radio", "chip", "cta", "input", "field", "textfield", "dropdown"];
/// Controls whose outline is what identifies them, label or not.
const FIELDS: [&str; 7] = ["input", "field", "textfield", "checkbox", "radio", "toggle", "switch"];
const IMAGES: [&str; 8] = ["icon", "logo", "illustration", "image", "img", "photo", "picture", "avatar"];

pub fn specs() -> Vec<CommandSpec> {
    vec![
        spec!(
            "a11y.checkTargets",
            "Check target sizes",
            "{root? (a layer), page? (id, name or \"all\"; default the open page), limit? (listed, default 100)}  controls (layers with interactions; frames, components and instances named like a button, link, tab, toggle, input…) under 24x24 (44x44 in frames up to 500 wide) → {checked, passed, total, listed, truncated?, failures: [{id, name, width, height, required, reason, fix}]}",
            false,
            check_targets
        ),
        spec!(
            "a11y.checkNonText",
            "Check non-text contrast",
            "{root?, page?, limit?}  WCAG 1.4.11: a control's fill or stroke against what is behind it (only for controls with no text label, and fields, checkboxes, toggles) and the icons inside it against the control's background must reach 3:1 → {checked, passed, total, listed, unknown, failures: [{kind: boundary | icon, id, control, name, fg, bg, ratio, required}]}. Gradient, image and blended paints count as `unknown`",
            false,
            check_non_text
        ),
        spec!(
            "a11y.checkAlt",
            "Check alt text",
            "{root?, page?, limit?}  images (a childless layer with an image fill), vectors and groups named icon, logo, image, photo, illustration, avatar without alt text (set with node.set {props: {alt}}; \"\" marks decorative; icons inside a control with a text label are skipped) → {checked, withAlt, decorative, skipped, total, listed, missing: [{id, name, type}]}",
            false,
            check_alt
        ),
        spec!(
            "a11y.readingOrder",
            "Reading order",
            "{root?, page?, limit? (items listed, default 100)}  the text, images and controls of each screen in the order a screen reader reads them: auto-layout order in auto-layout frames, else top to bottom and left to right by rows → {total, listed, truncated?, screens: [{id, name, items: [{role: text | image | control, id, text?, name?, alt?}]}]}. A control is read as one item with the text inside it",
            false,
            reading_order
        ),
        spec!(
            "a11y.audit",
            "Accessibility audit",
            "{root?, page?, limit? (listed per check, default 100)}  runs text contrast, target sizes, non-text contrast and alt text, and counts the reading order → {issues, checks: {contrast, targets, nonText, alt, readingOrder} (counts), details: {contrast, targets, nonText, alt} (the first `limit` findings of each)}",
            false,
            audit
        ),
    ]
}

fn scope(s: &Session, cmd: &str, p: &Value) -> Result<(Vec<NodeId>, usize)> {
    let limit = match get(p, "limit") {
        None | Some(Value::Null) => LISTED,
        Some(v) => v.as_u64().filter(|n| *n <= 5000).ok_or_else(|| bad(cmd, "`limit` must be an integer from 0 to 5000"))? as usize,
    };
    let roots = match opt_id(cmd, p, "root")? {
        Some(r) => vec![r],
        None => super::edit::structure_find::pages(s, cmd, p)?,
    };
    Ok((roots, limit))
}

fn has_word(name: &str, words: &[&str]) -> bool {
    let l = name.to_lowercase();
    l.split(|c: char| !c.is_alphanumeric())
        .any(|w| words.iter().any(|h| w == *h || w.strip_suffix('s') == Some(*h) || (h.len() >= 5 && w.starts_with(h))))
}

fn is_control(n: &Node) -> bool {
    !n.is_page()
        && (!n.interactions.is_empty()
            || (matches!(n.kind, NodeKind::Instance { .. } | NodeKind::Component { .. } | NodeKind::Frame) && has_word(&n.name, &CONTROLS)))
}

fn has_text(doc: &Document, id: NodeId) -> bool {
    doc.subtree(id).iter().any(|i| doc.node(*i).is_some_and(|n| n.visible && matches!(n.kind, NodeKind::Text(_))))
}

/// Visible layers under `roots` in document order. `f(layer, inside)` says whether to go into the
/// layer's children; `inside` is true under a control that has a text label.
fn walk(doc: &Document, roots: &[NodeId], mut f: impl FnMut(&Node, bool) -> bool) {
    let mut seen = BTreeSet::new();
    let mut stack: Vec<(NodeId, bool)> = roots.iter().rev().map(|r| (*r, false)).collect();
    while let Some((id, inside)) = stack.pop() {
        if !seen.insert(id) || seen.len() > MAX_NODES {
            continue;
        }
        let Some(n) = doc.node(id).filter(|n| n.visible) else { continue };
        let (descend, inside) = if n.is_page() { (true, false) } else { (f(n, inside), inside || (is_control(n) && has_text(doc, id))) };
        if descend {
            stack.extend(n.children.iter().rev().map(|c| (*c, inside)));
        }
    }
}

fn top_width(doc: &Document, n: &Node) -> f64 {
    let mut top = n;
    for _ in 0..10_000 {
        match top.parent.and_then(|p| doc.node(p)) {
            Some(p) if !p.is_page() => top = p,
            _ => break,
        }
    }
    top.width
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// A layer's visible top paint.
enum P {
    None,
    Solid(Color),
    Complex,
}

fn top_paint(n: &Node, strokes: bool) -> P {
    let list = if strokes { &n.strokes } else { &n.fills };
    let Some(p) = list.iter().rev().find(|p| p.visible) else { return P::None };
    match &p.kind {
        PaintKind::Solid { color } if p.blend == BlendMode::Normal => P::Solid(color.with_alpha(color.a * p.opacity * n.opacity)),
        _ => P::Complex,
    }
}

/// The colour behind a layer: its ancestors' solid fills over the page colour. None when an
/// ancestor paints a gradient, image or blended fill.
fn backdrop(doc: &Document, n: &Node) -> Option<Color> {
    let mut layers: Vec<Color> = Vec::new();
    let mut cur = n.parent;
    for _ in 0..10_000 {
        let Some(a) = cur.and_then(|c| doc.node(c)) else { break };
        if let NodeKind::Page { background, .. } = &a.kind {
            return Some(layers.iter().rev().fold(*background, |under, c| ca::over(*c, under)));
        }
        match top_paint(a, false) {
            P::None => {}
            P::Complex => return None,
            P::Solid(c) => {
                layers.push(c);
                if c.a >= 0.999 {
                    break;
                }
            }
        }
        cur = a.parent;
    }
    Some(layers.iter().rev().fold(Color::WHITE, |under, c| ca::over(*c, under)))
}

fn capped(mut v: Vec<Value>, limit: usize) -> (usize, Vec<Value>) {
    let total = v.len();
    v.truncate(limit);
    (total, v)
}

fn finish(checked: usize, failing: usize, total: usize, list: Vec<Value>, key: &str) -> Value {
    let mut out = json!({"checked": checked, "passed": checked.saturating_sub(failing), "total": total, "listed": list.len()});
    if total > list.len() {
        out["truncated"] = json!(true);
    }
    out[key] = json!(list);
    out
}

fn check_targets(s: &mut Session, p: &Value) -> Result<Value> {
    let (roots, limit) = scope(s, "a11y.checkTargets", p)?;
    let doc = s.doc()?;
    let (mut checked, mut fails) = (0usize, Vec::new());
    walk(doc, &roots, |n, _| {
        if is_control(n) {
            checked += 1;
            let min = if top_width(doc, n) <= MOBILE_WIDTH { 44.0 } else { 24.0 };
            if n.width < min - EPS || n.height < min - EPS {
                fails.push(json!({
                    "id": n.id, "name": n.name, "width": round2(n.width), "height": round2(n.height), "required": min,
                    "reason": if n.interactions.is_empty() { "named like a control" } else { "has interactions" },
                    "fix": {"command": "node.set", "params": {"ids": [n.id], "props": {"width": n.width.max(min), "height": n.height.max(min)}}},
                }));
            }
        }
        !matches!(n.kind, NodeKind::Instance { .. })
    });
    let n = fails.len();
    let (total, list) = capped(fails, limit);
    Ok(finish(checked, n, total, list, "failures"))
}

fn check_non_text(s: &mut Session, p: &Value) -> Result<Value> {
    let (roots, limit) = scope(s, "a11y.checkNonText", p)?;
    let doc = s.doc()?;
    let (mut checked, mut unknown, mut failing) = (0usize, 0usize, 0usize);
    let mut fails: Vec<Value> = Vec::new();
    walk(doc, &roots, |n, _| {
        if !is_control(n) {
            return true;
        }
        checked += 1;
        let before = fails.len();
        let Some(bd) = backdrop(doc, n) else {
            unknown += 1;
            return true;
        };
        let (fill, stroke) = (top_paint(n, false), top_paint(n, true));
        let mut entry = |kind: &str, id: NodeId, fg: Color, bg: Color, ratio: f64| {
            fails.push(json!({
                "kind": kind, "id": id, "control": n.id, "name": n.name, "fg": ca::over(fg, bg).to_css_hex(), "bg": ca::over(bg, Color::WHITE).to_css_hex(),
                "ratio": round2(ratio), "required": NON_TEXT_RATIO,
            }));
        };
        if !has_text(doc, n.id) || has_word(&n.name, &FIELDS) {
            let solid: Vec<Color> = [&fill, &stroke].into_iter().filter_map(|x| if let P::Solid(c) = x { Some(*c) } else { None }).collect();
            if let Some((c, r)) = solid.iter().map(|c| (*c, ca::wcag_ratio(*c, bd))).max_by(|a, b| a.1.total_cmp(&b.1)) {
                if r < NON_TEXT_RATIO {
                    entry("boundary", n.id, c, bd, r);
                }
            } else if matches!(fill, P::Complex) || matches!(stroke, P::Complex) {
                unknown += 1;
            }
        }
        let icon_bg = match fill {
            P::Solid(c) => ca::over(c, bd),
            P::None => bd,
            P::Complex => {
                unknown += 1;
                return true;
            }
        };
        for i in doc.subtree(n.id).into_iter().filter(|i| *i != n.id) {
            let Some(ic) = doc.node(i).filter(|k| {
                k.visible
                    && matches!(
                        k.kind,
                        NodeKind::Vector { .. } | NodeKind::Line | NodeKind::Polygon { .. } | NodeKind::Star { .. } | NodeKind::Ellipse { .. }
                    )
                    && k.width <= MAX_ICON
                    && k.height <= MAX_ICON
            }) else {
                continue;
            };
            let c = match (top_paint(ic, false), top_paint(ic, true)) {
                (P::Solid(c), _) | (P::None, P::Solid(c)) => c,
                _ => continue,
            };
            let r = ca::wcag_ratio(c, icon_bg);
            if r < NON_TEXT_RATIO {
                entry("icon", i, c, icon_bg, r);
            }
        }
        if fails.len() > before {
            failing += 1;
        }
        true
    });
    let (total, list) = capped(fails, limit);
    let mut out = finish(checked, failing, total, list, "failures");
    out["unknown"] = json!(unknown);
    Ok(out)
}

fn image_like(n: &Node) -> bool {
    if n.is_page() || matches!(n.kind, NodeKind::Text(_)) {
        return false;
    }
    let img = n.fills.iter().any(|f| f.visible && matches!(f.kind, PaintKind::Image(_)));
    (img && n.children.is_empty())
        || matches!(n.kind, NodeKind::Vector { .. })
        || (!n.children.is_empty() && !is_control(n) && has_word(&n.name, &IMAGES))
}

fn check_alt(s: &mut Session, p: &Value) -> Result<Value> {
    let (roots, limit) = scope(s, "a11y.checkAlt", p)?;
    let doc = s.doc()?;
    let (mut checked, mut with_alt, mut decorative, mut skipped) = (0usize, 0usize, 0usize, 0usize);
    let mut missing = Vec::new();
    walk(doc, &roots, |n, inside| {
        if !image_like(n) {
            return true;
        }
        checked += 1;
        match n.plugin_data.get(ALT_KEY) {
            Some(a) if a.is_empty() => decorative += 1,
            Some(_) => with_alt += 1,
            None if inside => skipped += 1,
            None => missing.push(json!({"id": n.id, "name": n.name, "type": n.kind.type_name()})),
        }
        false
    });
    let (total, list) = capped(missing, limit);
    Ok(json!({
        "checked": checked, "withAlt": with_alt, "decorative": decorative, "skipped": skipped, "total": total, "listed": list.len(),
        "truncated": (total > list.len()).then_some(true), "missing": list,
    }))
}

/// Visible children in the order a screen reader meets them.
fn ordered(doc: &Document, n: &Node) -> Vec<NodeId> {
    let kids: Vec<NodeId> = n.children.iter().copied().filter(|c| doc.node(*c).is_some_and(|k| k.visible)).collect();
    positional(doc, kids, n.layout.as_ref().is_some_and(|l| matches!(l.mode, LayoutMode::Horizontal | LayoutMode::Vertical)))
}

/// Rows from the top, each from the left; `keep` leaves the given order (auto layout).
fn positional(doc: &Document, kids: Vec<NodeId>, keep: bool) -> Vec<NodeId> {
    if keep {
        return kids;
    }
    let (mut placed, rest): (Vec<(NodeId, Rect)>, Vec<NodeId>) = kids.iter().fold((Vec::new(), Vec::new()), |(mut a, mut b), id| {
        match doc.world_bounds(*id) {
            Some(r) => a.push((*id, r)),
            None => b.push(*id),
        }
        (a, b)
    });
    placed.sort_by(|a, b| a.1.y0.total_cmp(&b.1.y0).then(a.1.x0.total_cmp(&b.1.x0)));
    let mut rows: Vec<Vec<(NodeId, Rect)>> = Vec::new();
    let mut anchor = (0.0, 0.0);
    for it in placed {
        match rows.last_mut() {
            Some(row) if it.1.y0 < anchor.0 + anchor.1 * 0.5 => row.push(it),
            _ => {
                anchor = (it.1.y0, it.1.y1 - it.1.y0);
                rows.push(vec![it]);
            }
        }
    }
    let mut out = Vec::new();
    for mut row in rows {
        row.sort_by(|a, b| a.1.x0.total_cmp(&b.1.x0));
        out.extend(row.into_iter().map(|(id, _)| id));
    }
    out.extend(rest);
    out
}

fn excerpt(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(80).collect()
}

struct Order<'a> {
    doc: &'a Document,
    limit: usize,
    total: usize,
}

impl Order<'_> {
    fn visit(&mut self, id: NodeId, depth: usize, items: &mut Vec<Value>) {
        let Some(n) = self.doc.node(id).filter(|n| n.visible) else { return };
        if depth > 500 || self.total >= MAX_NODES {
            return;
        }
        let mut push = |me: &mut Self, v: Value| {
            me.total += 1;
            if items.len() < me.limit {
                items.push(v);
            }
        };
        if is_control(n) {
            let label: Vec<String> = self
                .doc
                .subtree(id)
                .iter()
                .filter_map(|i| self.doc.node(*i).filter(|k| k.visible))
                .filter_map(|k| if let NodeKind::Text(t) = &k.kind { Some(t.characters.clone()) } else { None })
                .collect();
            push(self, json!({"role": "control", "id": id, "name": n.name, "text": excerpt(&label.join(" "))}));
        } else if let NodeKind::Text(t) = &n.kind {
            push(self, json!({"role": "text", "id": id, "text": excerpt(&t.characters)}));
        } else if image_like(n) {
            push(self, json!({"role": "image", "id": id, "name": n.name, "alt": n.plugin_data.get(ALT_KEY)}));
        } else {
            for c in ordered(self.doc, n) {
                self.visit(c, depth + 1, items);
            }
        }
    }
}

fn reading_order(s: &mut Session, p: &Value) -> Result<Value> {
    let (roots, limit) = scope(s, "a11y.readingOrder", p)?;
    let doc = s.doc()?;
    let mut screens: Vec<NodeId> = Vec::new();
    for r in roots {
        match doc.node(r) {
            Some(n) if n.is_page() => {
                let kids: Vec<NodeId> = n.children.iter().copied().filter(|c| doc.node(*c).is_some_and(|k| k.visible)).collect();
                screens.extend(positional(doc, kids, false));
            }
            Some(_) => screens.push(r),
            None => {}
        }
    }
    let mut ord = Order { doc, limit, total: 0 };
    let mut out = Vec::new();
    for sid in screens {
        let mut items = Vec::new();
        ord.visit(sid, 0, &mut items);
        let name = doc.node(sid).map(|n| n.name.clone()).unwrap_or_default();
        out.push(json!({"id": sid, "name": name, "items": items}));
    }
    let listed: usize = out.iter().map(|o| o["items"].as_array().map_or(0, Vec::len)).sum();
    Ok(json!({"total": ord.total, "listed": listed, "truncated": (ord.total > listed).then_some(true), "screens": out}))
}

fn audit(s: &mut Session, p: &Value) -> Result<Value> {
    let (roots, limit) = scope(s, "a11y.audit", p)?;
    let (mut c_checked, mut c_unknown, mut c_fails) = (0usize, 0usize, Vec::new());
    for r in &roots {
        let v = super::a11y::check_contrast(s, &json!({"ids": [r]}))?;
        c_checked += v["checked"].as_u64().unwrap_or(0) as usize;
        c_unknown += v["unknown"].as_array().map_or(0, Vec::len);
        c_fails.extend(v["failures"].as_array().cloned().unwrap_or_default());
    }
    let targets = check_targets(s, p)?;
    let non_text = check_non_text(s, p)?;
    let alt = check_alt(s, p)?;
    let order = reading_order(s, &json!({"root": p.get("root"), "page": p.get("page"), "limit": 0}))?;
    let c_total = c_fails.len();
    c_fails.truncate(limit);
    let count = |v: &Value| v["total"].as_u64().unwrap_or(0);
    let issues = c_total as u64 + count(&targets) + count(&non_text) + count(&alt);
    Ok(json!({
        "issues": issues,
        "checks": {
            "contrast": {"checked": c_checked, "failures": c_total, "unknown": c_unknown},
            "targets": {"checked": targets["checked"], "failures": targets["total"]},
            "nonText": {"checked": non_text["checked"], "failures": non_text["total"], "unknown": non_text["unknown"]},
            "alt": {"checked": alt["checked"], "missing": alt["total"]},
            "readingOrder": {"items": order["total"]},
        },
        "details": {"contrast": c_fails, "targets": targets["failures"], "nonText": non_text["failures"], "alt": alt["missing"]},
    }))
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::Session;
    use crate::params::id_of;

    fn ex(s: &mut Session, cmd: &str, p: Value) -> Value {
        match s.execute(cmd, &p) {
            Ok(v) => v,
            Err(e) => panic!("{cmd} {p}: {e}"),
        }
    }

    fn id(v: &Value) -> figcraft_doc::NodeId {
        id_of(&v["id"]).unwrap()
    }

    fn screen(s: &mut Session, w: f64) -> figcraft_doc::NodeId {
        id(&ex(s, "frame.create", json!({"x": 0, "y": 0, "width": w, "height": 800, "props": {"fill": "#FFFFFF"}})))
    }

    #[test]
    fn small_controls_fail_by_width_of_their_screen() {
        let mut s = Session::new();
        let phone = screen(&mut s, 400.0);
        let small = id(&ex(&mut s, "frame.create", json!({"name": "Button", "parent": phone, "x": 0, "y": 0, "width": 30, "height": 30})));
        ex(&mut s, "frame.create", json!({"name": "Button", "parent": phone, "x": 0, "y": 100, "width": 48, "height": 48}));
        ex(&mut s, "frame.create", json!({"name": "Table", "parent": phone, "x": 0, "y": 200, "width": 10, "height": 10}));
        let r = ex(&mut s, "a11y.checkTargets", json!({}));
        assert_eq!(
            (r["checked"].clone(), r["total"].clone(), r["failures"][0]["id"].clone(), r["failures"][0]["required"].clone()),
            (json!(2), json!(1), json!(small.0), json!(44.0)),
            "{r}"
        );
        let desk = screen(&mut s, 1200.0);
        ex(&mut s, "frame.create", json!({"name": "Link", "parent": desk, "x": 0, "y": 0, "width": 30, "height": 20}));
        let r = ex(&mut s, "a11y.checkTargets", json!({"root": desk.0}));
        assert_eq!((r["total"].clone(), r["failures"][0]["required"].clone()), (json!(1), json!(24.0)), "{r}");
    }

    #[test]
    fn a_pale_field_border_fails_non_text_contrast_and_a_dark_one_passes() {
        let mut s = Session::new();
        let f = screen(&mut s, 400.0);
        let bad = id(&ex(&mut s, "frame.create", json!({"name": "Input", "parent": f, "x": 0, "y": 0, "width": 200, "height": 48})));
        ex(&mut s, "node.set", json!({"ids": [bad.0], "props": {"strokes": [{"type": "solid", "color": "#DDDDDD"}]}}));
        let good = id(&ex(&mut s, "frame.create", json!({"name": "Input", "parent": f, "x": 0, "y": 100, "width": 200, "height": 48})));
        ex(&mut s, "node.set", json!({"ids": [good.0], "props": {"strokes": [{"type": "solid", "color": "#555555"}]}}));
        let r = ex(&mut s, "a11y.checkNonText", json!({}));
        assert_eq!(
            (r["checked"].clone(), r["total"].clone(), r["failures"][0]["id"].clone(), r["failures"][0]["kind"].clone()),
            (json!(2), json!(1), json!(bad.0), json!("boundary")),
            "{r}"
        );
        assert!(r["failures"][0]["ratio"].as_f64().unwrap() < 3.0);
    }

    #[test]
    fn a_pale_icon_in_a_button_fails() {
        let mut s = Session::new();
        let f = screen(&mut s, 400.0);
        let b = id(&ex(
            &mut s,
            "frame.create",
            json!({"name": "Icon Button", "parent": f, "x": 0, "y": 0, "width": 48, "height": 48, "props": {"fill": "#FFFFFF"}}),
        ));
        ex(&mut s, "shape.ellipse", json!({"parent": b.0, "x": 10, "y": 10, "width": 20, "height": 20, "props": {"fill": "#EEEEEE"}}));
        let r = ex(&mut s, "a11y.checkNonText", json!({}));
        assert!(r["failures"].as_array().unwrap().iter().any(|f| f["kind"] == json!("icon")), "{r}");
    }

    #[test]
    fn alt_text_is_stored_read_back_and_clears_the_finding() {
        let mut s = Session::new();
        let f = screen(&mut s, 400.0);
        let img = id(&ex(
            &mut s,
            "shape.rectangle",
            json!({"parent": f.0, "name": "Hero", "x": 0, "y": 0, "width": 100, "height": 100, "props": {"fills": [{"type": "solid", "color": "#FF0000"}]}}),
        ));
        s.doc_mut().unwrap().get_mut(img).unwrap().fills = vec![figcraft_color::Paint::image("h", figcraft_color::ImageScaleMode::Fill)];
        let r = ex(&mut s, "a11y.checkAlt", json!({}));
        assert_eq!((r["total"].clone(), r["missing"][0]["id"].clone()), (json!(1), json!(img.0)), "{r}");
        ex(&mut s, "node.set", json!({"ids": [img.0], "props": {"alt": "A red banner"}}));
        assert_eq!(s.doc().unwrap().node(img).unwrap().plugin_data.get(super::ALT_KEY).map(String::as_str), Some("A red banner"));
        let r = ex(&mut s, "a11y.checkAlt", json!({}));
        assert_eq!((r["total"].clone(), r["withAlt"].clone()), (json!(0), json!(1)), "{r}");
        ex(&mut s, "node.set", json!({"ids": [img.0], "props": {"alt": null}}));
        assert_eq!(ex(&mut s, "a11y.checkAlt", json!({}))["total"], json!(1));
        assert!(s.execute("node.set", &json!({"ids": [img.0], "props": {"alt": {"a": 1}}})).is_err());
    }

    #[test]
    fn reading_order_goes_by_rows_and_keeps_auto_layout_order() {
        let mut s = Session::new();
        let f = screen(&mut s, 400.0);
        // Created out of reading order.
        for (t, x, y) in [("third", 0, 200), ("second", 200, 10), ("first", 0, 12)] {
            ex(&mut s, "text.create", json!({"text": t, "x": x, "y": y, "parent": f}));
        }
        let r = ex(&mut s, "a11y.readingOrder", json!({"root": f.0}));
        let texts: Vec<&str> = r["screens"][0]["items"].as_array().unwrap().iter().map(|i| i["text"].as_str().unwrap()).collect();
        assert_eq!(texts, ["first", "second", "third"], "{r}");
        ex(&mut s, "node.set", json!({"ids": [f.0], "props": {"layout": "vertical"}}));
        let r = ex(&mut s, "a11y.readingOrder", json!({"root": f.0}));
        let texts: Vec<&str> = r["screens"][0]["items"].as_array().unwrap().iter().map(|i| i["text"].as_str().unwrap()).collect();
        assert_eq!(texts, ["third", "second", "first"], "{r}");
        let r = ex(&mut s, "a11y.readingOrder", json!({"root": f.0, "limit": 1}));
        assert_eq!((r["total"].clone(), r["listed"].clone(), r["truncated"].clone()), (json!(3), json!(1), json!(true)));
    }

    #[test]
    fn audit_counts_every_check() {
        let mut s = Session::new();
        let f = screen(&mut s, 400.0);
        ex(&mut s, "frame.create", json!({"name": "Button", "parent": f, "x": 0, "y": 0, "width": 20, "height": 20}));
        ex(&mut s, "text.create", json!({"text": "Hi", "x": 10, "y": 100, "parent": f, "props": {"fill": "#CCCCCC"}}));
        let r = ex(&mut s, "a11y.audit", json!({}));
        assert_eq!(r["checks"]["targets"]["failures"], json!(1), "{r}");
        assert_eq!(r["checks"]["contrast"]["failures"], json!(1), "{r}");
        assert_eq!(r["checks"]["readingOrder"]["items"], json!(2), "{r}");
        assert!(r["issues"].as_u64().unwrap() >= 2);
    }

    #[test]
    fn junk_parameters_are_errors_not_panics() {
        let mut s = Session::new();
        for cmd in ["a11y.checkTargets", "a11y.checkNonText", "a11y.checkAlt", "a11y.readingOrder", "a11y.audit"] {
            for junk in [json!({"root": "x"}), json!({"page": 3}), json!({"root": 99999}), json!({"ids": 4})] {
                let _ = s.execute(cmd, &junk);
            }
            for junk in [json!({"limit": -1}), json!({"limit": "a"}), json!({"limit": 9999})] {
                assert!(s.execute(cmd, &junk).is_err(), "{cmd} {junk}");
            }
        }
    }
}
