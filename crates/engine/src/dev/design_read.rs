//! Read commands shaped like the design-context tools coding agents already know from other design
//! MCP servers: a sparse layer outline, the variables a layer uses, a code-oriented description of
//! a layer and a search over components, variables and styles. All bounded in size; the MCP tools
//! `get_metadata`, `get_variable_defs`, `get_design_context` and `search_design_system` run them.

use std::collections::BTreeMap;

use figcraft_doc::*;
use serde_json::{Value, json};

use crate::cmd::CommandSpec;
use crate::codegen::{CodeLang, code_for};
use crate::devinspect::{assets, properties};
use crate::params::*;
use crate::{bad, spec};

const MAX_DEPTH: usize = 32;
const DEFAULT_NODES: usize = 500;
const MAX_NODES: usize = 5000;
/// Layers visited when collecting variables.
const MAX_VISIT: usize = 20_000;
const MAX_VARIABLES: usize = 500;
const MAX_CODE_CHARS: usize = 60_000;
const MAX_HITS: usize = 200;

fn round(v: f64) -> f64 {
    if v.is_finite() { (v * 100.0).round() / 100.0 } else { 0.0 }
}

/// A resolved variable value as text; colours as `#RRGGBB` (or `#RRGGBBAA`).
fn show(v: &VariableValue) -> String {
    match v {
        VariableValue::Color(c) => c.to_css_hex(),
        other => other.display(),
    }
}

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

struct Outline {
    xml: String,
    nodes: usize,
    max_nodes: usize,
    truncated: bool,
}

fn outline(doc: &Document, id: NodeId, depth: usize, o: &mut Outline) {
    let Some(n) = doc.node(id) else { return };
    o.nodes += 1;
    let tag = n.kind.type_name().to_ascii_lowercase().replace('_', "-");
    o.xml.push_str(&format!("<{tag} id=\"{}\" name=\"{}\"", n.id.0, esc(&n.name)));
    if !n.is_page() {
        let c = doc.frame_relative(id).as_coeffs();
        o.xml.push_str(&format!(" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"", round(c[4]), round(c[5]), round(n.width), round(n.height)));
    }
    if !n.visible {
        o.xml.push_str(" hidden=\"true\"");
    }
    let shown: Vec<NodeId> = if depth == 0 { Vec::new() } else { n.children.iter().copied().take(o.max_nodes.saturating_sub(o.nodes)).collect() };
    let omitted = n.children.len() - shown.len();
    if omitted > 0 {
        o.truncated = true;
        o.xml.push_str(&format!(" omitted=\"{omitted}\""));
    }
    if shown.is_empty() {
        o.xml.push_str("/>\n");
        return;
    }
    o.xml.push_str(">\n");
    for c in shown {
        // Children past the node budget (grandchildren eating it) are counted as omitted by the parent only
        // up to the budget at its start, so check again here.
        if o.nodes >= o.max_nodes {
            o.truncated = true;
            break;
        }
        outline(doc, c, depth - 1, o);
    }
    o.xml.push_str(&format!("</{tag}>\n"));
}

fn metadata_of(doc: &Document, id: NodeId, depth: usize, max_nodes: usize) -> (String, usize, bool) {
    let mut o = Outline { xml: String::new(), nodes: 0, max_nodes, truncated: false };
    outline(doc, id, depth, &mut o);
    (o.xml, o.nodes, o.truncated)
}

/// Variables used by `root` and everything under it (directly or through a linked style): name to
/// the value it resolves to in the layer's own modes.
fn variable_defs(doc: &Document, root: NodeId) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root];
    let mut visited = 0usize;
    while let Some(id) = stack.pop() {
        visited += 1;
        if visited > MAX_VISIT || out.len() >= MAX_VARIABLES {
            break;
        }
        let Some(n) = doc.node(id) else { continue };
        let styles = [&n.styles.fill, &n.styles.stroke, &n.styles.effect, &n.styles.grid, &n.styles.text];
        let mut used: Vec<&VariableId> = n.bound.values().collect();
        used.extend(styles.into_iter().flatten().filter_map(|s| doc.style(s)).flat_map(|s| s.bound.values()));
        if !used.is_empty() {
            let modes = doc.modes_for(id);
            for v in used.into_iter().filter_map(|v| doc.variables.get(v)) {
                if let Some(value) = doc.variables.resolve(&v.id, &modes) {
                    out.insert(v.name.clone(), show(&value));
                }
            }
        }
        stack.extend(n.children.iter().rev().copied());
    }
    out
}

fn depth_arg(cmd: &str, p: &Value, default: usize) -> crate::Result<usize> {
    match get(p, "depth") {
        None | Some(Value::Null) => Ok(default),
        Some(v) => f64_of(v)
            .filter(|d| (0.0..=MAX_DEPTH as f64).contains(d))
            .map(|d| d as usize)
            .ok_or_else(|| bad(cmd, format!("`depth` must be 0 to {MAX_DEPTH}"))),
    }
}

