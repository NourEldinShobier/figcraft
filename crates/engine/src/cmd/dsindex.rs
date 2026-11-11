//! `ds.index`: the file's design system in one compact read for agents: components, styles and
//! variables with how many layers use each, what is unused, and how much of each enabled library
//! the file uses. It reads only the open file (a library's own totals need its server).

use std::collections::{BTreeMap, BTreeSet};

use figcraft_doc::{Document, NodeId, NodeKind};
use serde_json::{Value, json};

use super::CommandSpec;
use crate::library::{asset_root, local_components, style_links};
use crate::params::{boolean, get};
use crate::{Result, Session, bad, spec};

const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 500;
const KINDS: [&str; 3] = ["components", "styles", "variables"];

pub fn specs() -> Vec<CommandSpec> {
    vec![spec!(
        "ds.index",
        "Design-system index",
        "{kinds? (\"components\"|\"styles\"|\"variables\" or a list; default all), usage? (count the layers using each asset; default true), unusedOnly? (list only assets nothing uses), query? (case-insensitive text in names and descriptions), limit? (entries listed per kind, 1-500; default 50)}  → {components|styles|variables: {total, matched, listed, truncated, entries: [{id, name, kind, description?, usageCount, unused, library?, + variantProperties for a component set, collection and type for a variable, type for a style}]}, collections: [{id, name, modes, variables}], totals: {components, styles, variables, unused: {…}}, unused: [{kind, id, name}], unusedTotal, libraries: [{id, name, version, components|styles|variables: {imported, used}}]}. usageCount is the instances of a component (a set counts its variants), the layers linked to a style or bound to a variable (a style that binds a variable counts as one use); layers inside instances belong to their component. Library components are not entries (see `libraries`); library styles and variables are, with `library`. With usage false there are no counts or unused lists",
        false,
        ds_index
    )]
}

fn kinds(p: &Value) -> Result<Vec<&'static str>> {
    const CMD: &str = "ds.index";
    let names: Vec<&str> = match get(p, "kinds") {
        None | Some(Value::Null) => return Ok(KINDS.to_vec()),
        Some(Value::String(s)) => vec![s.as_str()],
        Some(Value::Array(a)) => a.iter().map(|v| v.as_str().unwrap_or("")).collect(),
        Some(_) => return Err(bad(CMD, "`kinds` must be a string or a list of strings")),
    };
    let mut out = Vec::new();
    for n in names {
        let k = KINDS.iter().find(|k| **k == n).ok_or_else(|| bad(CMD, format!("unknown kind `{n}` (components, styles, variables)")))?;
        if !out.contains(k) {
            out.push(*k);
        }
    }
    Ok(out)
}

/// Layers (not inside an instance, not in the library store) per asset.
#[derive(Default)]
struct Uses {
    components: BTreeMap<NodeId, u64>,
    styles: BTreeMap<String, u64>,
    variables: BTreeMap<String, u64>,
}

fn count_uses(doc: &Document) -> Uses {
    let store = doc.library_store();
    let mut u = Uses::default();
    for n in doc.nodes() {
        if store.is_some_and(|st| doc.page_of(n.id) == Some(st)) || crate::components::inside_instance(doc, n.id) {
            continue;
        }
        if let NodeKind::Instance { component, .. } = &n.kind
            && let Some(root) = asset_root(doc, *component)
        {
            *u.components.entry(root).or_default() += 1;
        }
        for s in style_links(n).collect::<BTreeSet<_>>() {
            *u.styles.entry(s.clone()).or_default() += 1;
        }
        for v in n.bound.values().collect::<BTreeSet<_>>() {
            *u.variables.entry(v.clone()).or_default() += 1;
        }
    }
    for st in &doc.styles {
        for v in st.bound.values().collect::<BTreeSet<_>>() {
            *u.variables.entry(v.clone()).or_default() += 1;
        }
    }
    u
}

/// One kind's section: all entries (unfiltered, in listing order) → the section plus unused rows.
struct Rows {
    kind: &'static str,
    /// (entry, searchable text lowercased, unused)
    all: Vec<(Value, String, bool)>,
}

fn entry(id: Value, name: &str, kind: &str, description: &str, count: Option<u64>) -> Value {
    let mut e = json!({"id": id, "name": name, "kind": kind});
    if !description.is_empty() {
        e["description"] = json!(description);
    }
    if let Some(n) = count {
        e["usageCount"] = json!(n);
        e["unused"] = json!(n == 0);
    }
    e
}

