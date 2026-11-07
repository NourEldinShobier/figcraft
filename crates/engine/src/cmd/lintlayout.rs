//! `lint.layout`: layout defects among sibling layers that an agent can't see without a
//! screenshot: overlaps, edges a few pixels off a shared line, uneven gaps in a row or column,
//! tap targets that are too small, and positions off the pixel grid. Each finding can carry a
//! `fix`: a ready command for `run_command`.

use std::collections::BTreeSet;

use figcraft_doc::{Document, Node, NodeId, NodeKind};
use figcraft_geom::{Rect, transform_rect_bbox};
use serde_json::{Value, json};

use super::CommandSpec;
use crate::params::{f64_of, get, opt_id};
use crate::{Result, Session, bad, spec};

const CHECKS: [&str; 5] = ["overlap", "misaligned", "uneven-gap", "tiny-target", "off-grid"];
/// Run when `checks` is left out: off-grid is opt-in because it flags most free-form designs.
const DEFAULT_CHECKS: [&str; 4] = ["overlap", "misaligned", "uneven-gap", "tiny-target"];
const LISTED: usize = 100;
/// Siblings compared pairwise per container; larger containers are skipped.
const MAX_SIBLINGS: usize = 300;
/// An edge this close to a shared line but not on it counts as misaligned.
const NEAR: f64 = 4.0;
const EPS: f64 = 0.01;
/// Frames at most this wide are phones: tap targets there need 44, elsewhere 24.
const MOBILE_WIDTH: f64 = 500.0;
const HINTS: [&str; 10] = ["button", "btn", "link", "tab", "toggle", "switch", "checkbox", "radio", "chip", "cta"];

pub fn specs() -> Vec<CommandSpec> {
    vec![spec!(
        "lint.layout",
        "Find layout defects",
        "{root? (a layer: only its subtree), page? (id, name or \"all\"; default the open page), checks? (any of overlap, misaligned, uneven-gap, tiny-target, off-grid; default all but off-grid), grid? (off-grid step, default 4), limit? (findings listed, default 100)}  looks at the visible layers of each non-auto-layout container → {total, listed, truncated?, findings: [{check, ids, message, fix?: {command, params}}]}. overlap: siblings that intersect with neither inside the other; misaligned: edges or centres 0-4 px from a shared line (fix: align.*); uneven-gap: a row or column of 3+ whose gaps differ by up to 4 px (fix: distribute.*); tiny-target: frames, components and instances with a button-like name, or any layer with interactions, under 24x24 (44x44 in frames up to 500 wide) (fix: node.set size); off-grid: x/y not a multiple of `grid` (fix: node.set x/y)",
        false,
        lint_layout
    )]
}

fn lint_layout(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "lint.layout";
    let limit = match get(p, "limit") {
        None => LISTED,
        Some(v) => v.as_u64().filter(|n| *n <= 5000).ok_or_else(|| bad(CMD, "`limit` must be an integer from 0 to 5000"))? as usize,
    };
    let grid = match get(p, "grid") {
        None => 4.0,
        Some(v) => f64_of(v).filter(|g| (1.0..=1000.0).contains(g)).ok_or_else(|| bad(CMD, "`grid` must be a number from 1 to 1000"))?,
    };
    let checks: BTreeSet<&str> = match get(p, "checks") {
        None => DEFAULT_CHECKS.into_iter().collect(),
        Some(v) => {
            let list = v.as_array().ok_or_else(|| bad(CMD, "`checks` must be a list of check names"))?;
            let mut set = BTreeSet::new();
            for c in list {
                let name = c
                    .as_str()
                    .and_then(|n| CHECKS.iter().find(|k| **k == n))
                    .ok_or_else(|| bad(CMD, format!("unknown check {c}; use {}", CHECKS.join(", "))))?;
                set.insert(*name);
            }
            set
        }
    };
    let roots = match opt_id(CMD, p, "root")? {
        Some(r) => vec![r],
        None => super::edit::structure_find::pages(s, CMD, p)?,
    };
    let doc = s.doc()?;
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    let mut stack: Vec<NodeId> = roots.iter().rev().copied().collect();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(n) = doc.node(id) else { continue };
        if !n.visible {
            continue;
        }
        let instance = matches!(n.kind, NodeKind::Instance { .. });
        if !instance {
            stack.extend(n.children.iter().rev().copied());
        }
        if checks.contains("tiny-target") {
            tiny_target(doc, n, &mut out);
        }
        if n.kind.is_container() && !instance {
            container(doc, n, &checks, grid, &mut out);
        }
    }
    let total = out.len();
    out.truncate(limit);
    let mut r = json!({"total": total, "listed": out.len(), "findings": out});
    if total > limit {
        r["truncated"] = json!(true);
    }
    Ok(r)
}