fn layer(cmd: &str, s: &crate::Session, p: &Value, page_default: bool) -> crate::Result<NodeId> {
    let id = match opt_id(cmd, p, "id")? {
        Some(i) => i,
        None if page_default => s.page()?,
        None => targets(s, p).first().copied().ok_or_else(|| bad(cmd, "select a layer or pass `id`"))?,
    };
    if !s.doc()?.contains(id) {
        return Err(bad(cmd, "no such layer"));
    }
    Ok(id)
}

/// How well `q` (lower case) matches `hay`, 0-100, or `None`: exact 100, prefix 90, a match at a
/// word start 80, any substring 70, else a case-insensitive subsequence scored by how tightly the
/// letters sit (`row` in "Rounded Window" scores low; spread over more than 5x the query is no match).
fn fuzzy(hay: &str, q: &str) -> Option<f64> {
    if q.is_empty() {
        return Some(1.0);
    }
    let h = hay.to_lowercase();
    if h == q {
        return Some(100.0);
    }
    if let Some(pos) = h.find(q) {
        let word_start = h[..pos].chars().last().is_none_or(|c| !c.is_alphanumeric());
        return Some(if pos == 0 {
            90.0
        } else if word_start {
            80.0
        } else {
            70.0
        });
    }
    let want: Vec<char> = q.chars().collect();
    let (mut next, mut first, mut last) = (0usize, 0usize, 0usize);
    for (i, c) in h.chars().enumerate() {
        if want.get(next) == Some(&c) {
            if next == 0 {
                first = i;
            }
            last = i;
            next += 1;
        }
    }
    let span = (last - first + 1) as f64;
    (next == want.len()).then(|| 50.0 * want.len() as f64 / span).filter(|s| *s >= 10.0)
}

/// The best of several fields: (score, field name).
fn best<'a>(fields: impl IntoIterator<Item = (&'static str, &'a str, f64)>, q: &str) -> Option<(f64, &'static str)> {
    fields.into_iter().filter_map(|(f, text, weight)| fuzzy(text, q).map(|s| (s * weight, f))).max_by(|a, b| a.0.total_cmp(&b.0))
}

fn ranked(mut hits: Vec<(f64, Value)>, limit: usize) -> Vec<Value> {
    hits.sort_by(|a, b| b.0.total_cmp(&a.0));
    hits.into_iter().take(limit).map(|h| h.1).collect()
}

