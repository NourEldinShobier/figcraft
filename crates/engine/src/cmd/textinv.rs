//! Text inventory for copy work and translation: `text.extract` lists every text layer with its
//! frame path and box, `text.apply` writes new copy back as one undo step and reports which layers
//! then overflow or truncate, `text.pseudoLocalize` rewrites text with accented letters and
//! bracket padding to stress layouts, and puts it back (`restore`) from a map kept on each layer.

use std::collections::BTreeSet;

use figcraft_doc::{Document, Node, NodeId, NodeKind, TextAutoResize, TextData};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::CommandSpec;
use crate::params::{bool_of, f64_of, get, id_of, opt_id, string};
use crate::{Result, Session, bad, spec};

/// Plugin-data key holding what a layer said before `text.pseudoLocalize` (the stored map).
const ORIG_KEY: &str = "figcraft.l10n.orig";
/// Text layers listed by default, and at most.
const LISTED: usize = 500;
const MAX_LISTED: u64 = 5000;
/// Changes one `text.apply` takes, and layers one call looks at.
const MAX_CHANGES: usize = 5000;
const MAX_SCAN: usize = 200_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        spec!(
            "text.extract",
            "List all text for copy work",
            "{root? (a layer: only its subtree), page? (id, name or \"all\"; default the open page), limit? (layers listed, default 500, up to 5000)}  every text layer in layer order, so those of one frame are together → {total, listed, truncated?, texts: [{id, name, characters, path (names of the layers above it, outermost first), frame (nearest frame-like layer id or null), fontSize, maxWidth? (a fixed-width box), autoResize (widthAndHeight|height|none|truncate), styled (character runs with their own style or link), inInstance?}]}",
            false,
            extract
        ),
        spec!(
            "text.apply",
            "Write new copy into text layers",
            "{changes (array of {id, characters}), flatten? (drop character runs on layers that have them; without it such layers are refused), dryRun? (report only)}  one undo step; the layout is checked after the change → {applied, unchanged, dryRun, results: [{id, name, changed, fit (ok|overflow|truncated), was, lines, height, lost?: {runs, styledChars, links, properties}}], refused: [{id, reason, lost}]}",
            true,
            apply
        ),
        spec!(
            "text.pseudoLocalize",
            "Pseudo-localize text",
            "{root? (a layer: only its subtree), page? (id, name or \"all\"; default the open page), expand? (0 to 3, extra length as a share of each paragraph, default 0.35), mode? (accent|expand|both|restore; default both)}  accent swaps letters for accented ones (placeholders like {name}, <b> and %s stay), expand wraps each paragraph in [ ] and pads it, restore puts back the stored originals of layers still as pseudo-localized; layers inside instances follow their main component → {mode, changed, restored, skipped: [{id, reason}], overflow: [{id, name, fit}]}",
            true,
            pseudo
        ),
    ]
}

fn flag(cmd: &str, p: &Value, key: &str) -> Result<bool> {
    match get(p, key) {
        None | Some(Value::Null) => Ok(false),
        Some(v) => bool_of(v).ok_or_else(|| bad(cmd, format!("`{key}` must be true or false"))),
    }
}

fn is_container(n: &Node) -> bool {
    matches!(
        n.kind,
        NodeKind::Frame | NodeKind::Section { .. } | NodeKind::Component { .. } | NodeKind::Instance { .. } | NodeKind::ComponentSet { .. }
    )
}

/// Text layers under `roots`, in layer order (pre-order), at most [`MAX_SCAN`] layers looked at.
fn text_layers(doc: &Document, roots: &[NodeId]) -> Vec<NodeId> {
    let (mut out, mut seen) = (Vec::new(), BTreeSet::new());
    let mut stack: Vec<NodeId> = roots.iter().rev().copied().collect();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) || seen.len() > MAX_SCAN {
            continue;
        }
        let Some(n) = doc.node(id) else { continue };
        stack.extend(n.children.iter().rev().copied());
        if matches!(n.kind, NodeKind::Text(_)) {
            out.push(id);
        }
    }
    out
}

fn roots_of(s: &Session, cmd: &str, p: &Value) -> Result<Vec<NodeId>> {
    let roots = match opt_id(cmd, p, "root")? {
        Some(r) => vec![r],
        None => super::edit::structure_find::pages(s, cmd, p)?,
    };
    let doc = s.doc()?;
    match roots.iter().find(|r| !doc.contains(**r)) {
        Some(r) => Err(bad(cmd, format!("no layer {r}"))),
        None => Ok(roots),
    }
}

