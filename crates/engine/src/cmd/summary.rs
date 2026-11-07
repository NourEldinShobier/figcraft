//! `inspect.summary`: a token-budgeted overview of the open file for an agent that has not seen it:
//! pages, top-level frames with sizes and child counts, component, style and variable counts,
//! repeated element families, headings, and which calls to drill in with. The budget is met by
//! picking a coarser detail level (fewer pages, frames, headings, families, shorter snippets),
//! never by cutting the output off: every level is whole, and says what it left out.

use std::collections::BTreeMap;

use figcraft_doc::{Document, NodeId, NodeKind};
use serde_json::{Value, json};

use super::CommandSpec;
use crate::params::get;
use crate::{Result, Session, bad, spec};

const DEFAULT_BUDGET: u64 = 2000;
/// Layers one call reads.
const MAX_SCAN: usize = 200_000;

/// How much each detail level shows, richest first.
struct Level {
    pages: usize,
    frames: usize,
    headings: usize,
    families: usize,
    snippet: usize,
}

const LEVELS: [Level; 7] = [
    Level { pages: 100, frames: 200, headings: 30, families: 15, snippet: 80 },
    Level { pages: 50, frames: 60, headings: 15, families: 8, snippet: 50 },
    Level { pages: 30, frames: 25, headings: 8, families: 5, snippet: 40 },
    Level { pages: 20, frames: 10, headings: 4, families: 3, snippet: 30 },
    Level { pages: 10, frames: 4, headings: 2, families: 2, snippet: 24 },
    Level { pages: 5, frames: 2, headings: 0, families: 0, snippet: 0 },
    Level { pages: 3, frames: 0, headings: 0, families: 0, snippet: 0 },
];

pub fn specs() -> Vec<CommandSpec> {
    vec![spec!(
        "inspect.summary",
        "Summarise the document",
        "{budgetTokens? (default 2000, 100-50000)}  → {name, counts: {pages, layers, components, componentSets, instances, styles, variables, collections}, pages: [{id, name, layers, frames: [{id, name, type, w, h, kids, layers}], moreFrames?}], morePages?, headings: [{text, size, n, id}], moreHeadings?, families: [{family|name, count, example}], moreFamilies?, drill: [...], level, estTokens, overBudget?}. Sheds detail (pages, frames, headings, families, shorter snippets) to fit the budget; what is left out is counted in the more* fields",
        false,
        summary
    )]
}

struct Frame {
    id: NodeId,
    name: String,
    kind: &'static str,
    w: f64,
    h: f64,
    kids: usize,
    layers: usize,
}

struct Facts {
    pages: Vec<(NodeId, String, usize, Vec<Frame>)>,
    texts: Vec<(f64, String, NodeId)>,
    layers: usize,
    components: usize,
    sets: usize,
    instances: usize,
}

fn gather(cmd: &str, doc: &Document) -> Result<Facts> {
    let mut f = Facts { pages: Vec::new(), texts: Vec::new(), layers: 0, components: 0, sets: 0, instances: 0 };
    for p in &doc.pages {
        let Some(page) = doc.node(*p) else { continue };
        let (mut frames, mut page_layers) = (Vec::new(), 0usize);
        for top in &page.children {
            let Some(t) = doc.node(*top) else { continue };
            let mut count = 0usize;
            let mut stack = vec![*top];
            while let Some(id) = stack.pop() {
                let Some(n) = doc.node(id) else { continue };
                count += 1;
                f.layers += 1;
                if f.layers > MAX_SCAN {
                    return Err(bad(cmd, format!("more than {MAX_SCAN} layers: too large to summarise")));
                }
                match &n.kind {
                    NodeKind::Component { .. } => f.components += 1,
                    NodeKind::ComponentSet { .. } => f.sets += 1,
                    NodeKind::Instance { .. } => f.instances += 1,
                    NodeKind::Text(t) => {
                        let line = t.characters.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
                        if !line.is_empty() {
                            f.texts.push((t.style.font_size, line, id));
                        }
                    }
                    _ => {}
                }
                stack.extend(n.children.iter().copied());
            }
            page_layers += count;
            if t.kind.is_frame_like() || matches!(t.kind, NodeKind::Component { .. } | NodeKind::ComponentSet { .. }) {
                frames.push(Frame {
                    id: *top,
                    name: t.name.clone(),
                    kind: t.kind.type_name(),
                    w: t.width,
                    h: t.height,
                    kids: t.children.len(),
                    layers: count,
                });
            }
        }
        f.pages.push((*p, page.name.clone(), page_layers, frames));
    }
    Ok(f)
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    format!("{}…", s.chars().take(max.saturating_sub(1)).collect::<String>())
}

