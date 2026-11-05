//! `node.find`'s structural filters (`page`, `widthNot`/`heightNot`, `outsideParent`, `edgeGap`,
//! `details`) and the parent geometry `inspect.overflow` shares.

use figcraft_doc::{Document, Node, NodeId};
use figcraft_geom::{Rect, transform_rect_bbox};
use serde_json::{Value, json};

use crate::params::{bool_of, f64_of, get, id_of, string};
use crate::{Result, Session, bad};

/// How far (px) a layer may pass its parent's box before it counts as outside.
pub(super) const OUTSIDE_TOLERANCE: f64 = 0.5;
/// Sizes closer than this are the same size.
const SIZE_TOLERANCE: f64 = 0.01;
/// Slack on the inclusive `edgeGap` range, for float noise.
const GAP_SLACK: f64 = 1e-6;

#[derive(Clone, Copy)]
enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

struct EdgeGap {
    edge: Edge,
    min: f64,
    max: f64,
}

/// The structural filters of one `node.find` call, parsed once.
#[derive(Default)]
pub(super) struct Structure {
    width_not: Vec<f64>,
    height_not: Vec<f64>,
    outside_parent: Option<bool>,
    edge_gaps: Vec<EdgeGap>,
}

/// A number, or a list of numbers.
fn numbers(cmd: &str, p: &Value, key: &str) -> Result<Vec<f64>> {
    let one = |v: &Value| f64_of(v).ok_or_else(|| bad(cmd, format!("`{key}` must be a number or a list of numbers")));
    match get(p, key) {
        None => Ok(Vec::new()),
        Some(Value::Array(a)) => a.iter().map(one).collect(),
        Some(v) => Ok(vec![one(v)?]),
    }
}

/// `true` or `false` (the lenient spellings of `params::bool_of`), or none given.
pub(super) fn flag(cmd: &str, p: &Value, key: &str) -> Result<Option<bool>> {
    get(p, key).map(|v| bool_of(v).ok_or_else(|| bad(cmd, format!("`{key}` must be true or false")))).transpose()
}

/// `{edge, min?, max?}`, or a list of them.
fn edge_gaps(cmd: &str, p: &Value) -> Result<Vec<EdgeGap>> {
    let one = |v: &Value| -> Result<EdgeGap> {
        let o = v.as_object().ok_or_else(|| bad(cmd, "`edgeGap` must be {edge, min?, max?} or a list of them"))?;
        let edge = match o.get("edge").and_then(Value::as_str).map(str::to_ascii_lowercase).as_deref() {
            Some("top") => Edge::Top,
            Some("bottom") => Edge::Bottom,
            Some("left") => Edge::Left,
            Some("right") => Edge::Right,
            _ => return Err(bad(cmd, "`edgeGap.edge` must be top, bottom, left or right")),
        };
        let end = |k: &str, default: f64| -> Result<f64> {
            match o.get(k) {
                None | Some(Value::Null) => Ok(default),
                Some(v) => f64_of(v).ok_or_else(|| bad(cmd, format!("`edgeGap.{k}` must be a number"))),
            }
        };
        Ok(EdgeGap { edge, min: end("min", f64::NEG_INFINITY)?, max: end("max", f64::INFINITY)? })
    };
    match get(p, "edgeGap") {
        None => Ok(Vec::new()),
        Some(Value::Array(a)) => a.iter().map(one).collect(),
        Some(v) => Ok(vec![one(v)?]),
    }
}

impl Structure {
    pub(super) fn parse(cmd: &str, p: &Value) -> Result<Self> {
        Ok(Self {
            width_not: numbers(cmd, p, "widthNot")?,
            height_not: numbers(cmd, p, "heightNot")?,
            outside_parent: flag(cmd, p, "outsideParent")?,
            edge_gaps: edge_gaps(cmd, p)?,
        })
    }