fn finding(check: &str, ids: &[NodeId], message: String, fix: Option<Value>) -> Value {
    let mut f = json!({"check": check, "ids": ids, "message": message});
    if let Some(fix) = fix {
        f["fix"] = fix;
    }
    f
}

fn label(n: &Node) -> String {
    format!("\"{}\" ({})", n.name, n.id.0)
}

fn round(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn tiny_target(doc: &Document, n: &Node, out: &mut Vec<Value>) {
    let named = matches!(n.kind, NodeKind::Instance { .. } | NodeKind::Component { .. } | NodeKind::Frame) && {
        let l = n.name.to_lowercase();
        HINTS.iter().any(|h| l.contains(h))
    };
    if n.is_page() || !(named || !n.interactions.is_empty()) {
        return;
    }
    // The top-level frame decides whether this is a phone layout.
    let mut top = n;
    for _ in 0..10_000 {
        match top.parent.and_then(|p| doc.node(p)) {
            Some(p) if !p.is_page() => top = p,
            _ => break,
        }
    }
    let min = if top.width <= MOBILE_WIDTH { 44.0 } else { 24.0 };
    if n.width < min - EPS || n.height < min - EPS {
        let (w, h) = (n.width.max(min), n.height.max(min));
        out.push(finding(
            "tiny-target",
            &[n.id],
            format!("{} is {}x{}, under the {min}x{min} minimum for a tap target", label(n), round(n.width), round(n.height)),
            Some(json!({"command": "node.set", "params": {"ids": [n.id], "props": {"width": w, "height": h}}})),
        ));
    }
}

struct Item<'a> {
    node: &'a Node,
    r: Rect,
}

fn container(doc: &Document, parent: &Node, checks: &BTreeSet<&str>, grid: f64, out: &mut Vec<Value>) {
    // Auto layout places its children itself; absolute ones are decoration.
    if parent.layout.is_some() {
        return;
    }
    let items: Vec<Item> = parent
        .children
        .iter()
        .filter_map(|c| doc.node(*c))
        .filter(|c| c.visible && !c.child_layout.absolute && !c.is_mask)
        .filter_map(|c| doc.world_bounds(c.id).map(|r| Item { node: c, r }))
        .filter(|i| i.r.width() > EPS && i.r.height() > EPS)
        .collect();
    if items.len() > MAX_SIBLINGS {
        return;
    }
    if checks.contains("off-grid") && !parent.is_page() {
        off_grid(doc, &items, grid, out);
    }
    if items.len() < 2 {
        return;
    }
    if checks.contains("overlap") {
        overlap(&items, out);
    }
    if checks.contains("misaligned") {
        misaligned(&items, out);
    }
    if checks.contains("uneven-gap") {
        uneven_gaps(&items, true, out);
        uneven_gaps(&items, false, out);
    }
}

fn contains(a: Rect, b: Rect) -> bool {
    a.x0 <= b.x0 + EPS && a.y0 <= b.y0 + EPS && a.x1 >= b.x1 - EPS && a.y1 >= b.y1 - EPS
}

