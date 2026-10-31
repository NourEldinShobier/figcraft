//! `inspect.lintIcons`: checks an icon set for the inconsistencies an icon designer fixes by hand,
//! as structured issues an agent can act on. Read-only; fixable issues carry the command that fixes them.
//!
//! Rules: `size` (the icon box is the grid), `stroke-width`, `stroke-cap`, `stroke-join`,
//! `corner-radius`, `color` (one colour across the set), `pixel-grid` (edges on whole pixels, or
//! half pixels for odd strokes, so strokes are crisp), `padding` (the art stays inside the live
//! area) and `naming` (no default names, no repeats, one style).

use std::collections::BTreeMap;

use figcraft_doc::{Document, NodeId, NodeKind};
use figcraft_geom::Rect;
use serde::Serialize;
use serde_json::{Value, json};

use crate::cmd::CommandSpec;
use crate::params::{color_of, f64_of, get, string, targets};
use crate::{bad, spec};

const MAX_ISSUES: usize = 500;
const MAX_ICONS: usize = 2000;
const MAX_DEPTH: usize = 4;
const SHAPES: [&str; 7] = ["RECTANGLE", "ELLIPSE", "POLYGON", "STAR", "LINE", "VECTOR", "BOOLEAN_OPERATION"];
const DEFAULT_NAMES: [&str; 14] = [
    "frame",
    "group",
    "vector",
    "rectangle",
    "ellipse",
    "line",
    "polygon",
    "star",
    "component",
    "instance",
    "union",
    "subtract",
    "intersect",
    "exclude",
];

fn label<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v).ok().and_then(|x| x.as_str().map(str::to_string)).unwrap_or_default()
}

fn r3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

struct Leaf {
    id: NodeId,
    name: String,
    weight: Option<f64>,
    cap: Option<String>,
    join: Option<String>,
    radius: Option<f64>,
    /// ("fill" | "stroke", `#RRGGBB`)
    colors: Vec<(&'static str, String)>,
    /// Relative to the icon's top-left.
    bounds: Option<Rect>,
    /// Position in its parent frame.
    rel: (f64, f64),
}

struct Icon {
    id: NodeId,
    name: String,
    /// The icon is a box (frame, group...) rather than a lone shape.
    boxed: bool,
    size: (f64, f64),
    leaves: Vec<Leaf>,
}

fn solid(paints: &[figcraft_color::Paint]) -> Vec<String> {
    paints
        .iter()
        .filter(|p| p.visible)
        .filter_map(|p| match &p.kind {
            figcraft_color::PaintKind::Solid { color } => Some(color.to_css_hex().to_ascii_uppercase()),
            _ => None,
        })
        .collect()
}

fn collect_leaves(doc: &Document, id: NodeId, origin: (f64, f64), out: &mut Vec<Leaf>, depth: usize) {
    let Some(n) = doc.node(id) else { return };
    if !n.visible || depth > 16 || out.len() > 2000 {
        return;
    }
    if SHAPES.contains(&n.kind.type_name()) {
        let mut colors: Vec<(&'static str, String)> = solid(&n.fills).into_iter().map(|c| ("fill", c)).collect();
        let stroke_colors = solid(&n.strokes);
        let stroked = !stroke_colors.is_empty();
        colors.extend(stroke_colors.into_iter().map(|c| ("stroke", c)));
        let rotated = figcraft_geom::rotation_degrees(n.transform).abs() > 1e-6;
        let c = doc.frame_relative(id).as_coeffs();
        out.push(Leaf {
            id,
            name: n.name.clone(),
            weight: stroked.then_some(n.stroke.weight),
            cap: stroked.then(|| {
                if n.stroke.cap_start == n.stroke.cap_end {
                    label(&n.stroke.cap_start)
                } else {
                    format!("{}/{}", label(&n.stroke.cap_start), label(&n.stroke.cap_end))
                }
            }),
            join: stroked.then(|| label(&n.stroke.join)),
            radius: n.corners.uniform().filter(|r| *r > 0.0 && matches!(n.kind, NodeKind::Rectangle)),
            colors,
            bounds: doc.world_bounds(id).filter(|_| !rotated).map(|b| Rect::new(b.x0 - origin.0, b.y0 - origin.1, b.x1 - origin.0, b.y1 - origin.1)),
            rel: (c[4], c[5]),
        });
        return;
    }
    for &k in &n.children {
        collect_leaves(doc, k, origin, out, depth + 1);
    }
}

fn find_icons(doc: &Document, id: NodeId, grid: f64, depth: usize, out: &mut Vec<Icon>) {
    let Some(n) = doc.node(id) else { return };
    if out.len() >= MAX_ICONS || !n.visible {
        return;
    }
    let shape = SHAPES.contains(&n.kind.type_name());
    let small_box = !n.children.is_empty()
        && n.width <= grid * 2.0 + 1e-6
        && n.height <= grid * 2.0 + 1e-6
        && (n.kind.is_frame_like() || matches!(n.kind, NodeKind::Group));
    if shape || small_box {
        let origin = doc.world_bounds(id).map_or((0.0, 0.0), |b| (b.x0, b.y0));
        let mut leaves = Vec::new();
        collect_leaves(doc, id, if shape { (0.0, 0.0) } else { origin }, &mut leaves, 0);
        out.push(Icon { id, name: n.name.clone(), boxed: !shape, size: (n.width, n.height), leaves });
    } else if depth < MAX_DEPTH {
        for &k in &n.children {
            find_icons(doc, k, grid, depth + 1, out);
        }
    }
}

/// The most common value (ties: the smallest), counted by its text.
fn mode(values: impl Iterator<Item = String>) -> Option<String> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for v in values {
        *counts.entry(v).or_default() += 1;
    }
    let best = counts.values().copied().max()?;
    counts.into_iter().find(|(_, n)| *n == best).map(|(v, _)| v)
}

fn name_style(name: &str) -> &'static str {
    let last = name.rsplit('/').next().unwrap_or(name);
    if last.contains(' ') {
        "words"
    } else if last.contains('-') {
        "kebab-case"
    } else if last.contains('_') {
        "snake_case"
    } else if last.chars().any(char::is_uppercase) && last.chars().any(char::is_lowercase) {
        "camel/Pascal case"
    } else {
        "plain"
    }
}

