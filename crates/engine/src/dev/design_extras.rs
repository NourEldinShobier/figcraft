//! The optional extras of `inspect.designContext`: instances with their Code Connect snippets,
//! annotations, dev resources, handoff status, open comments, variable bindings, prototype
//! interactions and a priority-ordered list of hints. Each list is capped.

use std::collections::BTreeSet;

use figcraft_doc::proto::Action;
use figcraft_doc::{Document, Node, NodeId, NodeKind};
use serde_json::{Map, Value, json};

use crate::bad;
use crate::codegen::{CodeLang, CssNames};
use crate::params::get;

const MAX_ITEMS: usize = 100;
const MAX_TEXT: usize = 500;
const MAX_VISIT: usize = 20_000;
const MAX_SNIPPET: usize = 4_000;
const INCLUDES: [&str; 8] = ["instances", "annotations", "devResources", "status", "comments", "bindings", "interactions", "hints"];
/// Left out unless asked for: the long ones.
const HEAVY: [&str; 2] = ["comments", "interactions"];

/// The `include` parameter: the named extras, by default every one but the heavy ones.
pub fn include_arg(cmd: &str, p: &Value) -> crate::Result<Vec<&'static str>> {
    let names: Vec<String> = match get(p, "include") {
        None | Some(Value::Null) => return Ok(INCLUDES.into_iter().filter(|i| !HEAVY.contains(i)).collect()),
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(a)) => {
            a.iter().map(|v| v.as_str().map(str::to_string).ok_or_else(|| bad(cmd, "`include` holds names"))).collect::<crate::Result<_>>()?
        }
        Some(_) => return Err(bad(cmd, "`include` must be a name or a list of names")),
    };
    let mut out = Vec::new();
    for n in &names {
        if n.eq_ignore_ascii_case("all") {
            return Ok(INCLUDES.to_vec());
        }
        let known = INCLUDES.iter().find(|i| i.eq_ignore_ascii_case(n)).ok_or_else(|| bad(cmd, format!("include: all|{}", INCLUDES.join("|"))))?;
        if !out.contains(known) {
            out.push(*known);
        }
    }
    Ok(out)
}

fn clip(s: &str) -> String {
    s.chars().take(MAX_TEXT).collect()
}

/// `v` cut to [`MAX_ITEMS`]; the list's name goes to `trunc` when entries were dropped.
fn capped(mut v: Vec<Value>, name: &'static str, trunc: &mut Vec<&'static str>) -> Vec<Value> {
    if v.len() > MAX_ITEMS {
        v.truncate(MAX_ITEMS);
        trunc.push(name);
    }
    v
}

fn interaction_records(doc: &Document, n: &Node) -> Vec<Value> {
    let target = |d: NodeId| doc.node(d).map_or_else(|| format!("#{}", d.0), |t| format!("{} (#{})", t.name, d.0));
    n.interactions
        .iter()
        .map(|it| {
            let actions: Vec<String> = figcraft_doc::proto::walk(&it.actions)
                .map(|a| match (a, a.dest()) {
                    (Action::OpenUrl { url, .. }, _) => format!("{} {}", a.label(), clip(url)),
                    (_, Some(d)) => format!("{} {}", a.label(), target(d)),
                    _ => a.label().to_string(),
                })
                .collect();
            json!({"id": n.id, "name": n.name, "trigger": it.trigger.label(), "actions": actions, "durationMs": it.duration_ms})
        })
        .collect()
}