fn overlap(items: &[Item], out: &mut Vec<Value>) {
    for (i, a) in items.iter().enumerate() {
        for b in &items[i + 1..] {
            let (w, h) = (a.r.x1.min(b.r.x1) - a.r.x0.max(b.r.x0), a.r.y1.min(b.r.y1) - a.r.y0.max(b.r.y0));
            if w > 0.5 && h > 0.5 && !contains(a.r, b.r) && !contains(b.r, a.r) {
                out.push(finding(
                    "overlap",
                    &[a.node.id, b.node.id],
                    format!("{} and {} overlap by {}x{}", label(a.node), label(b.node), round(w), round(h)),
                    None,
                ));
            }
        }
    }
}

/// Edge kinds per axis: name, value, align command.
fn edges(r: Rect) -> [[(&'static str, f64, &'static str); 3]; 2] {
    [
        [("left", r.x0, "align.left"), ("centre", (r.x0 + r.x1) / 2.0, "align.hcenter"), ("right", r.x1, "align.right")],
        [("top", r.y0, "align.top"), ("middle", (r.y0 + r.y1) / 2.0, "align.vcenter"), ("bottom", r.y1, "align.bottom")],
    ]
}

fn misaligned(items: &[Item], out: &mut Vec<Value>) {
    for axis in 0..2 {
        for kind in 0..3 {
            let mut vals: Vec<(f64, usize)> = items.iter().enumerate().map(|(i, it)| (edges(it.r)[axis][kind].1, i)).collect();
            vals.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut start = 0;
            while start < vals.len() {
                let mut end = start + 1;
                while end < vals.len() && vals[end].0 - vals[start].0 <= NEAR {
                    end += 1;
                }
                let group = &vals[start..end];
                start = end;
                let (Some(first), Some(last)) = (group.first(), group.last()) else { continue };
                let spread = last.0 - first.0;
                if group.len() < 2 || spread <= EPS {
                    continue;
                }
                // Exactly aligned on another edge of the same axis: not a mistake.
                let exact = (0..3).filter(|k| *k != kind).any(|k| {
                    let v: Vec<f64> = group.iter().map(|(_, i)| edges(items[*i].r)[axis][k].1).collect();
                    v.iter().all(|x| (x - v[0]).abs() <= EPS)
                });
                if exact {
                    continue;
                }
                let ids: Vec<NodeId> = group.iter().map(|(_, i)| items[*i].node.id).collect();
                let (name, _, cmd) = edges(items[first.1].r)[axis][kind];
                out.push(finding(
                    "misaligned",
                    &ids,
                    format!("{} {name} edges lie within {} px of one line but are not on it", ids.len(), round(spread)),
                    Some(json!({"command": cmd, "params": {"ids": ids}})),
                ));
            }
        }
    }
}

fn uneven_gaps(items: &[Item], horizontal: bool, out: &mut Vec<Value>) {
    let lo = |r: Rect| if horizontal { r.x0 } else { r.y0 };
    let hi = |r: Rect| if horizontal { r.x1 } else { r.y1 };
    let cross = |a: Rect, b: Rect| if horizontal { a.y1.min(b.y1) - a.y0.max(b.y0) } else { a.x1.min(b.x1) - a.x0.max(b.x0) };
    let mut order: Vec<&Item> = items.iter().collect();
    order.sort_by(|a, b| lo(a.r).total_cmp(&lo(b.r)));
    let mut run: Vec<&Item> = Vec::new();
    let mut gaps: Vec<f64> = Vec::new();
    for it in order.into_iter().map(Some).chain([None]) {
        if let (Some(prev), Some(next)) = (run.last(), it)
            && cross(prev.r, next.r) > 0.0
            && lo(next.r) - hi(prev.r) >= 0.0
        {
            gaps.push(lo(next.r) - hi(prev.r));
            run.push(next);
            continue;
        }
        if run.len() >= 3 {
            let (min, max) = gaps.iter().fold((f64::MAX, f64::MIN), |(a, b), g| (a.min(*g), b.max(*g)));
            if max - min > EPS && max - min <= NEAR {
                let ids: Vec<NodeId> = run.iter().map(|i| i.node.id).collect();
                let axis = if horizontal { "row" } else { "column" };
                let list = gaps.iter().map(|g| round(*g).to_string()).collect::<Vec<_>>().join(", ");
                out.push(finding(
                    "uneven-gap",
                    &ids,
                    format!("gaps in this {axis} of {} layers are {list} px", ids.len()),
                    Some(json!({"command": if horizontal { "distribute.horizontal" } else { "distribute.vertical" }, "params": {"ids": ids}})),
                ));
            }
        }
        run = it.into_iter().collect();
        gaps.clear();
    }
}

