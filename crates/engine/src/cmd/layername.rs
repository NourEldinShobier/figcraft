//! `layer.autoName`: rename generic layer names ("Vector", "Rectangle 12", "Frame 5") from their
//! context with fixed rules (documented in docs/mcp.md). No model: the same document always gets
//! the same names.

use std::collections::{HashMap, HashSet};

use figcraft_doc::{Document, Node, NodeId, NodeKind};
use serde_json::{Value, json};

use super::CommandSpec;
use crate::params::{bool_of, get, id, req_targets, string};
use crate::{Result, Session, bad, spec};

const MAX_VISIT: usize = 20_000;
/// Longest name taken from a text layer's content.
const MAX_TEXT_NAME: usize = 40;
/// Icons are at most this many pixels on a side.
const ICON_MAX: f64 = 64.0;
const GENERIC: &[&str] =
    &["Vector", "Rectangle", "Ellipse", "Line", "Polygon", "Star", "Frame", "Group", "Union", "Subtract", "Intersect", "Exclude", "Boolean", "Text"];

/// "Rectangle 12" is generic, "Rectangle 12 copy" and "Card" are not.
fn is_generic(name: &str) -> bool {
    let base = name.trim().trim_end_matches(|c: char| c.is_ascii_digit()).trim_end();
    GENERIC.contains(&base)
}

fn one_line(s: &str) -> String {
    let t: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    t.chars().take(MAX_TEXT_NAME).collect::<String>().trim().to_string()
}

struct Ctx<'a> {
    doc: &'a Document,
    /// Names proposed so far, so children see their renamed parents.
    new: HashMap<NodeId, String>,
}

impl Ctx<'_> {
    fn name(&self, n: &Node) -> String {
        self.new.get(&n.id).cloned().unwrap_or_else(|| n.name.clone())
    }

    /// The layer's own name, or the nearest ancestor's when it is generic. A variant stands for its set.
    fn context(&self, id: NodeId) -> Option<String> {
        let mut cur = Some(id);
        for _ in 0..64 {
            let n = self.doc.node(cur?)?;
            let n = match (&n.kind, n.parent.and_then(|p| self.doc.node(p))) {
                (NodeKind::Component { .. }, Some(set)) if matches!(set.kind, NodeKind::ComponentSet { .. }) => set,
                _ => n,
            };
            let name = self.name(n);
            if n.is_page() {
                return None;
            }
            if !is_generic(&name) && !name.trim().is_empty() {
                return Some(name.trim().to_string());
            }
            cur = n.parent;
        }
        None
    }

    fn is_button(&self, id: NodeId) -> bool {
        self.doc.ancestors(id).iter().filter_map(|a| self.doc.node(*a)).any(|a| {
            let l = self.name(a).to_lowercase();
            !a.is_page() && (l.contains("button") || l.contains("btn"))
        })
    }

    fn is_icon_frame(&self, parent: &Node) -> bool {
        let small = parent.width > 0.0 && parent.height > 0.0 && parent.width <= ICON_MAX && parent.height <= ICON_MAX;
        let drawn = |k: &NodeKind| {
            matches!(k, NodeKind::Vector { .. } | NodeKind::BooleanOp { .. } | NodeKind::Rectangle | NodeKind::Ellipse { .. } | NodeKind::Line)
        };
        let only_shapes = parent.children.iter().filter_map(|c| self.doc.node(*c)).all(|c| drawn(&c.kind) || matches!(c.kind, NodeKind::Group));
        let named = self.name(parent).to_lowercase().starts_with("icon");
        matches!(parent.kind, NodeKind::Frame | NodeKind::Component { .. } | NodeKind::Group) && (named || small && only_shapes)
    }

    /// The new name for `n`, if the rules give it one.
    fn propose(&self, n: &Node) -> Option<String> {
        if !is_generic(&n.name) {
            return None;
        }
        let parent = n.parent.and_then(|p| self.doc.node(p)).filter(|p| !p.is_page());
        let ctx = parent.and_then(|p| self.context(p.id));
        let shape = match &n.kind {
            NodeKind::Text(t) => return Some(one_line(&t.characters)).filter(|s| !s.is_empty()),
            NodeKind::Vector { .. } => "path",
            NodeKind::BooleanOp { .. } => "shape",
            NodeKind::Rectangle => "rect",
            NodeKind::Ellipse { .. } => "ellipse",
            NodeKind::Line => "line",
            NodeKind::Polygon { .. } => "polygon",
            NodeKind::Star { .. } => "star",
            NodeKind::Frame | NodeKind::Group => {
                // A container that holds one text layer is named after it.
                let texts: Vec<String> = n
                    .children
                    .iter()
                    .filter_map(|c| self.doc.node(*c))
                    .filter_map(|c| if let NodeKind::Text(t) = &c.kind { Some(one_line(&t.characters)) } else { None })
                    .filter(|t| !t.is_empty())
                    .collect();
                if let [only] = texts.as_slice() {
                    return Some(only.clone());
                }
                let word = if matches!(n.kind, NodeKind::Group) { "group" } else { "frame" };
                return ctx.map(|c| format!("{c}/{word}"));
            }
            _ => return None,
        };
        let icon_like = matches!(n.kind, NodeKind::Vector { .. } | NodeKind::BooleanOp { .. })
            && n.width > 0.0
            && n.height > 0.0
            && n.width <= ICON_MAX
            && n.height <= ICON_MAX
            && parent.is_some_and(|p| self.is_icon_frame(p));
        if icon_like {
            return Some(match ctx {
                Some(c) if c.to_lowercase().starts_with("icon/") => format!("{c}/{shape}"),
                Some(c) => format!("icon/{c}"),
                None => "icon".to_string(),
            });
        }
        // A shape that fills its parent is its background; in a button it is "button/bg".
        let fills_parent = matches!(n.kind, NodeKind::Rectangle | NodeKind::Ellipse { .. })
            && parent.is_some_and(|p| p.width > 0.0 && p.height > 0.0 && n.width >= p.width * 0.9 && n.height >= p.height * 0.9);
        if fills_parent {
            return Some(if self.is_button(n.id) { "button/bg".to_string() } else { format!("{}/bg", ctx?) });
        }
        if self.is_button(n.id) {
            return Some(format!("button/{shape}"));
        }
        ctx.map(|c| format!("{c}/{shape}"))
    }
}