fn hits(p: &Value, kind: &str) -> bool {
    match get(p, "types") {
        None | Some(Value::Null) => true,
        Some(Value::Array(a)) => a.iter().any(|t| t.as_str().is_some_and(|t| t.eq_ignore_ascii_case(kind))),
        Some(Value::String(t)) => t.eq_ignore_ascii_case(kind),
        Some(_) => false,
    }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        spec!(
            "inspect.metadata",
            "Layer outline",
            "{id? (default the current page), depth? (0-32; default 4), maxNodes? (1-5000; default 500)} → {xml, nodes, truncated}  a sparse XML outline: one element per layer (`<frame id name x y width height>`, x/y relative to the parent, `hidden`, `omitted` = children not shown)",
            false,
            |s, p| {
                let cmd = "inspect.metadata";
                let id = layer(cmd, s, p, true)?;
                let depth = depth_arg(cmd, p, 4)?;
                let max = match get(p, "maxNodes") {
                    None | Some(Value::Null) => DEFAULT_NODES,
                    Some(v) => f64_of(v)
                        .filter(|n| (1.0..=MAX_NODES as f64).contains(n))
                        .map(|n| n as usize)
                        .ok_or_else(|| bad(cmd, "`maxNodes` must be 1 to 5000"))?,
                };
                let (xml, nodes, truncated) = metadata_of(s.doc()?, id, depth, max);
                Ok(json!({"xml": xml, "nodes": nodes, "truncated": truncated}))
            }
        ),
        spec!(
            "inspect.variableDefs",
            "Variables used by a layer",
            "{id? (default the selection)} → {variables: {name: resolved value}, count}  the variables bound to the layer, its descendants and their styles, resolved in the layer's modes (colours as hex)",
            false,
            |s, p| {
                let id = layer("inspect.variableDefs", s, p, false)?;
                let variables = variable_defs(s.doc()?, id);
                Ok(json!({"count": variables.len(), "variables": variables}))
            }
        ),
        spec!(
            "inspect.designContext",
            "Design context",
            "{id? (default the selection), language? (as inspect.code; default html), depth? (0-32; outline depth, default 3)} → {id, name, type, language, code, codeTruncated, properties (as inspect.properties), variables, assets (as inspect.assets), outline}  everything needed to implement a layer in code",
            false,
            |s, p| {
                let cmd = "inspect.designContext";
                let id = layer(cmd, s, p, false)?;
                let lang = match string(p, "language") {
                    Some(l) => CodeLang::from_id(&l).ok_or_else(|| bad(cmd, "language: css|html|react|leptos|vue|tailwind|swiftui|compose|arkui"))?,
                    None => CodeLang::Html,
                };
                let depth = depth_arg(cmd, p, 3)?;
                let doc = s.doc()?;
                let n = doc.node(id).ok_or_else(|| bad(cmd, "no such layer"))?;
                let mut code = code_for(doc, id, lang);
                let code_truncated = match code.char_indices().nth(MAX_CODE_CHARS) {
                    Some((cut, _)) => {
                        code.truncate(cut);
                        true
                    }
                    None => false,
                };
                let (outline, _, _) = metadata_of(doc, id, depth, DEFAULT_NODES);
                Ok(json!({
                    "id": id, "name": n.name, "type": n.kind.type_name(),
                    "language": lang.id(), "code": code, "codeTruncated": code_truncated,
                    "properties": serde_json::to_value(properties(doc, id)).unwrap_or(Value::Null),
                    "variables": variable_defs(doc, id),
                    "assets": serde_json::to_value(assets(doc, &[id])).unwrap_or(Value::Null),
                    "outline": outline,
                }))
            }
        ),
        spec!(
            "inspect.searchDesignSystem",
            "Search design system",
            "{query (fuzzy, case-insensitive: substring, word start or letters in order, against names and descriptions, and a component set's variant names like \"State=Default\"; empty lists everything), types? (\"component\"|\"variable\"|\"style\" or a list; default all), limit? (1-200 per type; default 25)} → {components: [{id, name, type, description, score, matchedOn (name|description|variant), matchedVariant}], variables: [{id, name, collection, type, value, score, matchedOn}], styles: [{id, name, type, description, score, matchedOn}], total, semantic}  best score first (0-100); `total` counts every match; `semantic` is always false: no embedding search",
            false,
            |s, p| {
                let cmd = "inspect.searchDesignSystem";
                let q = string(p, "query").unwrap_or_default().to_lowercase();
                let limit = match get(p, "limit") {
                    None | Some(Value::Null) => 25,
                    Some(v) => f64_of(v)
                        .filter(|n| (1.0..=MAX_HITS as f64).contains(n))
                        .map(|n| n as usize)
                        .ok_or_else(|| bad(cmd, "`limit` must be 1 to 200"))?,
                };
                let doc = s.doc()?;
                let mut components = Vec::new();
                if hits(p, "component") {
                    for n in doc.nodes() {
                        let (NodeKind::Component { description, .. } | NodeKind::ComponentSet { description }) = &n.kind else { continue };
                        // Variants are found through their set.
                        if n.parent.and_then(|pp| doc.node(pp)).is_some_and(|pp| matches!(pp.kind, NodeKind::ComponentSet { .. })) {
                            continue;
                        }
                        let variants: Vec<&Node> = if matches!(n.kind, NodeKind::ComponentSet { .. }) {
                            n.children.iter().filter_map(|c| doc.node(*c)).collect()
                        } else {
                            Vec::new()
                        };
                        let mut found = best([("name", n.name.as_str(), 1.0), ("description", description.as_str(), 0.5)], &q);
                        let mut variant = None;
                        for v in &variants {
                            if let Some((sc, _)) = best([("variant", v.name.as_str(), 0.9)], &q)
                                && found.is_none_or(|(f, _)| sc > f)
                            {
                                found = Some((sc, "variant"));
                                variant = Some(json!({"id": v.id, "name": v.name}));
                            }
                        }
                        if let Some((score, on)) = found {
                            components.push((
                                score,
                                json!({"id": n.id, "name": n.name, "type": n.kind.type_name(), "description": description, "score": round(score), "matchedOn": on, "matchedVariant": variant}),
                            ));
                        }
                    }
                }
                let mut variables = Vec::new();
                if hits(p, "variable") {
                    let none = BTreeMap::new();
                    for v in &doc.variables.variables {
                        if let Some((score, on)) = best([("name", v.name.as_str(), 1.0), ("description", v.description.as_str(), 0.5)], &q) {
                            let collection = doc.variables.collection(&v.collection).map(|c| c.name.clone());
                            let value = doc.variables.resolve(&v.id, &none).map(|x| show(&x));
                            variables.push((
                                score,
                                json!({"id": v.id, "name": v.name, "collection": collection, "type": v.kind.id(), "value": value, "score": round(score), "matchedOn": on}),
                            ));
                        }
                    }
                }
                let mut styles = Vec::new();
                if hits(p, "style") {
                    for st in &doc.styles {
                        if let Some((score, on)) = best([("name", st.name.as_str(), 1.0), ("description", st.description.as_str(), 0.5)], &q) {
                            styles.push((
                                score,
                                json!({"id": st.id, "name": st.name, "type": st.kind.type_name(), "description": st.description, "score": round(score), "matchedOn": on}),
                            ));
                        }
                    }
                }
                let total = components.len() + variables.len() + styles.len();
                Ok(json!({
                    "components": ranked(components, limit), "variables": ranked(variables, limit), "styles": ranked(styles, limit),
                    "total": total, "semantic": false,
                }))
            }
        ),
    ]
}