fn is_default_name(name: &str) -> bool {
    let base = name.trim().trim_end_matches(|c: char| c.is_ascii_digit()).trim().to_ascii_lowercase();
    name.trim().is_empty() || DEFAULT_NAMES.contains(&base.as_str())
}

struct Report {
    issues: Vec<Value>,
    total: usize,
    counts: BTreeMap<&'static str, usize>,
}

impl Report {
    #[allow(clippy::too_many_arguments)]
    fn add(
        &mut self,
        rule: &'static str,
        severity: &str,
        id: NodeId,
        name: &str,
        message: String,
        found: Value,
        expected: Value,
        fix: Option<Value>,
    ) {
        self.total += 1;
        *self.counts.entry(rule).or_default() += 1;
        if self.issues.len() < MAX_ISSUES {
            let mut v = json!({"rule": rule, "severity": severity, "id": id, "name": name, "message": message, "found": found, "expected": expected, "fixable": fix.is_some()});
            if let Some(f) = fix {
                v["fix"] = f;
            }
            self.issues.push(v);
        }
    }
}

fn set_fix(id: NodeId, props: Value) -> Value {
    json!({"command": "node.set", "params": {"ids": [id], "props": props}})
}

fn expected_or_mode(given: Option<String>, values: impl Iterator<Item = String>) -> Option<String> {
    given.or_else(|| mode(values))
}

