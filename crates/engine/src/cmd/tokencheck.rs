//! `lint.tokens`: solid fills and strokes that carry a literal colour a colour variable already
//! holds. Agents and designers use it to find values that should follow the design system, and
//! the `fixes` it returns are `variable.bind` calls away from doing so.

use std::collections::{BTreeMap, BTreeSet};

use figcraft_color::PaintKind;
use figcraft_doc::{NodeId, NodeKind, VariableType, VariableValue};
use serde_json::{Value, json};

use super::CommandSpec;
use super::lists::paint_binding_key;
use crate::params::{get, opt_id};
use crate::{Result, Session, bad, spec};

/// Hits listed in full; the counts and `fixes` cover all of them.
const LISTED: usize = 200;

pub fn specs() -> Vec<CommandSpec> {
    vec![spec!(
        "lint.tokens",
        "Find colours that should be variables",
        "{root? (a layer: only its subtree), page? (id, name or \"all\"; default the open page), limit? (hits listed, default 200)}  finds visible solid fills and strokes with no variable bound whose colour equals the default-mode value of a colour variable → {total, listed, hardcoded: [{id, name, field, color, variables: [{id, name}]}], fixes: [{variable, field, ids}], bound}. A fix is `variable.bind {ids, field, variable}`; only hits with exactly one matching variable are in `fixes`",
        false,
        lint_tokens
    )]
}

fn lint_tokens(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "lint.tokens";
    let limit = match get(p, "limit") {
        None | Some(Value::Null) => LISTED,
        Some(v) => v.as_u64().filter(|n| *n <= 5000).ok_or_else(|| bad(CMD, "`limit` must be an integer from 0 to 5000"))? as usize,
    };
    let roots = match opt_id(CMD, p, "root")? {
        Some(r) => vec![r],
        None => super::edit::structure_find::pages(s, CMD, p)?,
    };
    let doc = s.doc()?;
    // Colour -> the colour variables that hold it in their default mode.
    let mut by_color: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for v in doc.variables.variables.iter().filter(|v| v.kind == VariableType::Color) {
        if let Some(VariableValue::Color(c)) = doc.variables.resolve(&v.id, &BTreeMap::new()) {
            by_color.entry(c.to_css_hex()).or_default().push((v.id.clone(), v.name.clone()));
        }
    }
    let (mut hits, mut total, mut bound) = (Vec::new(), 0usize, 0usize);
    let mut fixes: BTreeMap<(String, String), Vec<NodeId>> = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut stack: Vec<NodeId> = roots.iter().rev().copied().collect();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(n) = doc.node(id) else { continue };
        stack.extend(n.children.iter().rev().copied());
        if matches!(n.kind, NodeKind::Page { .. }) {
            continue;
        }
        for (list, fills) in [(&n.fills, true), (&n.strokes, false)] {
            for (i, paint) in list.iter().enumerate().filter(|(_, p)| p.visible) {
                let PaintKind::Solid { color } = &paint.kind else { continue };
                if paint_binding_key(n, fills, i).is_some() {
                    bound += 1;
                    continue;
                }
                let Some(vars) = by_color.get(&color.to_css_hex()) else { continue };
                total += 1;
                let field = format!("{}/{i}/color", if fills { "fills" } else { "strokes" });
                if let [(vid, _)] = vars.as_slice() {
                    fixes.entry((vid.clone(), field.clone())).or_default().push(id);
                }
                if hits.len() < limit {
                    hits.push(json!({
                        "id": id,
                        "name": n.name,
                        "field": field,
                        "color": color.to_css_hex(),
                        "variables": vars.iter().map(|(i, name)| json!({"id": i, "name": name})).collect::<Vec<_>>(),
                    }));
                }
            }
        }
    }
    let fixes: Vec<Value> = fixes.into_iter().map(|((variable, field), ids)| json!({"variable": variable, "field": field, "ids": ids})).collect();
    Ok(json!({"total": total, "listed": hits.len(), "hardcoded": hits, "fixes": fixes, "bound": bound}))
}

#[cfg(test)]
mod tests {
    use figcraft_doc::NodeKind;
    use serde_json::json;

    use crate::Session;

    #[test]
    fn a_literal_colour_equal_to_a_variable_is_found_and_the_fix_binds_it() {
        let mut s = Session::new();
        s.execute("variable.createCollection", &json!({"name": "Tokens"})).unwrap();
        s.execute("variable.create", &json!({"collection": "Tokens", "name": "brand", "type": "color", "value": "#123456"})).unwrap();
        let page = s.page().unwrap();
        let mut ids = Vec::new();
        for (i, c) in ["#123456", "#123456", "#FF0000"].iter().enumerate() {
            let id = s.doc_mut().unwrap().create(page, NodeKind::Rectangle, "R", i as f64 * 50.0, 0.0, 40.0, 40.0).unwrap();
            s.execute("node.set", &json!({"ids": [id], "props": {"fills": [{"type": "solid", "color": c}]}})).unwrap();
            ids.push(id);
        }
        let r = s.execute("lint.tokens", &json!({})).unwrap();
        assert_eq!(r["total"], json!(2), "{r}");
        assert_eq!(r["fixes"][0]["ids"], json!([ids[0], ids[1]]), "{r}");
        let fix = r["fixes"][0].clone();
        s.execute("variable.bind", &json!({"ids": fix["ids"], "field": fix["field"], "variable": fix["variable"]})).unwrap();
        let after = s.execute("lint.tokens", &json!({})).unwrap();
        assert_eq!((after["total"].clone(), after["bound"].clone()), (json!(0), json!(2)), "{after}");
    }

    #[test]
    fn junk_parameters_are_errors_not_panics() {
        let mut s = Session::new();
        for junk in [json!({"root": "x"}), json!({"limit": -1}), json!({"page": 3}), json!({"root": 99999})] {
            let _ = s.execute("lint.tokens", &junk);
        }
    }
}