fn extract(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "text.extract";
    let limit = match get(p, "limit") {
        None | Some(Value::Null) => LISTED,
        Some(v) => v.as_u64().filter(|n| *n <= MAX_LISTED).ok_or_else(|| bad(CMD, "`limit` must be an integer from 0 to 5000"))? as usize,
    };
    let roots = roots_of(s, CMD, p)?;
    let doc = s.doc()?;
    let ids = text_layers(doc, &roots);
    let mut texts = Vec::new();
    for id in ids.iter().take(limit) {
        let Some(n) = doc.node(*id) else { continue };
        let NodeKind::Text(t) = &n.kind else { continue };
        let above: Vec<NodeId> =
            doc.ancestors(*id).into_iter().rev().filter(|a| doc.node(*a).is_some_and(|a| !matches!(a.kind, NodeKind::Page { .. }))).collect();
        let path: Vec<&str> = above.iter().filter_map(|a| doc.node(*a)).map(|a| a.name.as_str()).collect();
        let frame = above.iter().rev().find(|a| doc.node(**a).is_some_and(is_container));
        let mut o = json!({
            "id": id,
            "name": n.name,
            "characters": t.characters,
            "path": path,
            "frame": frame,
            "fontSize": t.style.font_size,
            "autoResize": serde_json::to_value(t.auto_resize).unwrap_or(Value::Null),
            "styled": !t.runs.is_empty(),
        });
        if t.auto_resize != TextAutoResize::WidthAndHeight && t.path.is_none() && n.width.is_finite() {
            o["maxWidth"] = json!(n.width);
        }
        if crate::components::inside_instance(doc, *id) {
            o["inInstance"] = json!(true);
        }
        texts.push(o);
    }
    let mut r = json!({"total": ids.len(), "listed": texts.len(), "texts": texts});
    if ids.len() > texts.len() {
        r["truncated"] = json!(true);
    }
    Ok(r)
}

/// How a text sits in its box: "ok", "overflow" (fixed box, text taller than it) or "truncated"
/// (cut off with an ellipsis), with the lines and the height the text needs.
struct Fit {
    status: &'static str,
    lines: usize,
    height: f64,
}

fn fit_of(n: &Node, t: &TextData) -> Fit {
    let (w, h) = match t.auto_resize {
        _ if t.path.is_some() => (None, None),
        TextAutoResize::WidthAndHeight => (None, None),
        TextAutoResize::Height => (Some(n.width), None),
        TextAutoResize::None | TextAutoResize::Truncate => (Some(n.width), Some(n.height)),
    };
    let l = figcraft_text::layout_cached(t, w, h);
    let status = match t.auto_resize {
        _ if t.path.is_some() => "ok",
        TextAutoResize::Truncate if l.truncated => "truncated",
        TextAutoResize::None if l.height > n.height + 0.5 => "overflow",
        _ => "ok",
    };
    Fit { status, lines: l.lines.len(), height: l.height }
}

/// What dropping `t`'s character runs loses.
fn lost_by_flatten(t: &TextData) -> Value {
    let mut props = BTreeSet::new();
    for r in &t.runs {
        let (a, b) = (&r.style, &t.style);
        for (name, differs) in [
            ("fontFamily", a.font_family != b.font_family),
            ("fontStyle", a.font_style != b.font_style),
            ("fontSize", a.font_size != b.font_size),
            ("lineHeight", a.line_height != b.line_height),
            ("letterSpacing", a.letter_spacing != b.letter_spacing),
            ("decoration", a.decoration != b.decoration),
            ("case", a.case != b.case),
            ("fills", a.fills != b.fills),
            ("link", a.link != b.link),
            ("variations", a.variations != b.variations),
            ("textStyle", a.style_id != b.style_id),
        ] {
            if differs {
                props.insert(name);
            }
        }
    }
    json!({
        "runs": t.runs.len(),
        "styledChars": t.runs.iter().map(|r| r.end.saturating_sub(r.start)).sum::<usize>(),
        "links": t.runs.iter().filter(|r| r.style.link.is_some()).count(),
        "properties": props,
    })
}