    pub(super) fn matches(&self, doc: &Document, n: &Node) -> bool {
        let differs = |not: &[f64], v: f64| not.iter().all(|x| (v - x).abs() > SIZE_TOLERANCE);
        if !differs(&self.width_not, n.width) || !differs(&self.height_not, n.height) {
            return false;
        }
        if self.outside_parent.is_none() && self.edge_gaps.is_empty() {
            return true;
        }
        // A layer straight on the page has no parent box: it is inside nothing and has no gaps.
        let Some(parent) = space_parent(doc, n) else { return self.outside_parent == Some(false) && self.edge_gaps.is_empty() };
        let b = bounds_in_parent(doc, n);
        let outside = reaches(b, parent.width, parent.height).iter().any(|(_, v)| *v > OUTSIDE_TOLERANCE);
        if self.outside_parent.is_some_and(|want| want != outside) {
            return false;
        }
        self.edge_gaps.iter().all(|g| {
            let gap = match g.edge {
                Edge::Top => b.y0,
                Edge::Left => b.x0,
                Edge::Bottom => parent.height - b.y1,
                Edge::Right => parent.width - b.x1,
            };
            gap >= g.min - GAP_SLACK && gap <= g.max + GAP_SLACK
        })
    }
}

/// The frame, section or component whose box `n` sits in: the nearest ancestor with a coordinate
/// space of its own (groups and boolean groups don't count, as for x and y). `None` on the page.
pub(super) fn space_parent<'a>(doc: &'a Document, n: &Node) -> Option<&'a Node> {
    let mut cur = n.parent;
    for _ in 0..10_000 {
        let p = doc.node(cur?)?;
        if p.is_page() {
            return None;
        }
        if !p.kind.bounds_follow_children() {
            return Some(p);
        }
        cur = p.parent;
    }
    None
}

/// The page-aligned box of `n` in its [`space_parent`]'s coordinates.
pub(super) fn bounds_in_parent(doc: &Document, n: &Node) -> Rect {
    transform_rect_bbox(doc.frame_relative(n.id), n.local_rect())
}

/// How far `r` reaches past the box `(0, 0, w, h)` on each side (zero or less: inside).
pub(super) fn reaches(r: Rect, w: f64, h: f64) -> [(&'static str, f64); 4] {
    [("left", -r.x0), ("top", -r.y0), ("right", r.x1 - w), ("bottom", r.y1 - h)]
}

/// The pages a command searches: `page` (an id, a name or `all`), else the open page.
pub(super) fn pages(s: &Session, cmd: &str, p: &Value) -> Result<Vec<NodeId>> {
    let Some(v) = get(p, "page") else { return Ok(vec![s.page()?]) };
    let doc = s.doc()?;
    if v.as_str().is_some_and(|t| t.eq_ignore_ascii_case("all")) {
        return Ok(doc.pages.clone());
    }
    if let Some(id) = id_of(v).filter(|i| doc.pages.contains(i)) {
        return Ok(vec![id]);
    }
    let want = string(p, "page").unwrap_or_default();
    let named = |exact: bool| {
        doc.pages.iter().copied().find(|i| doc.node(*i).is_some_and(|n| if exact { n.name == want } else { n.name.eq_ignore_ascii_case(&want) }))
    };
    named(true)
        .or_else(|| named(false))
        .map(|i| vec![i])
        .ok_or_else(|| bad(cmd, format!("`page` must be a page id, a page name or \"all\" (no page matches {want})")))
}

/// What `details` returns for a hit: x and y are relative to the parent frame.
pub(super) fn detail(doc: &Document, n: &Node) -> Value {
    let c = doc.frame_relative(n.id).as_coeffs();
    let r = |v: f64| if v.is_finite() { (v * 100.0).round() / 100.0 } else { 0.0 };
    json!({"id": n.id, "name": n.name, "type": n.kind.type_name(), "parent": n.parent, "x": r(c[4]), "y": r(c[5]), "width": r(n.width), "height": r(n.height)})
}