fn render(doc: &Document, f: &Facts, families: &Value, l: &Level, level: usize) -> Value {
    let styles: BTreeMap<&str, usize> = doc.styles.iter().fold(BTreeMap::new(), |mut m, s| {
        *m.entry(s.kind.type_name()).or_default() += 1;
        m
    });
    let mut out = json!({
        "name": doc.name,
        "counts": {
            "pages": f.pages.len(), "layers": f.layers, "components": f.components, "componentSets": f.sets, "instances": f.instances,
            "styles": styles, "variables": doc.variables.variables.len(), "collections": doc.variables.collections.len(),
        },
    });
    let current = doc.current_page;
    let mut order: Vec<usize> = (0..f.pages.len()).collect();
    order.sort_by_key(|i| f.pages.get(*i).map(|p| Some(p.0) != current));
    let pages: Vec<Value> = order
        .iter()
        .take(l.pages)
        .filter_map(|i| f.pages.get(*i))
        .map(|(id, name, layers, frames)| {
            let mut p = json!({"id": id, "name": clip(name, l.snippet.max(24)), "layers": layers});
            if l.frames > 0 {
                p["frames"] = json!(frames.iter().take(l.frames).map(|fr| {
                    json!({"id": fr.id, "name": clip(&fr.name, l.snippet.max(24)), "type": fr.kind, "w": fr.w.round(), "h": fr.h.round(), "kids": fr.kids, "layers": fr.layers})
                }).collect::<Vec<_>>());
            }
            if frames.len() > l.frames.min(frames.len()) {
                p["moreFrames"] = json!(frames.len() - l.frames);
            }
            p
        })
        .collect();
    out["pages"] = json!(pages);
    if f.pages.len() > l.pages {
        out["morePages"] = json!(f.pages.len() - l.pages);
    }
    // Headings: the largest text, one row per distinct string.
    let mut texts: Vec<&(f64, String, NodeId)> = f.texts.iter().collect();
    texts.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut heads: Vec<(f64, String, usize, NodeId)> = Vec::new();
    for (size, text, id) in texts {
        match heads.iter_mut().find(|h| h.1 == *text) {
            Some(h) => h.2 += 1,
            None => heads.push((*size, text.clone(), 1, *id)),
        }
    }
    let distinct = heads.len();
    out["headings"] = json!(
        heads
            .into_iter()
            .take(l.headings)
            .map(|(s, t, n, id)| json!({"text": clip(&t, l.snippet), "size": s.round(), "n": n, "id": id}))
            .collect::<Vec<_>>()
    );
    if distinct > l.headings {
        out["moreHeadings"] = json!(distinct - l.headings);
    }
    let mut fam: Vec<Value> = Vec::new();
    for (key, name_key) in [("families", "family"), ("suggested", "name")] {
        for x in families.get(key).and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default() {
            let mut row = json!({"count": x.get("count"), "example": x.pointer("/ids/0")});
            row[name_key] = x.get(name_key).cloned().unwrap_or(Value::Null);
            fam.push(row);
        }
    }
    let total = fam.len();
    fam.truncate(l.families);
    out["families"] = json!(fam);
    if total > l.families {
        out["moreFamilies"] = json!(total - l.families);
    }
    out["drill"] = json!([
        "get_nodes {ids} / inspect_document {id, depth} for a frame",
        "find_nodes {type|name} to search; find_similar {id} for a family's other members",
        "get_variable_defs, audit_tokens, lint_layout for tokens and layout checks",
        "compare_version / diff_documents to see what changed",
    ]);
    out["level"] = json!(level);
    out
}