fn walk(doc: &Document, root: NodeId, out: &mut Vec<NodeId>, seen: &mut HashSet<NodeId>) {
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        if out.len() >= MAX_VISIT || !seen.insert(id) {
            continue;
        }
        let Some(n) = doc.node(id) else { continue };
        out.push(id);
        // Instances show their component: their insides are not renamed here.
        if !matches!(n.kind, NodeKind::Instance { .. }) {
            stack.extend(n.children.iter().rev().copied());
        }
    }
}

/// The renames the rules propose for the layers under `roots`: (id, from, to), parents before children.
pub fn propose(doc: &Document, roots: &[NodeId]) -> Vec<(NodeId, String, String)> {
    let mut order = Vec::new();
    let mut seen = HashSet::new();
    for r in roots {
        walk(doc, *r, &mut order, &mut seen);
    }
    let mut ctx = Ctx { doc, new: HashMap::new() };
    let mut out = Vec::new();
    for id in order {
        let Some(n) = doc.node(id) else { continue };
        if n.is_page() || doc.ancestors(id).iter().filter_map(|a| doc.node(*a)).any(|a| matches!(a.kind, NodeKind::Instance { .. })) {
            continue;
        }
        let Some(mut to) = ctx.propose(n).filter(|t| t != &n.name) else { continue };
        // Siblings never share a proposed name: the second gets "-2", the third "-3".
        let taken = |name: &str, ctx: &Ctx| {
            n.parent
                .and_then(|p| doc.node(p))
                .is_some_and(|p| p.children.iter().filter(|c| **c != id).filter_map(|c| doc.node(*c)).any(|c| ctx.name(c) == name))
        };
        let base = to.clone();
        let mut k = 2;
        while taken(&to, &ctx) && k < 1000 {
            to = format!("{base}-{k}");
            k += 1;
        }
        ctx.new.insert(id, to.clone());
        out.push((id, n.name.clone(), to));
    }
    out
}

fn auto_name(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "layer.autoName";
    let dry = match get(p, "dryRun") {
        None | Some(Value::Null) => false,
        Some(v) => bool_of(v).ok_or_else(|| bad(CMD, "`dryRun` must be true or false"))?,
    };
    let explicit = matches!(get(p, "ids"), Some(Value::Array(_))) || id(p, "id").is_some();
    let roots = if explicit { req_targets(CMD, s, p)? } else { s.selection() };
    let scope = match string(p, "scope") {
        Some(sc) => sc,
        None if roots.is_empty() => "page".into(),
        None => "selection".into(),
    };
    let doc = s.doc()?;
    let roots: Vec<NodeId> = match scope.as_str() {
        "selection" => roots,
        "page" => vec![s.page()?],
        "document" => doc.pages.clone(),
        _ => return Err(bad(CMD, "`scope` must be selection, page or document")),
    };
    if roots.iter().any(|r| !doc.contains(*r)) {
        return Err(bad(CMD, "no such layer"));
    }
    let renames = propose(doc, &roots);
    // Touching the document with nothing to rename would still record an undo step.
    if !dry && !renames.is_empty() {
        let doc = s.doc_mut()?;
        for (id, _, to) in &renames {
            if let Some(n) = doc.node_mut(*id) {
                n.name.clone_from(to);
            }
        }
    }
    let list: Vec<Value> = renames.iter().map(|(id, from, to)| json!({"id": id, "from": from, "to": to})).collect();
    Ok(json!({"renames": list, "count": list.len(), "dryRun": dry}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![spec!(
        "layer.autoName",
        "Rename layers from context",
        "{ids? (default the selection), scope? (selection|page|document; default selection, or the current page when nothing is selected), dryRun?} → {renames: [{id, from, to}], count, dryRun}  renames generic names (Vector, Rectangle 12, Frame 5, Group 3, Text 2…) by fixed rules, one undo step; dryRun only returns the proposal",
        true,
        auto_name
    )]
}
