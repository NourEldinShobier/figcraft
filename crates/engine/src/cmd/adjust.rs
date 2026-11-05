//! `node.adjust`: relative edits in bulk. Each number is added to the layer's own current value
//! (layers of one call have different values), then applied through `node.set`, so constraints,
//! auto layout, instance rules and undo behave exactly as they do for a `node.set` width, x, gap…

use std::collections::BTreeSet;

use figcraft_doc::{Document, Node, NodeKind};
use figcraft_geom::Affine;
use serde_json::{Map, Value, json};

use super::CommandSpec;
use crate::params::{get, req_targets};
use crate::{EngineError, Result, Session, bad, find_command, spec};

/// The adjustments, in the order they are read.
const KEYS: [&str; 10] = ["dx", "dy", "dw", "dh", "fontSize", "cornerRadius", "gap", "padding", "opacity", "rotation"];

pub fn specs() -> Vec<CommandSpec> {
    vec![spec!(
        "node.adjust",
        "Adjust layers",
        "{ids?, dx?, dy?, dw?, dh?, fontSize?, cornerRadius?, gap?, padding?, opacity?, rotation?}  adds each number to every layer's own current value, as one undo step: dx and dy move it in its parent frame like `node.set` x and y (not in an auto layout, which positions its children); dw and dh resize it like `node.set` width and height (constraints and auto layout apply); fontSize (text layers with one size), cornerRadius (frames and rectangles, every corner), gap and padding (auto layout frames; padding on all four sides), opacity (0-1; a number above 1 is a percentage), rotation (degrees). A layer a property does not apply to keeps it and is listed in `skipped` → {changed: [ids], skipped: [{id, why}], warnings?}",
        true,
        adjust
    )]
}

fn adjust(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "node.adjust";
    let mut deltas = Vec::new();
    for key in KEYS {
        if let Some(v) = get(p, key) {
            let d = v.as_f64().filter(|d| d.is_finite()).ok_or_else(|| bad(CMD, format!("`{key}` must be a number")))?;
            deltas.push((key, d));
        }
    }
    if deltas.is_empty() {
        return Err(bad(CMD, format!("give at least one of {}", KEYS.join(", "))));
    }
    let ids = req_targets(CMD, s, p)?;
    let set = find_command("node.set").ok_or_else(|| EngineError::UnknownCommand("node.set".into()))?;
    let (mut changed, mut skipped, mut warnings) = (Vec::new(), Vec::new(), Vec::new());
    let mut seen = BTreeSet::new();
    // One layer at a time, each read just before it is set: resizing a frame can move or resize
    // layers in it, which then take their own change from where they ended up.
    for id in ids.into_iter().filter(|i| seen.insert(*i)) {
        let (props, why) = {
            let doc = s.doc()?;
            plan(doc, doc.get(id)?, &deltas)
        };
        if !why.is_empty() {
            skipped.push(json!({"id": id, "why": why.join("; ")}));
        }
        if props.is_empty() {
            continue;
        }
        let r = (set.run)(s, &json!({"ids": [id], "props": props}))?;
        warnings.extend(r.get("warnings").and_then(Value::as_array).into_iter().flatten().cloned());
        changed.push(id);
    }
    let mut out = json!({"changed": changed, "skipped": skipped});
    if !warnings.is_empty()
        && let Some(o) = out.as_object_mut()
    {
        o.insert("warnings".into(), Value::Array(warnings));
    }
    Ok(out)
}

/// The `node.set` props that give `n` each adjustment, and why it keeps the ones that don't apply.
fn plan(doc: &Document, n: &Node, deltas: &[(&str, f64)]) -> (Map<String, Value>, Vec<String>) {
    let mut props = Map::new();
    let mut why = Vec::new();
    for &(key, d) in deltas {
        if d == 0.0 {
            continue;
        }
        match key {
            "dx" | "dy" => {
                let in_auto_layout = n.parent.and_then(|p| doc.node(p)).is_some_and(|p| p.layout.is_some()) && !n.child_layout.absolute;
                if in_auto_layout {
                    why.push(format!("`{key}`: the auto layout of its parent positions it"));
                } else {
                    let (x, y) = origin(doc, n);
                    let (name, now) = if key == "dx" { ("x", x) } else { ("y", y) };
                    props.insert(name.into(), json!(now + d));
                }
            }
            "dw" => {
                props.insert("width".into(), json!((n.width + d).max(0.0)));
            }
            "dh" => {
                props.insert("height".into(), json!((n.height + d).max(0.0)));
            }
            "fontSize" => match n.kind.text() {
                None => why.push("`fontSize`: only text layers have a font size".into()),
                Some(t) => {
                    let mut sizes = t.spans().into_iter().map(|(_, _, style)| style.font_size);
                    let first = sizes.next().unwrap_or(t.style.font_size);
                    if sizes.any(|z| (z - first).abs() > 1e-9) {
                        why.push("`fontSize`: its characters have different sizes".into());
                    } else {
                        props.insert("fontSize".into(), json!((first + d).clamp(1.0, 5000.0)));
                    }
                }
            },
            "cornerRadius" => {
                if n.kind.is_frame_like() || matches!(n.kind, NodeKind::Rectangle) {
                    props.insert("cornerRadius".into(), json!(n.corners.radii.map(|r| (r + d).max(0.0))));
                } else {
                    why.push("`cornerRadius`: only frames and rectangles have corners".into());
                }
            }
            "gap" | "padding" => match &n.layout {
                None => why.push(format!("`{key}`: only auto layout frames have one")),
                Some(l) if key == "gap" => {
                    props.insert("gap".into(), json!(l.gap + d));
                }
                Some(l) => {
                    props.insert("padding".into(), json!(l.padding.map(|v| (v + d).max(0.0))));
                }
            },
            "opacity" => {
                let d = if d.abs() > 1.0 { d / 100.0 } else { d };
                props.insert("opacity".into(), json!((f64::from(n.opacity) + d).clamp(0.0, 1.0)));
            }
            "rotation" => {
                props.insert("rotation".into(), json!(figcraft_geom::rotation_degrees(n.transform) + d));
            }
            _ => {}
        }
    }
    (props, why)
}

/// The x and y `node.set` reads: the layer's origin in its parent frame (groups don't count).
fn origin(doc: &Document, n: &Node) -> (f64, f64) {
    let group = n.parent.filter(|p| doc.node(*p).is_some_and(|pn| pn.kind.bounds_follow_children()));
    let space = group.map_or(Affine::IDENTITY, |g| doc.frame_relative(g));
    let c = (space * n.transform).as_coeffs();
    (c[4], c[5])
}