fn ds_index(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ds.index";
    let kinds = kinds(p)?;
    let usage = boolean(p, "usage").unwrap_or(true);
    let unused_only = boolean(p, "unusedOnly").unwrap_or(false);
    if unused_only && !usage {
        return Err(bad(CMD, "`unusedOnly` needs `usage`"));
    }
    let query = match get(p, "query") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(q)) => q.to_lowercase(),
        Some(_) => return Err(bad(CMD, "`query` must be a string")),
    };
    let limit = match get(p, "limit") {
        None | Some(Value::Null) => DEFAULT_LIMIT,
        Some(v) => {
            v.as_u64().filter(|n| (1..=MAX_LIMIT as u64).contains(n)).ok_or_else(|| bad(CMD, "`limit` must be an integer from 1 to 500"))? as usize
        }
    };
    let doc = s.doc()?;
    let uses = if usage { count_uses(doc) } else { Uses::default() };
    let count = |n: u64| usage.then_some(n);
    let lib_names: BTreeMap<&str, &str> = doc.libraries.enabled.iter().map(|l| (l.id.as_str(), l.name.as_str())).collect();
    let mut sections: Vec<Rows> = Vec::new();

    for kind in &kinds {
        let mut all = Vec::new();
        match *kind {
            "components" => {
                for id in local_components(doc) {
                    let Some(n) = doc.node(id) else { continue };
                    let description = match &n.kind {
                        NodeKind::Component { description, .. } | NodeKind::ComponentSet { description } => description.as_str(),
                        _ => "",
                    };
                    let is_set = matches!(n.kind, NodeKind::ComponentSet { .. });
                    let mut e = entry(
                        json!(id),
                        &n.name,
                        if is_set { "componentSet" } else { "component" },
                        description,
                        count(uses.components.get(&id).copied().unwrap_or(0)),
                    );
                    if is_set {
                        e["variantProperties"] = json!(crate::components::variant_options(doc, id).into_iter().map(|(k, _)| k).collect::<Vec<_>>());
                    }
                    let unused = usage && uses.components.get(&id).copied().unwrap_or(0) == 0;
                    all.push((e, format!("{} {}", n.name, description).to_lowercase(), unused));
                }
            }
            "styles" => {
                for st in &doc.styles {
                    let n = uses.styles.get(&st.id).copied().unwrap_or(0);
                    let mut e = entry(json!(st.id), &st.name, "style", &st.description, count(n));
                    e["type"] = json!(st.kind.type_name());
                    if let Some(l) = doc.libraries.style_import(&st.id).and_then(|r| lib_names.get(r.library.as_str())) {
                        e["library"] = json!(l);
                    }
                    all.push((e, format!("{} {}", st.name, st.description).to_lowercase(), usage && n == 0));
                }
            }
            _ => {
                for v in &doc.variables.variables {
                    let n = uses.variables.get(&v.id).copied().unwrap_or(0);
                    let mut e = entry(json!(v.id), &v.name, "variable", &v.description, count(n));
                    e["type"] = json!(v.kind.id());
                    e["collection"] = json!(doc.variables.collection(&v.collection).map(|c| c.name.clone()));
                    if let Some(l) = doc.libraries.variable_import(&v.id).and_then(|r| lib_names.get(r.library.as_str())) {
                        e["library"] = json!(l);
                    }
                    all.push((e, format!("{} {}", v.name, v.description).to_lowercase(), usage && n == 0));
                }
            }
        }
        sections.push(Rows { kind, all });
    }

    let mut out = serde_json::Map::new();
    let (mut totals, mut unused_totals) = (serde_json::Map::new(), serde_json::Map::new());
    let mut unused_rows: Vec<Value> = Vec::new();
    let mut unused_total = 0usize;
    for sec in sections {
        let total = sec.all.len();
        let unused = sec.all.iter().filter(|r| r.2).count();
        totals.insert(sec.kind.into(), json!(total));
        unused_totals.insert(sec.kind.into(), json!(unused));
        unused_total += unused;
        for (e, _, _) in sec.all.iter().filter(|r| r.2) {
            if unused_rows.len() < limit {
                unused_rows.push(json!({"kind": sec.kind, "id": e["id"], "name": e["name"]}));
            }
        }
        let matched: Vec<Value> =
            sec.all.into_iter().filter(|r| (!unused_only || r.2) && (query.is_empty() || r.1.contains(&query))).map(|r| r.0).collect();
        let mut matched = matched;
        if usage {
            matched.sort_by(|a, b| b["usageCount"].as_u64().cmp(&a["usageCount"].as_u64()));
        }
        let n = matched.len();
        matched.truncate(limit);
        out.insert(
            sec.kind.into(),
            json!({"total": total, "matched": n, "listed": matched.len(), "truncated": n > matched.len(), "entries": matched}),
        );
    }
    if kinds.contains(&"variables") {
        let per: BTreeMap<&str, usize> = doc.variables.variables.iter().fold(BTreeMap::new(), |mut m, v| {
            *m.entry(v.collection.as_str()).or_default() += 1;
            m
        });
        out.insert(
            "collections".into(),
            json!(doc
                .variables
                .collections
                .iter()
                .map(|c| json!({"id": c.id, "name": c.name, "modes": c.modes.iter().map(|m| m.name.clone()).collect::<Vec<_>>(), "variables": per.get(c.id.as_str()).copied().unwrap_or(0)}))
                .collect::<Vec<_>>()),
        );
    }
    if usage {
        totals.insert("unused".into(), Value::Object(unused_totals));
    }
    out.insert("totals".into(), Value::Object(totals));
    if usage {
        out.insert("unused".into(), json!(unused_rows));
        out.insert("unusedTotal".into(), json!(unused_total));
        out.insert("unusedTruncated".into(), json!(unused_total > unused_rows.len()));
    }
    out.insert("libraries".into(), json!(libraries(doc, &uses, usage)));
    Ok(Value::Object(out))
}

