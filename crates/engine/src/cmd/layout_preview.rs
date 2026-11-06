//! `layout.previewWidths`: how an auto layout frame lays out at other widths, without touching the
//! design. Each width is laid out on a clone of the document (resized with the code `node.set` uses,
//! then settled by [`crate::normalize`]), read, optionally rendered, and dropped: the live document,
//! selection, history and tabs are never written to.

use base64::Engine as _;
use figcraft_doc::{Document, ExportFormat, ExportSize, LayoutMode, Node, NodeId};
use figcraft_geom::Rect;
use serde_json::{Map, Value, json};

use super::CommandSpec;
use crate::params::{bool_of, f64_of, get, req_id};
use crate::{EngineError, Result, Session, bad, spec};

const CMD: &str = "layout.previewWidths";
/// Most widths one call previews.
const MAX_WIDTHS: usize = 12;
/// Largest width or height asked for.
const MAX_SIZE: f64 = 20_000.0;
/// Most children listed for each preview.
const MAX_CHILDREN: usize = 200;
/// How far (px) a box may pass the frame before it counts as overflowing, and the cross-axis
/// overlap still counted as one wrapped line.
const TOLERANCE: f64 = 0.5;
const DEFAULT_SCALE: f64 = 0.25;

pub fn specs() -> Vec<CommandSpec> {
    vec![spec!(
        "layout.previewWidths",
        "Preview frame at other widths",
        "{id (an auto layout frame), widths ([numbers], 1 to 12 values, each 1 to 20000), heights? ([numbers], as many as widths, each 1 to 20000; default the frame's own height setting: a hugging height follows the content), render? (bool, default false), scale? (render size, default 0.25)}  → {id, previews: [{width, height (as laid out; min/max sizes apply), rows? (wrapping frames: number of rows, or columns for a vertical one), children: [{id, x, y, width, height}] (the first 200 visible layers, relative to the frame), childCount, overflowing: [ids of visible children extending past the frame], format?, data? (with render: PNG, base64)}]}  read-only: lays the frame out on a copy of the document, so the design, selection and undo history stay as they are",
        false,
        preview
    )]
}

/// `key` as 1 to [`MAX_WIDTHS`] numbers from 1 to [`MAX_SIZE`]; `None` when it isn't given and not `required`.
fn sizes(p: &Value, key: &str, required: bool) -> Result<Option<Vec<f64>>> {
    let shape = || bad(CMD, format!("`{key}` must be an array of 1 to {MAX_WIDTHS} numbers, each from 1 to {MAX_SIZE}"));
    let Some(v) = get(p, key) else { return if required { Err(shape()) } else { Ok(None) } };
    let list = v.as_array().filter(|a| (1..=MAX_WIDTHS).contains(&a.len())).ok_or_else(shape)?;
    let mut out = Vec::with_capacity(list.len());
    for item in list {
        let n = if item.is_boolean() { None } else { f64_of(item) };
        out.push(n.filter(|n| (1.0..=MAX_SIZE).contains(n)).ok_or_else(shape)?);
    }
    Ok(Some(out))
}

/// A copy of `doc` with frame `id` resized to `w` (and `h`) and everything that follows settled, as
/// `node.set` leaves it.
fn laid_out(doc: &Document, id: NodeId, w: f64, h: Option<f64>) -> Result<Document> {
    let mut props = Map::new();
    props.insert("width".into(), json!(w));
    if let Some(h) = h {
        props.insert("height".into(), json!(h));
    }
    let mut copy = doc.clone();
    super::props::apply(CMD, &mut copy, id, &props)?;
    crate::normalize(&mut copy, doc);
    Ok(copy)
}

/// A layer's box in its frame (rotation turns it into its bounding box).
fn bounds(doc: &Document, n: &Node) -> Rect {
    figcraft_geom::transform_rect_bbox(doc.frame_relative(n.id), n.local_rect())
}