fn lint(doc: &Document, ids: &[NodeId], p: &Value) -> crate::Result<Value> {
    let cmd = "inspect.lintIcons";
    let positive = |key: &str, default: f64| -> crate::Result<f64> {
        match get(p, key) {
            None | Some(Value::Null) => Ok(default),
            Some(v) => f64_of(v)
                .filter(|n| n.is_finite() && *n > 0.0 && *n <= 4096.0)
                .ok_or_else(|| bad(cmd, format!("`{key}` must be a number above 0 (up to 4096)"))),
        }
    };
    let grid = positive("grid", 24.0)?;
    let tol = positive("tolerance", 0.02)?;
    let pad = match get(p, "padding") {
        None | Some(Value::Null) => grid / 12.0,
        Some(v) => f64_of(v)
            .filter(|n| n.is_finite() && *n >= 0.0 && *n < grid / 2.0)
            .ok_or_else(|| bad(cmd, "`padding` must be a number from 0 to half the grid"))?,
    };
    let given_weight = match get(p, "strokeWeight") {
        None | Some(Value::Null) => None,
        Some(v) => Some(r3(f64_of(v).filter(|n| n.is_finite() && *n >= 0.0).ok_or_else(|| bad(cmd, "`strokeWeight` must be a number"))?).to_string()),
    };
    let text = |key: &str| string(p, key);
    let given_color = match get(p, "color") {
        None | Some(Value::Null) => None,
        Some(v) => Some(color_of(v).ok_or_else(|| bad(cmd, "`color` must be a colour like \"#111111\""))?.to_css_hex().to_ascii_uppercase()),
    };

    let mut icons = Vec::new();
    for &id in ids {
        find_icons(doc, id, grid, 0, &mut icons);
    }
    if icons.is_empty() {
        return Err(bad(cmd, "no icons found: pass the icon-set frame (or an icon) in `ids`"));
    }
    let leaves = || icons.iter().flat_map(|i| i.leaves.iter());
    let weight = expected_or_mode(given_weight, leaves().filter_map(|l| l.weight).map(|w| r3(w).to_string()));
    let cap = expected_or_mode(text("cap"), leaves().filter_map(|l| l.cap.clone()));
    let join = expected_or_mode(text("join"), leaves().filter_map(|l| l.join.clone()));
    let radius = expected_or_mode(None, leaves().filter_map(|l| l.radius).map(|r| r3(r).to_string()));
    let color = expected_or_mode(given_color, leaves().flat_map(|l| l.colors.iter().map(|(_, c)| c.clone())));
    let name_mode = mode(icons.iter().map(|i| name_style(&i.name)).filter(|s| *s != "plain").map(str::to_string));

    let mut r = Report { issues: vec![], total: 0, counts: BTreeMap::new() };
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for i in &icons {
        *seen.entry(i.name.to_lowercase()).or_default() += 1;
    }
    for icon in &icons {
        if icon.boxed && ((icon.size.0 - grid).abs() > tol || (icon.size.1 - grid).abs() > tol) {
            r.add(
                "size",
                "warning",
                icon.id,
                &icon.name,
                format!("the icon is {}x{}, the grid is {grid}x{grid}", r3(icon.size.0), r3(icon.size.1)),
                json!([r3(icon.size.0), r3(icon.size.1)]),
                json!([grid, grid]),
                None,
            );
        }
        if is_default_name(&icon.name) {
            r.add(
                "naming",
                "warning",
                icon.id,
                &icon.name,
                "the icon has a default name".into(),
                json!(icon.name),
                json!("a descriptive name, e.g. \"arrow-left\""),
                None,
            );
        } else if seen.get(&icon.name.to_lowercase()).is_some_and(|n| *n > 1) {
            r.add("naming", "error", icon.id, &icon.name, "another icon has this name".into(), json!(icon.name), json!("a unique name"), None);
        } else if let (Some(m), style) = (&name_mode, name_style(&icon.name))
            && style != "plain"
            && style != m
        {
            r.add("naming", "warning", icon.id, &icon.name, format!("the name is {style}, most icons use {m}"), json!(style), json!(m), None);
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for l in &icon.leaves {
            if let (Some(w), Some(want)) = (l.weight, &weight)
                && r3(w).to_string() != *want
            {
                r.add(
                    "stroke-width",
                    "error",
                    l.id,
                    &l.name,
                    format!("stroke width {} where the set uses {want}", r3(w)),
                    json!(r3(w)),
                    json!(want.parse::<f64>().unwrap_or_default()),
                    want.parse::<f64>().ok().map(|v| set_fix(l.id, json!({"strokeWeight": v}))),
                );
            }
            if let (Some(c), Some(want)) = (&l.cap, &cap)
                && c != want
            {
                let fix = (!c.contains('/')).then(|| set_fix(l.id, json!({"strokeCap": want})));
                r.add("stroke-cap", "error", l.id, &l.name, format!("{c} caps where the set uses {want}"), json!(c), json!(want), fix);
            }
            if let (Some(j), Some(want)) = (&l.join, &join)
                && j != want
            {
                r.add(
                    "stroke-join",
                    "error",
                    l.id,
                    &l.name,
                    format!("{j} joins where the set uses {want}"),
                    json!(j),
                    json!(want),
                    Some(set_fix(l.id, json!({"strokeJoin": want}))),
                );
            }
            if let (Some(rad), Some(want)) = (l.radius, &radius)
                && r3(rad).to_string() != *want
            {
                r.add(
                    "corner-radius",
                    "warning",
                    l.id,
                    &l.name,
                    format!("corner radius {} where the set uses {want}", r3(rad)),
                    json!(r3(rad)),
                    json!(want.parse::<f64>().unwrap_or_default()),
                    want.parse::<f64>().ok().map(|v| set_fix(l.id, json!({"cornerRadius": v}))),
                );
            }
            if let Some(want) = &color {
                for (kind, c) in &l.colors {
                    if c != want {
                        r.add(
                            "color",
                            "warning",
                            l.id,
                            &l.name,
                            format!("{kind} colour {c} where the set uses {want}"),
                            json!(c),
                            json!(want),
                            Some(set_fix(l.id, Value::Object(std::iter::once((kind.to_string(), json!(want))).collect()))),
                        );
                    }
                }
            }
            let Some(b) = l.bounds else { continue };
            let half = l.weight.map_or(0.0, |w| w / 2.0);
            x0 = x0.min(b.x0 - half);
            y0 = y0.min(b.y0 - half);
            x1 = x1.max(b.x1 + half);
            y1 = y1.max(b.y1 + half);
            // A stroke of odd width straddles the edge: its geometry sits on half pixels to stay crisp.
            let odd = l.weight.is_some_and(|w| w > 0.0 && (w.round() as i64) % 2 == 1);
            let offset = if odd { 0.5 } else { 0.0 };
            let snap = |v: f64| (v - offset).round() + offset;
            let off = [b.x0, b.y0, b.x1, b.y1].iter().any(|v| (snap(*v) - v).abs() > tol);
            if off {
                let (dx, dy) = (snap(b.x0) - b.x0, snap(b.y0) - b.y0);
                let shift_only = (snap(b.x1) - b.x1 - dx).abs() <= tol && (snap(b.y1) - b.y1 - dy).abs() <= tol;
                let fix = shift_only.then(|| set_fix(l.id, json!({"x": r3(l.rel.0 + dx), "y": r3(l.rel.1 + dy)})));
                r.add(
                    "pixel-grid",
                    "warning",
                    l.id,
                    &l.name,
                    format!(
                        "edges at x {}..{}, y {}..{} are off the {} pixel grid",
                        r3(b.x0),
                        r3(b.x1),
                        r3(b.y0),
                        r3(b.y1),
                        if odd { "half" } else { "whole" }
                    ),
                    json!({"x0": r3(b.x0), "y0": r3(b.y0), "x1": r3(b.x1), "y1": r3(b.y1)}),
                    json!(if odd { "multiples of 0.5 + 0.5" } else { "whole numbers" }),
                    fix,
                );
            }
        }
        if icon.boxed && x0 <= x1 {
            let m = [x0, y0, grid - x1, grid - y1];
            if m.iter().any(|v| *v < pad - tol) {
                r.add(
                    "padding",
                    "warning",
                    icon.id,
                    &icon.name,
                    format!(
                        "the art (with strokes) comes within {} of an edge; the live area keeps {} free",
                        r3(m.iter().copied().fold(f64::MAX, f64::min)),
                        r3(pad)
                    ),
                    json!({"left": r3(m[0]), "top": r3(m[1]), "right": r3(m[2]), "bottom": r3(m[3])}),
                    json!(format!("at least {} on every side", r3(pad))),
                    None,
                );
            }
        }
    }
    let fixable = r.issues.iter().filter(|i| i["fixable"] == true).count();
    Ok(json!({
        "icons": icons.len(),
        "grid": grid,
        "expected": {
            "strokeWeight": weight.and_then(|w| w.parse::<f64>().ok()),
            "strokeCap": cap, "strokeJoin": join,
            "cornerRadius": radius.and_then(|w| w.parse::<f64>().ok()),
            "color": color, "padding": r3(pad),
        },
        "issues": r.issues,
        "total": r.total,
        "truncated": r.total > MAX_ISSUES,
        "fixable": fixable,
        "counts": r.counts,
    }))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![spec!(
        "inspect.lintIcons",
        "Lint icon set",
        "{ids? (default the selection: an icon-set frame, or icons), grid? (icon size; default 24), strokeWeight? cap? join? color? (what the set should use; default the most common value), padding? (free space at the edges, with strokes; default grid/12), tolerance? (default 0.02)} → {icons, grid, expected, issues: [{rule: size|stroke-width|stroke-cap|stroke-join|corner-radius|color|pixel-grid|padding|naming, severity, id, name, message, found, expected, fixable, fix?: {command, params}}], total, truncated, fixable, counts}  reports only; run a `fix` with run_command/batch",
        false,
        |s, p| {
            let ids = targets(s, p);
            if ids.is_empty() {
                return Err(bad("inspect.lintIcons", "select the icon-set frame or pass `ids`"));
            }
            lint(s.doc()?, &ids, p)
        }
    )]
}