/// Per enabled library: how many of the assets copied into this file are in use.
fn libraries(doc: &Document, uses: &Uses, usage: bool) -> Vec<Value> {
    let stat = |imported: usize, used: usize| if usage { json!({"imported": imported, "used": used}) } else { json!({"imported": imported}) };
    doc.libraries
        .enabled
        .iter()
        .map(|l| {
            let mine = |recs: &[figcraft_doc::ImportedRecord]| recs.iter().filter(|r| r.library == l.id).map(|r| r.local.clone()).collect::<Vec<_>>();
            let comps: Vec<NodeId> = doc.libraries.components.iter().filter(|r| r.library == l.id).map(|r| r.root).collect();
            let styles = mine(&doc.libraries.styles);
            let vars = mine(&doc.libraries.variables);
            json!({
                "id": l.id, "name": l.name, "version": l.version,
                "components": stat(comps.len(), comps.iter().filter(|c| uses.components.contains_key(c)).count()),
                "styles": stat(styles.len(), styles.iter().filter(|x| uses.styles.contains_key(x.as_str())).count()),
                "variables": stat(vars.len(), vars.iter().filter(|x| uses.variables.contains_key(*x)).count()),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::Session;

    #[test]
    fn counts_uses_and_lists_what_nothing_uses() {
        let mut s = Session::new();
        s.execute("variable.createCollection", &json!({"name": "Tokens"})).unwrap();
        s.execute("variable.create", &json!({"collection": "Tokens", "name": "brand", "type": "color", "value": "#123456"})).unwrap();
        s.execute("variable.create", &json!({"collection": "Tokens", "name": "spare", "type": "color", "value": "#654321"})).unwrap();
        let page = s.page().unwrap();
        let r = s.doc_mut().unwrap().create(page, figcraft_doc::NodeKind::Rectangle, "R", 0.0, 0.0, 40.0, 40.0).unwrap();
        s.execute("node.set", &json!({"ids": [r], "props": {"fills": [{"type": "solid", "color": "#123456"}]}})).unwrap();
        let fix = s.execute("lint.tokens", &json!({})).unwrap()["fixes"][0].clone();
        s.execute("variable.bind", &json!({"ids": fix["ids"], "field": fix["field"], "variable": fix["variable"]})).unwrap();
        s.execute("component.create", &json!({"ids": [r]})).unwrap();

        let all = s.execute("ds.index", &json!({})).unwrap();
        let vars = all["variables"]["entries"].as_array().unwrap();
        let by = |n: &str| vars.iter().find(|v| v["name"] == n).unwrap().clone();
        assert_eq!(by("brand")["usageCount"], json!(1), "{all}");
        assert_eq!(by("spare")["unused"], json!(true), "{all}");
        assert_eq!(by("spare")["collection"], json!("Tokens"));
        assert_eq!(all["totals"]["unused"]["variables"], json!(1), "{all}");
        assert_eq!(all["unused"].as_array().unwrap().iter().filter(|u| u["kind"] == "variables").count(), 1);
        assert_eq!(all["components"]["total"], json!(1), "{all}");
        assert_eq!(all["collections"][0]["variables"], json!(2));
        assert_eq!(all["libraries"], json!([]));

        let only = s.execute("ds.index", &json!({"kinds": "variables", "unusedOnly": true, "query": "SPA"})).unwrap();
        assert_eq!(only["variables"]["listed"], json!(1), "{only}");
        assert!(only.get("components").is_none());
        let none = s.execute("ds.index", &json!({"kinds": ["variables"], "usage": false})).unwrap();
        assert!(none["variables"]["entries"][0].get("usageCount").is_none(), "{none}");
        let cut = s.execute("ds.index", &json!({"kinds": ["variables"], "limit": 1})).unwrap();
        assert_eq!(cut["variables"]["truncated"], json!(true), "{cut}");
    }

    #[test]
    fn junk_parameters_are_errors_not_panics() {
        let mut s = Session::new();
        for junk in [
            json!({"kinds": 3}),
            json!({"kinds": ["bogus"]}),
            json!({"limit": 0}),
            json!({"limit": 99999}),
            json!({"limit": "x"}),
            json!({"query": 5}),
            json!({"usage": false, "unusedOnly": true}),
        ] {
            assert!(s.execute("ds.index", &junk).is_err(), "{junk}");
        }
    }
}