fn off_grid(doc: &Document, items: &[Item], grid: f64, out: &mut Vec<Value>) {
    for it in items {
        let n = it.node;
        let c = n.transform.as_coeffs();
        if c[1].abs() > EPS || c[2].abs() > EPS {
            continue; // rotated or skewed: x/y are not the box edge
        }
        let r = transform_rect_bbox(doc.frame_relative(n.id), n.local_rect());
        let snap = |v: f64| (v / grid).round() * grid;
        let off = |v: f64| (v - snap(v)).abs() > EPS;
        if off(r.x0) || off(r.y0) {
            out.push(finding(
                "off-grid",
                &[n.id],
                format!("{} sits at ({}, {}), off the {grid} px grid", label(n), round(r.x0), round(r.y0)),
                Some(json!({"command": "node.set", "params": {"ids": [n.id], "props": {"x": snap(r.x0), "y": snap(r.y0)}}})),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use figcraft_doc::{NodeId, NodeKind};
    use serde_json::{Value, json};

    use crate::Session;

    fn rect(s: &mut Session, parent: NodeId, name: &str, x: f64, y: f64, w: f64, h: f64) -> NodeId {
        s.doc_mut().unwrap().create(parent, NodeKind::Rectangle, name, x, y, w, h).unwrap()
    }

    fn frame(s: &mut Session, w: f64) -> NodeId {
        let page = s.page().unwrap();
        s.doc_mut().unwrap().create(page, NodeKind::Frame, "F", 0.0, 0.0, w, 800.0).unwrap()
    }

    fn lint(s: &mut Session, check: &str) -> Value {
        s.execute("lint.layout", &json!({"checks": [check]})).unwrap()
    }

    #[test]
    fn overlapping_siblings_are_found_but_nesting_is_not() {
        let mut s = Session::new();
        let f = frame(&mut s, 1000.0);
        let a = rect(&mut s, f, "A", 0.0, 0.0, 100.0, 100.0);
        let b = rect(&mut s, f, "B", 60.0, 60.0, 100.0, 100.0);
        rect(&mut s, f, "Inner", 10.0, 10.0, 20.0, 20.0);
        let r = lint(&mut s, "overlap");
        assert_eq!(r["total"], json!(1), "{r}");
        assert_eq!(r["findings"][0]["ids"], json!([a, b]), "{r}");
    }

    #[test]
    fn auto_layout_children_are_ignored() {
        let mut s = Session::new();
        let f = frame(&mut s, 1000.0);
        rect(&mut s, f, "A", 0.0, 0.0, 100.0, 100.0);
        rect(&mut s, f, "B", 60.0, 60.0, 100.0, 100.0);
        assert_eq!(lint(&mut s, "overlap")["total"], json!(1));
        s.execute("layout.add", &json!({"ids": [f]})).unwrap();
        assert_eq!(lint(&mut s, "overlap")["total"], json!(0));
    }

    #[test]
    fn near_aligned_edges_get_an_align_fix_and_exact_ones_do_not() {
        let mut s = Session::new();
        let f = frame(&mut s, 1000.0);
        let a = rect(&mut s, f, "A", 100.0, 0.0, 50.0, 40.0);
        let b = rect(&mut s, f, "B", 102.0, 200.0, 70.0, 40.0);
        let r = lint(&mut s, "misaligned");
        let left: Vec<&Value> = r["findings"].as_array().unwrap().iter().filter(|x| x["fix"]["command"] == "align.left").collect();
        assert_eq!(left.len(), 1, "{r}");
        assert_eq!(left[0]["ids"], json!([a, b]), "{r}");
        s.execute("align.left", &left[0]["fix"]["params"]).unwrap();
        let after = lint(&mut s, "misaligned");
        assert!(after["findings"].as_array().unwrap().iter().all(|x| x["fix"]["command"] != "align.left"), "{after}");
    }

    #[test]
    fn uneven_gaps_in_a_row_are_found_and_distribute_fixes_them() {
        let mut s = Session::new();
        let f = frame(&mut s, 1000.0);
        for (i, x) in [0.0, 60.0, 122.0, 180.0].iter().enumerate() {
            rect(&mut s, f, &format!("c{i}"), *x, 0.0, 50.0, 50.0);
        }
        let r = lint(&mut s, "uneven-gap");
        assert_eq!(r["total"], json!(1), "{r}");
        s.execute("distribute.horizontal", &r["findings"][0]["fix"]["params"]).unwrap();
        assert_eq!(lint(&mut s, "uneven-gap")["total"], json!(0));
    }

    #[test]
    fn tiny_targets_depend_on_the_frame_width() {
        let mut s = Session::new();
        let phone = frame(&mut s, 390.0);
        let desktop = frame(&mut s, 1200.0);
        let a = rect(&mut s, phone, "Save button", 0.0, 0.0, 30.0, 30.0);
        let b = rect(&mut s, desktop, "Save button", 0.0, 0.0, 30.0, 30.0);
        assert_eq!(lint(&mut s, "tiny-target")["total"], json!(0), "plain rectangles are not interactive");
        for id in [a, b] {
            s.doc_mut().unwrap().node_mut(id).unwrap().kind = NodeKind::Frame;
        }
        let r = lint(&mut s, "tiny-target");
        assert_eq!(r["total"], json!(1), "{r}");
        assert_eq!(r["findings"][0]["ids"], json!([a]), "{r}");
        assert_eq!(r["findings"][0]["fix"]["params"]["props"], json!({"width": 44.0, "height": 44.0}));
    }

    #[test]
    fn off_grid_is_opt_in_and_the_fix_snaps() {
        let mut s = Session::new();
        let f = frame(&mut s, 1000.0);
        let a = rect(&mut s, f, "A", 13.0, 8.0, 50.0, 50.0);
        assert_eq!(s.execute("lint.layout", &json!({})).unwrap()["total"], json!(0));
        let r = lint(&mut s, "off-grid");
        assert_eq!(r["findings"][0]["ids"], json!([a]), "{r}");
        assert_eq!(r["findings"][0]["fix"]["params"]["props"], json!({"x": 12.0, "y": 8.0}));
    }

    #[test]
    fn limit_truncates_and_junk_parameters_are_errors() {
        let mut s = Session::new();
        let f = frame(&mut s, 1000.0);
        for i in 0..4 {
            rect(&mut s, f, "o", i as f64 * 10.0, 0.0, 100.0, 100.0);
        }
        let r = s.execute("lint.layout", &json!({"checks": ["overlap"], "limit": 2})).unwrap();
        assert_eq!((r["total"].clone(), r["listed"].clone(), r["truncated"].clone()), (json!(6), json!(2), json!(true)), "{r}");
        for junk in [
            json!({"checks": ["nope"]}),
            json!({"checks": "overlap"}),
            json!({"limit": -1}),
            json!({"grid": 0}),
            json!({"root": "x"}),
            json!({"page": 3}),
        ] {
            assert!(s.execute("lint.layout", &junk).is_err(), "{junk}");
        }
    }
}