fn summary(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "inspect.summary";
    let budget = match get(p, "budgetTokens") {
        None | Some(Value::Null) => DEFAULT_BUDGET,
        Some(v) => {
            v.as_u64().filter(|n| (100..=50_000).contains(n)).ok_or_else(|| bad(CMD, "`budgetTokens` must be an integer from 100 to 50000"))?
        }
    };
    let facts = gather(CMD, s.doc()?)?;
    let families = super::family::list(s, &json!({"limit": 15})).unwrap_or(Value::Null);
    let doc = s.doc()?;
    let mut last = Value::Null;
    for (i, l) in LEVELS.iter().enumerate() {
        let mut v = render(doc, &facts, &families, l, i);
        // About three characters of compact JSON to a token.
        let est = (v.to_string().len() as u64).div_ceil(3);
        v["estTokens"] = json!(est);
        if est <= budget {
            return Ok(v);
        }
        last = v;
    }
    last["overBudget"] = json!(true);
    Ok(last)
}

#[cfg(test)]
mod tests {
    use figcraft_doc::NodeKind;
    use serde_json::json;

    use crate::Session;

    fn busy(frames: usize) -> Session {
        let mut s = Session::new();
        let page = s.page().unwrap();
        for i in 0..frames {
            let f = s.doc_mut().unwrap().create(page, NodeKind::Frame, &format!("Screen {i}"), i as f64 * 500.0, 0.0, 400.0, 800.0).unwrap();
            for j in 0..3 {
                s.doc_mut().unwrap().create(f, NodeKind::Rectangle, &format!("Box {j}"), 0.0, j as f64 * 50.0, 40.0, 40.0).unwrap();
            }
        }
        s
    }

    #[test]
    fn a_small_file_is_summarised_in_full_with_sizes_and_child_counts() {
        let mut s = busy(2);
        let r = s.execute("inspect.summary", &json!({})).unwrap();
        assert_eq!(r["counts"]["pages"], json!(1), "{r}");
        assert_eq!(r["counts"]["layers"], json!(8), "{r}");
        let frame = &r["pages"][0]["frames"][1];
        assert_eq!(
            (frame["name"].clone(), frame["w"].clone(), frame["kids"].clone(), frame["layers"].clone()),
            (json!("Screen 1"), json!(400.0), json!(3), json!(4)),
            "{r}"
        );
        assert_eq!(r["level"], json!(0), "{r}");
        assert!(r.get("overBudget").is_none(), "{r}");
    }

    #[test]
    fn a_tight_budget_sheds_detail_whole_and_counts_what_it_left_out() {
        let mut s = busy(300);
        let full = s.execute("inspect.summary", &json!({"budgetTokens": 50000})).unwrap();
        let tight = s.execute("inspect.summary", &json!({"budgetTokens": 600})).unwrap();
        assert!(tight["level"].as_u64() > full["level"].as_u64(), "{tight}");
        assert!(tight["estTokens"].as_u64().unwrap() <= 600, "{tight}");
        let shown = tight["pages"][0]["frames"].as_array().map_or(0, Vec::len);
        assert_eq!(shown as u64 + tight["pages"][0]["moreFrames"].as_u64().unwrap_or(0), 300, "{tight}");
    }

    #[test]
    fn junk_parameters_are_errors_not_panics() {
        let mut s = Session::new();
        for junk in [json!({"budgetTokens": 0}), json!({"budgetTokens": -5}), json!({"budgetTokens": "x"}), json!({"budgetTokens": 1e12})] {
            assert!(s.execute("inspect.summary", &junk).is_err(), "{junk}");
        }
        assert!(s.execute("inspect.summary", &json!({"budgetTokens": 100})).is_ok());
    }
}
