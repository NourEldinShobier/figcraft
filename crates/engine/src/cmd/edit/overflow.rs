//! `inspect.overflow`: visible layers that stick out of their parent, or whose text doesn't fit.

use figcraft_doc::{Document, Node, NodeKind, TextAutoResize};
use figcraft_geom::Rect;
use serde_json::{Value, json};

use super::structure_find::{OUTSIDE_TOLERANCE, bounds_in_parent, pages, reaches, space_parent};
use crate::params::{get, opt_id};
use crate::{Result, Session, bad};

/// `v` px, to one decimal.
fn px(v: f64) -> String {
    ((v * 10.0).round() / 10.0).to_string()
}

/// What the layer has wrong: (kind, detail), the box first, then the text.
fn problems(doc: &Document, n: &Node, frame: Option<&Node>) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    if let Some(f) = frame {
        let clips = f.clip && f.kind.is_frame_like();
        let past = |r: Rect| {
            let sides: Vec<String> = reaches(r, f.width, f.height)
                .iter()
                .filter(|(_, v)| *v > OUTSIDE_TOLERANCE)
                .map(|(side, v)| format!("{} past {side}", px(*v)))
                .collect();
            sides.join(", ")
        };
        let boxed = past(bounds_in_parent(doc, n));
        if !boxed.is_empty() {
            // A group is what its children are, and they are listed themselves.
            if !matches!(n.kind, NodeKind::Group) {
                out.push((if clips { "outside-parent-clipped" } else { "outside-parent" }, boxed));
            }
        } else if clips {
            // The box fits; what is painted around it (shadows, blurs, outside strokes) may not.
            let painted = past(n.own_paint_bounds(doc.frame_relative(n.id)));
            if !painted.is_empty() {
                out.push(("effect-clipped", painted));
            }
        }
    }
    if let NodeKind::Text(t) = &n.kind
        && t.path.is_none()
        && matches!(t.auto_resize, TextAutoResize::None)
        && let Some((layout, _)) = figcraft_text::layout_node(n)
    {
        let mut parts = Vec::new();
        if layout.height.is_finite() && layout.height - n.height > OUTSIDE_TOLERANCE {
            parts.push(format!("text is {} tall, the box {}", px(layout.height), px(n.height)));
        }
        if layout.width.is_finite() && layout.width - n.width > OUTSIDE_TOLERANCE {
            parts.push(format!("text is {} wide, the box {}", px(layout.width), px(n.width)));
        }
        if !parts.is_empty() {
            out.push(("text-overflow", parts.join("; ")));
        }
    }
    out
}

pub(super) fn overflow(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "inspect.overflow";
    let limit = match get(p, "limit") {
        None => 200,
        Some(v) => v.as_u64().filter(|n| *n <= 10000).ok_or_else(|| bad(cmd, "`limit` must be an integer from 0 to 10000"))? as usize,
    };
    let roots = match opt_id(cmd, p, "root")? {
        Some(root) => vec![root],
        None => pages(s, cmd, p)?,
    };
    let doc = s.doc()?;
    let mut items = Vec::new();
    let mut total = 0usize;
    for root in roots {
        let mut stack = vec![doc.get(root)?.children.iter()];
        while let Some(children) = stack.last_mut() {
            let Some(&id) = children.next() else {
                stack.pop();
                continue;
            };
            let n = doc.get(id)?;
            // A hidden layer paints nothing, and neither do its children (nor a section's hidden contents).
            if !n.visible {
                continue;
            }
            if !n.hides_contents() {
                stack.push(n.children.iter());
            }
            let frame = space_parent(doc, n);
            let parent = frame.map(|f| f.id).or(n.parent);
            for (kind, detail) in problems(doc, n, frame) {
                total += 1;
                if items.len() < limit {
                    let parent_name = parent.and_then(|i| doc.node(i)).map(|q| q.name.as_str()).unwrap_or_default();
                    items.push(json!({"id": id, "name": n.name, "parent": parent, "parentName": parent_name, "kind": kind, "detail": detail}));
                }
            }
        }
    }
    Ok(json!({"items": items, "total": total, "truncated": total > items.len()}))
}