fn apply(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "text.apply";
    let list = get(p, "changes").and_then(Value::as_array).ok_or_else(|| bad(CMD, "`changes` must be an array of {id, characters}"))?;
    if list.len() > MAX_CHANGES {
        return Err(bad(CMD, format!("at most {MAX_CHANGES} changes in one call")));
    }
    let (flatten, dry) = (flag(CMD, p, "flatten")?, flag(CMD, p, "dryRun")?);
    let mut changes: Vec<(NodeId, String)> = Vec::new();
    for (i, c) in list.iter().enumerate() {
        let id = c.get("id").and_then(id_of).ok_or_else(|| bad(CMD, format!("changes[{i}]: `id` must be a layer id")))?;
        let chars = c.get("characters").and_then(Value::as_str).ok_or_else(|| bad(CMD, format!("changes[{i}]: `characters` must be a string")))?;
        if changes.iter().any(|(j, _)| *j == id) {
            return Err(bad(CMD, format!("changes[{i}]: layer {id} appears twice")));
        }
        changes.push((id, super::props::normalize_line_endings(chars)));
    }
    let doc = s.doc()?;
    let (mut results, mut refused, mut todo, mut unchanged) = (Vec::new(), Vec::new(), Vec::new(), 0usize);
    for (id, new) in changes {
        let n = doc.node(id).ok_or_else(|| bad(CMD, format!("no layer {id}")))?;
        let NodeKind::Text(t) = &n.kind else { return Err(bad(CMD, format!("layer {id} is not text"))) };
        if t.characters == new {
            unchanged += 1;
            continue;
        }
        let lost = (!t.runs.is_empty()).then(|| lost_by_flatten(t));
        if lost.is_some() && !flatten {
            refused.push(json!({"id": id, "reason": "mixed styled runs: pass flatten: true to drop them", "lost": lost}));
            continue;
        }
        let mut after = (**t).clone();
        after.runs.clear();
        let len = after.char_count();
        after.replace_range(0, len, &new);
        let (was, now) = (fit_of(n, t), fit_of(n, &after));
        let mut r =
            json!({"id": id, "name": n.name, "changed": true, "fit": now.status, "was": was.status, "lines": now.lines, "height": now.height});
        if let Some(l) = &lost {
            r["lost"] = l.clone();
        }
        results.push(r);
        todo.push((id, new, lost.is_some()));
    }
    let applied = todo.len();
    if !dry && applied > 0 {
        let ids: Vec<NodeId> = todo.iter().map(|(i, ..)| *i).collect();
        let doc = s.doc_mut()?;
        let before = super::styles::bound_before(doc, &ids);
        for (id, new, drop_runs) in todo {
            if drop_runs && let NodeKind::Text(t) = &mut doc.get_mut(id)?.kind {
                t.runs.clear();
            }
            let mut m = Map::new();
            m.insert("characters".into(), json!(new));
            super::props::apply(CMD, doc, id, &m)?;
        }
        super::styles::detach_edited(doc, &before);
    }
    Ok(json!({"applied": applied, "unchanged": unchanged, "dryRun": dry, "results": results, "refused": refused}))
}

/// What a layer kept so `restore` can put it back: its text as it was, and the characters the
/// pseudo-localizing made (a layer edited since is left alone).
#[derive(Serialize, Deserialize)]
struct Stored {
    orig: TextData,
    made: String,
}

/// Letters to accented look-alikes, one char for one char; everything else stays.
fn accent(c: char) -> char {
    match c {
        'a' => 'á',
        'b' => 'ƀ',
        'c' => 'ç',
        'd' => 'ð',
        'e' => 'é',
        'f' => 'ƒ',
        'g' => 'ğ',
        'h' => 'ĥ',
        'i' => 'í',
        'j' => 'ĵ',
        'k' => 'ķ',
        'l' => 'ļ',
        'm' => 'ḿ',
        'n' => 'ñ',
        'o' => 'ö',
        'p' => 'þ',
        'q' => 'ǫ',
        'r' => 'ŕ',
        's' => 'š',
        't' => 'ţ',
        'u' => 'ü',
        'v' => 'ṽ',
        'w' => 'ŵ',
        'x' => 'ẋ',
        'y' => 'ý',
        'z' => 'ž',
        'A' => 'Å',
        'B' => 'Ɓ',
        'C' => 'Ç',
        'D' => 'Ð',
        'E' => 'É',
        'F' => 'Ƒ',
        'G' => 'Ĝ',
        'H' => 'Ĥ',
        'I' => 'Í',
        'J' => 'Ĵ',
        'K' => 'Ķ',
        'L' => 'Ļ',
        'M' => 'Ḿ',
        'N' => 'Ñ',
        'O' => 'Ö',
        'P' => 'Þ',
        'Q' => 'Ǫ',
        'R' => 'Ŕ',
        'S' => 'Š',
        'T' => 'Ţ',
        'U' => 'Ü',
        'V' => 'Ṽ',
        'W' => 'Ŵ',
        'X' => 'Ẋ',
        'Y' => 'Ý',
        'Z' => 'Ž',
        c => c,
    }
}