/// How many wrapped lines the frame's flow children sit in: a child starts a new line when it begins
/// at or past the end of the line before it across the flow.
fn lines(doc: &Document, frame: NodeId, horizontal: bool) -> usize {
    let (mut count, mut end) = (0usize, f64::NEG_INFINITY);
    for n in figcraft_layout::flow_children(doc, frame).into_iter().filter_map(|c| doc.node(c)) {
        let b = bounds(doc, n);
        let (start, stop) = if horizontal { (b.y0, b.y1) } else { (b.x0, b.x1) };
        if count == 0 || start >= end - TOLERANCE {
            count += 1;
            end = stop;
        } else {
            end = end.max(stop);
        }
    }
    count
}

/// What frame `id` looks like in `doc`.
fn describe(doc: &Document, id: NodeId) -> Result<Map<String, Value>> {
    let f = doc.get(id)?;
    let (mut children, mut overflowing, mut count) = (Vec::new(), Vec::new(), 0usize);
    for n in f.children.iter().filter_map(|c| doc.node(*c)).filter(|n| n.visible) {
        let b = bounds(doc, n);
        if b.x0 < -TOLERANCE || b.y0 < -TOLERANCE || b.x1 > f.width + TOLERANCE || b.y1 > f.height + TOLERANCE {
            overflowing.push(n.id);
        }
        if children.len() < MAX_CHILDREN {
            let at = doc.frame_relative(n.id).as_coeffs();
            let r = |v: f64| figcraft_geom::round_to(v, 3);
            children.push(json!({"id": n.id, "x": r(at[4]), "y": r(at[5]), "width": r(n.width), "height": r(n.height)}));
        }
        count += 1;
    }
    let mut out = Map::new();
    out.insert("width".into(), json!(figcraft_geom::round_to(f.width, 3)));
    out.insert("height".into(), json!(figcraft_geom::round_to(f.height, 3)));
    if let Some(l) = f.layout.as_ref().filter(|l| l.wrap && l.mode != LayoutMode::Grid) {
        out.insert("rows".into(), json!(lines(doc, id, l.mode == LayoutMode::Horizontal)));
    }
    out.insert("children".into(), Value::Array(children));
    out.insert("childCount".into(), json!(count));
    out.insert("overflowing".into(), json!(overflowing));
    Ok(out)
}

fn preview(s: &mut Session, p: &Value) -> Result<Value> {
    let id = req_id(CMD, p, "id")?;
    let widths = sizes(p, "widths", true)?.unwrap_or_default();
    let heights = sizes(p, "heights", false)?;
    if heights.as_ref().is_some_and(|h| h.len() != widths.len()) {
        return Err(bad(CMD, "`heights` must have as many values as `widths`"));
    }
    let render = match get(p, "render") {
        None => false,
        Some(v) => bool_of(v).ok_or_else(|| bad(CMD, "`render` must be a boolean (true or false)"))?,
    };
    let scale = match get(p, "scale") {
        None => DEFAULT_SCALE,
        Some(v) => f64_of(v).filter(|n| *n > 0.0).ok_or_else(|| bad(CMD, "`scale` must be a number above 0"))?.clamp(0.01, ExportSize::MAX_SCALE),
    };
    let doc = s.doc()?;
    let frame = doc.get(id)?;
    if !frame.kind.is_frame_like() || frame.layout.is_none() {
        return Err(EngineError::Disabled(CMD.into(), format!("layer {id} is not a frame with auto layout")));
    }
    let mut previews = Vec::with_capacity(widths.len());
    for (i, w) in widths.iter().enumerate() {
        let copy = laid_out(doc, id, *w, heights.as_ref().and_then(|h| h.get(i)).copied())?;
        let mut entry = describe(&copy, id)?;
        if render {
            let png = super::file::export_bytes(&copy, &[id], ExportFormat::Png, scale)?;
            entry.insert("format".into(), json!("png"));
            entry.insert("data".into(), json!(base64::engine::general_purpose::STANDARD.encode(&png)));
        }
        previews.push(Value::Object(entry));
    }
    Ok(json!({"id": id, "previews": previews}))
}