/// The extras named in `include` for `root` and its descendants, as fields to merge into the result.
pub fn extras(doc: &Document, root: NodeId, lang: CodeLang, include: &[&str]) -> Map<String, Value> {
    let want = |k: &str| include.contains(&k);
    let ids: Vec<NodeId> = doc.subtree(root).into_iter().take(MAX_VISIT).collect();
    let in_tree: BTreeSet<NodeId> = ids.iter().copied().collect();
    let nodes = || ids.iter().filter_map(|i| doc.node(*i));
    let mut trunc: Vec<&'static str> = Vec::new();
    let mut out = Map::new();

    // Instances (also the base of the hints).
    let cc_lang = if crate::code_connect::language(lang.id()).is_some() { lang } else { CodeLang::React };
    let mut instances = Vec::new();
    if want("instances") || want("hints") {
        for n in nodes() {
            let NodeKind::Instance { component, .. } = n.kind else { continue };
            let name = crate::code_connect::owner(doc, n.id).and_then(|o| doc.node(o)).map_or_else(String::new, |o| o.name.clone());
            let mut j = json!({"id": n.id, "componentName": name, "mainId": component});
            if let Some(l) = crate::code_connect::link(doc, n.id, cc_lang)
                && let Some(code) = crate::code_connect::connected_code(doc, n.id, cc_lang, l)
            {
                j["codeConnect"] =
                    json!({"importPath": l.import_path, "name": l.name, "snippet": code.chars().take(MAX_SNIPPET).collect::<String>()});
            }
            instances.push(j);
        }
    }

    // Bindings: property to variable, as CSS with the raw value as the fallback.
    let mut bindings = Vec::new();
    if want("bindings") || want("hints") {
        let names = CssNames::new(doc);
        for n in nodes().filter(|n| !n.bound.is_empty()) {
            let modes = doc.modes_for(n.id);
            let mut fields = Map::new();
            for (field, vid) in &n.bound {
                let Some(v) = doc.variables.get(vid) else { continue };
                let Some(raw) = doc.variables.resolve(&v.id, &modes).map(|x| match x {
                    figcraft_doc::VariableValue::Color(c) => c.to_css_hex(),
                    other => other.display(),
                }) else {
                    continue;
                };
                let collection = doc.variables.collection(&v.collection).map(|c| c.name.clone());
                fields.insert(
                    field.clone(),
                    json!({"variable": v.name, "collection": collection, "value": raw, "css": format!("var({}, {raw})", names.name(v))}),
                );
            }
            if !fields.is_empty() {
                bindings.push(json!({"id": n.id, "name": n.name, "bound": fields}));
            }
        }
    }

    if want("instances") {
        out.insert("instances".into(), json!(capped(instances.clone(), "instances", &mut trunc)));
    }
    if want("annotations") {
        let list: Vec<Value> =
            nodes().flat_map(|n| n.annotations.iter().enumerate().map(move |(k, a)| crate::annotations::annotation_json(doc, n.id, k, a))).collect();
        out.insert("annotations".into(), json!(capped(list, "annotations", &mut trunc)));
    }
    if want("devResources") {
        let mut list = Vec::new();
        for i in &ids {
            if let Value::Array(a) = crate::dev_resources::list_json(doc, *i) {
                for mut r in a {
                    r["layer"] = json!(i);
                    list.push(r);
                }
            }
        }
        out.insert("devResources".into(), json!(capped(list, "devResources", &mut trunc)));
    }
    if want("status")
        && let Some(n) = doc.node(root)
    {
        let mut j = json!({"status": n.dev_status.id(), "changed": n.dev_changed});
        if !n.dev_note.is_empty() {
            j["note"] = json!(clip(&n.dev_note));
        }
        out.insert("status".into(), j);
    }
    if want("comments") {
        let list: Vec<Value> = doc
            .comments
            .iter()
            .filter(|c| !c.resolved && c.anchor.is_some_and(|a| in_tree.contains(&a)))
            .map(|c| {
                let replies: Vec<Value> = c.replies.iter().take(10).map(|r| json!({"author": r.author, "text": clip(&r.text)})).collect();
                json!({"id": c.id, "anchor": c.anchor, "author": c.first.author, "text": clip(&c.first.text), "replies": replies})
            })
            .collect();
        out.insert("comments".into(), json!(capped(list, "comments", &mut trunc)));
    }
    if want("bindings") {
        out.insert("bindings".into(), json!(capped(bindings.clone(), "bindings", &mut trunc)));
    }
    if want("interactions") {
        let list: Vec<Value> = nodes().flat_map(|n| interaction_records(doc, n)).collect();
        out.insert("interactions".into(), json!(capped(list, "interactions", &mut trunc)));
    }
    if want("hints") {
        let mut hints = Vec::new();
        let mapped: Vec<String> = instances
            .iter()
            .filter(|i| i.get("codeConnect").is_some())
            .map(|i| {
                format!(
                    "{} from {}",
                    i["componentName"].as_str().unwrap_or_default(),
                    i["codeConnect"]["importPath"].as_str().unwrap_or("its mapped module")
                )
            })
            .collect();
        if !mapped.is_empty() {
            hints.push(format!(
                "Use the mapped components first, with their Code Connect snippets instead of generated markup: {}.",
                mapped.iter().take(10).cloned().collect::<Vec<_>>().join("; ")
            ));
        }
        let unmapped = instances.len() - mapped.len();
        if unmapped > 0 {
            hints.push(format!(
                "{unmapped} instance(s) have no Code Connect mapping: reuse the project's own component of the same name where one exists."
            ));
        }
        if !bindings.is_empty() {
            hints.push(
                "Then use the design tokens: write bound properties as the `css` value in `bindings` (var(--name, raw value)), not the raw value."
                    .to_string(),
            );
        }
        hints.push("Use raw values only for what neither a mapped component nor a token covers.".to_string());
        out.insert("hints".into(), json!(hints));
    }
    if !trunc.is_empty() {
        out.insert("truncated".into(), json!(trunc));
    }
    out
}