/// `s` with letters accented, keeping `{…}`, `<…>` and `%x` placeholders as they are. Same char count.
fn accented(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut keep = vec![false; chars.len()];
    let mut i = 0;
    while i < chars.len() {
        let close = match chars.get(i) {
            Some('{') => Some('}'),
            Some('<') => Some('>'),
            Some('%') if chars.get(i + 1).is_some_and(|c| c.is_ascii_alphanumeric()) => {
                keep[i..=i + 1].fill(true);
                i += 2;
                continue;
            }
            _ => None,
        };
        let end = close.and_then(|c| chars.get(i..).and_then(|r| r.iter().position(|x| *x == c)).map(|k| i + k));
        match end {
            Some(e) => {
                keep[i..=e].fill(true);
                i = e + 1;
            }
            None => i += 1,
        }
    }
    chars.iter().zip(&keep).map(|(c, k)| if *k { *c } else { accent(*c) }).collect()
}

/// Wrap each paragraph in `[ ]` and pad it by `ratio` of its length, through `replace_range` so
/// character runs shift with the text.
fn bracket_pad(t: &mut TextData, ratio: f64) {
    const FILLER: &str = "lorem ipsum dolor sit amet ";
    let mut spans = Vec::new();
    let (mut start, mut len) = (0usize, 0usize);
    for c in t.characters.chars() {
        if c == '\n' {
            spans.push((start, len));
            start += len + 1;
            len = 0;
        } else {
            len += 1;
        }
    }
    spans.push((start, len));
    for (start, len) in spans.into_iter().rev().filter(|(_, l)| *l > 0) {
        let pad = (len as f64 * ratio).ceil() as usize;
        let filler: String = FILLER.chars().cycle().take(pad).collect();
        let tail = if pad == 0 { "]".to_string() } else { format!(" {filler}]") };
        t.replace_range(start + len, start + len, &tail);
        t.replace_range(start, start, "[");
    }
}

fn pseudo(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "text.pseudoLocalize";
    let mode = string(p, "mode").map(|m| m.to_ascii_lowercase()).unwrap_or_else(|| "both".into());
    if !["accent", "expand", "both", "restore"].contains(&mode.as_str()) {
        return Err(bad(CMD, "`mode` must be accent, expand, both or restore"));
    }
    let expand = match get(p, "expand") {
        None | Some(Value::Null) => 0.35,
        Some(v) => f64_of(v).filter(|x| x.is_finite() && (0.0..=3.0).contains(x)).ok_or_else(|| bad(CMD, "`expand` must be a number from 0 to 3"))?,
    };
    let roots = roots_of(s, CMD, p)?;
    let doc = s.doc_mut()?;
    let (mut changed, mut restored, mut skipped, mut overflow) = (0usize, 0usize, Vec::new(), Vec::new());
    for id in text_layers(doc, &roots) {
        if crate::components::inside_instance(doc, id) {
            continue;
        }
        let n = doc.get_mut(id)?;
        let NodeKind::Text(t) = &mut n.kind else { continue };
        let kept = n.plugin_data.get(ORIG_KEY).and_then(|j| serde_json::from_str::<Stored>(j).ok());
        let current_is_made = kept.as_ref().is_some_and(|k| k.made == t.characters);
        if mode == "restore" {
            match kept {
                Some(k) if current_is_made => {
                    **t = k.orig;
                    n.plugin_data.remove(ORIG_KEY);
                    n.overrides.insert("characters".into());
                    restored += 1;
                }
                Some(_) => skipped.push(json!({"id": id, "reason": "edited since it was pseudo-localized"})),
                None => {}
            }
            continue;
        }
        // Run again on the original, so the result does not pile up.
        let source = match kept {
            Some(k) if current_is_made => k.orig,
            _ => (**t).clone(),
        };
        if source.characters.trim().is_empty() {
            continue;
        }
        let mut new = source.clone();
        if mode != "expand" {
            new.characters = accented(&new.characters);
        }
        if mode != "accent" {
            bracket_pad(&mut new, expand);
        }
        let Ok(stored) = serde_json::to_string(&Stored { orig: source, made: new.characters.clone() }) else {
            skipped.push(json!({"id": id, "reason": "could not store the original"}));
            continue;
        };
        **t = new;
        n.plugin_data.insert(ORIG_KEY.into(), stored);
        n.overrides.insert("characters".into());
        changed += 1;
        if let NodeKind::Text(t) = &n.kind {
            let fit = fit_of(n, t);
            if fit.status != "ok" {
                overflow.push(json!({"id": id, "name": n.name, "fit": fit.status}));
            }
        }
    }
    Ok(json!({"mode": mode, "changed": changed, "restored": restored, "skipped": skipped, "overflow": overflow}))
}

#[cfg(test)]
#[path = "textinv_tests.rs"]
mod tests;
