//! Verification after a bulk edit, against a saved version (a checkpoint):
//! - `check.changes`: layout, accessibility, token and overflow findings now, minus the ones the
//!   version already had, so an agent sees only what its edit introduced;
//! - `visual.diff`: each top-level frame rendered now and in the version, the frames whose pixels
//!   differ, where, and whether the last guarded edit was aimed at them.
//!
//! Both read the version through the version history (`file.saveVersion`, `version.list`); the
//! version's checks run in a scratch session, so the open document is untouched.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use figcraft_doc::{Document, NodeId, NodeKind};
use figcraft_render::Pixmap;
use serde_json::{Value, json};

use super::CommandSpec;
use crate::params::{f64_of, get, id_of, opt_id};
use crate::{Result, Session, bad, spec};

/// Findings one check reads per root (the commands cap `limit` at 5000 and 10000): more than this
/// is reported as `incomplete`.
const CAP: usize = 5000;
/// New issues listed by default, and at most.
const LISTED: usize = 50;
const MAX_LISTED: usize = 500;
const CHECKS: [&str; 4] = ["layout", "a11y", "tokens", "overflow"];
/// Frames compared at once.
const MAX_FRAMES: usize = 50;
/// Pixels one rendered frame may have; a larger frame is rendered at a smaller scale.
const MAX_PIXELS: f64 = 6_000_000.0;
/// A channel differing by more than this (0-255) is a changed pixel.
const TOLERANCE: u8 = 2;
/// Frames on a contact sheet: before, after and diff cells in rows of three.
const SHEET_FRAMES: usize = crate::sheet::MAX_FRAMES / 3;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        spec!(
            "check.changes",
            "Check what an edit broke",
            "{since (a version id from version.list, or its index), root? (a layer), page? (id, name or \"all\"; default the open page), checks? (any of layout, a11y, tokens, overflow; default all), limit? (new issues listed, default 50, up to 500)}  runs lint.layout, a11y.audit, lint.tokens and inspect.overflow on the document now and on the version and returns only the issues that are new → {since: {id, name, time}, ok, new, resolved, counts: {layout, a11y, tokens, overflow} (new issues), issues: [{check, id|ids, name?, ...}], truncated?, incomplete?, note?}. An issue is the same when it is the same check on the same layer(s): one that got worse is not new. `resolved` counts issues the version had and the document no longer has; `incomplete` names checks that found more than 5000 per page (old issues past the cap may then show as new). A layer or page the version doesn't have has all its issues new",
            false,
            check_changes
        ),
        spec!(
            "visual.diff",
            "Diff frames visually",
            "{since (a version id from version.list, or its index), frames? (layer ids; default the top-level frames and sections of the pages in scope), page? (id, name or \"all\"; default the open page), threshold? (percent of the frame's pixels that changed, 0-100; a frame changed by more is listed; default 0 = any change), scale? (0.05-2, default 1; larger frames render smaller so they stay under 6 million pixels), sheet? (a .png path: a contact sheet of the changed frames, each as before, after and diff cells, up to 16 frames)}  renders each frame now and in the version → {since, compared, unchanged, changed: [{id, name, status (changed|added|removed), changedPercent, changedPixels, bbox: {x, y, width, height} (layer units, from the frame's top-left), resized?: {from, to}, targeted, untargeted?}], untargeted: [ids], lastEdit: {known, layers}, sheet?: {path, bytes, frames, columns, cells}, skipped?, truncated?}. `targeted` says the last guarded edit (a write tool call, which carries maxAffected) touched the frame or a layer inside it; `untargeted: true` marks frames that changed without that (a component edit reaching instances, a style or variable edit). `lastEdit.known` is false when the document has changed since, by anything else, so `targeted` is then null",
            false,
            visual_diff
        ),
    ]
}

/// The version `since` names, as `{id, name, time}` and its document.
fn version_doc(cmd: &str, s: &Session, p: &Value) -> Result<(Value, Arc<Document>)> {
    let ds = s.ds()?;
    let q = match get(p, "since") {
        Some(Value::String(id)) => json!({"id": id}),
        Some(Value::Number(n)) if n.is_u64() => json!({"index": n}),
        _ => return Err(bad(cmd, "`since` is required: a version id (version.list) or its index; save one first with file.saveVersion")),
    };
    let i = super::file::version_index(cmd, ds, &q)?;
    let v = ds.versions.get(i).ok_or_else(|| bad(cmd, "no such version"))?;
    Ok((json!({"id": v.id, "name": v.name, "time": v.time}), super::file::read_version(v)?))
}

fn limit_param(cmd: &str, p: &Value) -> Result<usize> {
    match get(p, "limit") {
        None => Ok(LISTED),
        Some(v) => {
            Ok(v.as_u64().filter(|n| *n <= MAX_LISTED as u64).ok_or_else(|| bad(cmd, format!("`limit` must be an integer from 0 to {MAX_LISTED}")))?
                as usize)
        }
    }
}

// ---------- check.changes ----------

/// Findings by identity (`check/...`), each as the compact item that is listed.
type Found = BTreeMap<String, Value>;

/// Layer-level scope of one run: `("page" | "root", id)`.
type Scope = Vec<(&'static str, NodeId)>;

fn check_changes(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "check.changes";
    let (since, before) = version_doc(CMD, s, p)?;
    let limit = limit_param(CMD, p)?;
    let checks: BTreeSet<&str> = match get(p, "checks") {
        None => CHECKS.into_iter().collect(),
        Some(v) => {
            let list = v.as_array().ok_or_else(|| bad(CMD, "`checks` must be a list of check names"))?;
            let mut set = BTreeSet::new();
            for c in list {
                let name = c
                    .as_str()
                    .and_then(|n| CHECKS.iter().find(|k| **k == n))
                    .ok_or_else(|| bad(CMD, format!("unknown check {c}; use {}", CHECKS.join(", "))))?;
                set.insert(*name);
            }
            if set.is_empty() {
                return Err(bad(CMD, "`checks` is empty"));
            }
            set
        }
    };
    let scope: Scope = match opt_id(CMD, p, "root")? {
        Some(r) => {
            if !s.doc()?.contains(r) {
                return Err(bad(CMD, format!("no layer {}", r.0)));
            }
            vec![("root", r)]
        }
        None => super::edit::structure_find::pages(s, CMD, p)?.into_iter().map(|i| ("page", i)).collect(),
    };

    let mut incomplete = BTreeSet::new();
    let now = collect(s, &scope, &checks, &mut incomplete)?;
    // The version's own layers and pages; ones it lacks have no old issues.
    let old_scope: Scope = scope.iter().copied().filter(|(_, id)| before.contains(*id)).collect();
    let mut scratch = Session::with_document((*before).clone());
    let old = collect(&mut scratch, &old_scope, &checks, &mut incomplete)?;

    let new: Vec<(&String, &Value)> = now.iter().filter(|(k, _)| !old.contains_key(*k)).collect();
    let resolved = old.keys().filter(|k| !now.contains_key(*k)).count();
    let mut counts = serde_json::Map::new();
    for c in checks.iter() {
        let n = new.iter().filter(|(k, _)| k.split('/').next() == Some(c)).count();
        counts.insert((*c).to_string(), json!(n));
    }
    let mut out = json!({
        "since": since, "ok": new.is_empty(), "new": new.len(), "resolved": resolved, "counts": counts,
        "issues": new.iter().take(limit).map(|(_, v)| (*v).clone()).collect::<Vec<_>>(),
    });
    if new.len() > limit {
        out["truncated"] = json!(true);
        out["note"] =
            json!(format!("{limit} of {} new issues listed; raise `limit` (up to {MAX_LISTED}) or narrow `root`, `page` or `checks`", new.len()));
    }
    if !incomplete.is_empty() {
        out["incomplete"] = json!(incomplete);
    }
    Ok(out)
}

/// The findings of `checks` under `scope` in `s`'s document.
fn collect(s: &mut Session, scope: &Scope, checks: &BTreeSet<&str>, incomplete: &mut BTreeSet<&'static str>) -> Result<Found> {
    let mut out = Found::new();
    for (key, id) in scope {
        let base = json!({*key: id, "limit": CAP});
        if checks.contains("layout") {
            let r = s.execute("lint.layout", &base)?;
            let listed = r["findings"].as_array().map(Vec::as_slice).unwrap_or_default();
            note_cap(incomplete, "layout", r["total"].as_u64(), listed.len());
            for f in listed {
                let ids = f["ids"].to_string();
                let check = f["check"].as_str().unwrap_or("");
                out.entry(format!("layout/{check}/{ids}"))
                    .or_insert_with(|| json!({"check": format!("layout/{check}"), "ids": f["ids"], "message": f["message"]}));
            }
        }
        if checks.contains("a11y") {
            let r = s.execute("a11y.audit", &base)?;
            let mut listed = 0;
            for kind in ["contrast", "targets", "nonText", "alt"] {
                for f in r["details"][kind].as_array().map(Vec::as_slice).unwrap_or_default() {
                    listed += 1;
                    // The same layer can fail one check in more than one way (non-text: stroke and fill).
                    let which = f["kind"].as_str().unwrap_or("");
                    let mut item = f.clone();
                    if let Some(o) = item.as_object_mut() {
                        for drop in ["fix", "suggestion", "background", "fixable", "boundVariable"] {
                            o.remove(drop);
                        }
                        o.insert("check".into(), json!(format!("a11y/{kind}")));
                    }
                    out.entry(format!("a11y/{kind}/{}/{which}", f["id"])).or_insert(item);
                }
            }
            note_cap(incomplete, "a11y", r["issues"].as_u64(), listed);
        }
        if checks.contains("tokens") {
            let r = s.execute("lint.tokens", &base)?;
            note_cap(incomplete, "tokens", r["total"].as_u64(), r["listed"].as_u64().unwrap_or(0) as usize);
            for h in r["hardcoded"].as_array().map(Vec::as_slice).unwrap_or_default() {
                out.entry(format!("tokens/{}/{}", h["id"], h["field"].as_str().unwrap_or("")))
                    .or_insert_with(|| json!({"check": "tokens", "id": h["id"], "name": h["name"], "field": h["field"], "color": h["color"]}));
            }
        }
        if checks.contains("overflow") {
            let r = s.execute("inspect.overflow", &base)?;
            let listed = r["items"].as_array().map(Vec::as_slice).unwrap_or_default();
            note_cap(incomplete, "overflow", r["total"].as_u64(), listed.len());
            for i in listed {
                let kind = i["kind"].as_str().unwrap_or("");
                out.entry(format!("overflow/{kind}/{}", i["id"]))
                    .or_insert_with(|| json!({"check": format!("overflow/{kind}"), "id": i["id"], "name": i["name"], "detail": i["detail"]}));
            }
        }
    }
    Ok(out)
}

fn note_cap(incomplete: &mut BTreeSet<&'static str>, check: &'static str, total: Option<u64>, listed: usize) {
    if total.is_some_and(|t| t > listed as u64) {
        incomplete.insert(check);
    }
}

// ---------- visual.diff ----------

/// Layers `visual.diff` compares by default: a page's top-level frames and sections.
fn is_frame(doc: &Document, id: NodeId) -> bool {
    doc.node(id).is_some_and(|n| n.kind.is_frame_like() || matches!(n.kind, NodeKind::Section { .. }))
}

/// The layer drawn alone, over its own area, at `scale`.
fn render(doc: &Document, id: NodeId, scale: f64) -> Option<Pixmap> {
    figcraft_render::render_area(doc, &[id], figcraft_render::export_bounds(doc, id)?, scale)
}

fn size_of(doc: &Document, id: NodeId) -> Option<(f64, f64)> {
    figcraft_render::export_bounds(doc, id).map(|b| (b.width(), b.height()))
}

/// How two renders differ over the larger of the two areas (the part one lacks counts as changed
/// where the other draws something): changed pixels, all pixels, the changed pixels' box
/// `(x0, y0, x1, y1)` and, when `mask`, a picture of them.
struct Delta {
    changed: usize,
    total: usize,
    bbox: Option<(u32, u32, u32, u32)>,
    mask: Option<Pixmap>,
}

fn compare(a: &Pixmap, b: &Pixmap, mask: bool) -> Option<Delta> {
    let (w, h) = (a.width().max(b.width()), a.height().max(b.height()));
    let at = |p: &Pixmap, x: u32, y: u32| -> [u8; 4] {
        if x < p.width() && y < p.height() {
            let i = (y as usize * p.width() as usize + x as usize) * 4;
            p.data().get(i..i + 4).and_then(|s| <[u8; 4]>::try_from(s).ok()).unwrap_or([0; 4])
        } else {
            [0; 4]
        }
    };
    let mut pic = if mask { Some(Pixmap::new(w, h)?) } else { None };
    let (mut changed, mut bbox) = (0usize, None::<(u32, u32, u32, u32)>);
    for y in 0..h {
        for x in 0..w {
            let (pa, pb) = (at(a, x, y), at(b, x, y));
            let differs = pa.iter().zip(&pb).any(|(u, v)| u.abs_diff(*v) > TOLERANCE);
            if differs {
                changed += 1;
                bbox = Some(match bbox {
                    Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1)),
                    None => (x, y, x + 1, y + 1),
                });
            }
            if let Some(pic) = pic.as_mut() {
                let i = (y as usize * w as usize + x as usize) * 4;
                let px: [u8; 4] = if differs { [255, 0, 60, 255] } else { [60, 60, 60, 255] };
                if let Some(slot) = pic.data_mut().get_mut(i..i + 4) {
                    slot.copy_from_slice(&px);
                }
            }
        }
    }
    Some(Delta { changed, total: w as usize * h as usize, bbox, mask: pic })
}

fn round(x: f64, places: i32) -> f64 {
    let k = 10f64.powi(places);
    (x * k).round() / k
}

fn visual_diff(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "visual.diff";
    let (since, before) = version_doc(CMD, s, p)?;
    let threshold = match get(p, "threshold") {
        None => 0.0,
        Some(v) => f64_of(v).filter(|t| (0.0..=100.0).contains(t)).ok_or_else(|| bad(CMD, "`threshold` must be a percent from 0 to 100"))?,
    };
    let scale = match get(p, "scale") {
        None => 1.0,
        Some(v) => f64_of(v).filter(|t| (0.05..=2.0).contains(t)).ok_or_else(|| bad(CMD, "`scale` must be a number from 0.05 to 2"))?,
    };
    let sheet = match get(p, "sheet") {
        None => None,
        Some(Value::String(t)) if !t.trim().is_empty() && t.to_ascii_lowercase().ends_with(".png") => Some(t.clone()),
        Some(_) => return Err(bad(CMD, "`sheet` must be a path ending in .png")),
    };
    let ds = s.ds()?;
    let now = ds.doc.clone();
    let touched: Option<Vec<NodeId>> = ds.last_touched.as_ref().filter(|(w, _)| w.ptr_eq(&Arc::downgrade(&now))).map(|(_, ids)| ids.clone());

    // The layers to compare: given, or the top-level frames of the pages in scope, now and then.
    let mut ids: Vec<NodeId> = Vec::new();
    match get(p, "frames") {
        Some(Value::Array(list)) => {
            for v in list {
                let id = id_of(v).ok_or_else(|| bad(CMD, format!("`frames` holds layer ids; {v} is not one")))?;
                if !now.contains(id) && !before.contains(id) {
                    return Err(bad(CMD, format!("no layer {} now or in the version", id.0)));
                }
                if now.node(id).or_else(|| before.node(id)).is_some_and(|n| n.is_page()) {
                    return Err(bad(CMD, format!("{} is a page; name its frames", id.0)));
                }
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        Some(_) => return Err(bad(CMD, "`frames` must be a list of layer ids")),
        None => {
            for page in super::edit::structure_find::pages(s, CMD, p)? {
                let in_now = now.node(page).map(|n| n.children.clone()).unwrap_or_default();
                let in_then = before.node(page).map(|n| n.children.clone()).unwrap_or_default();
                for c in in_now
                    .into_iter()
                    .filter(|c| is_frame(&now, *c))
                    .chain(in_then.into_iter().filter(|c| !now.contains(*c) && is_frame(&before, *c)))
                {
                    if !ids.contains(&c) {
                        ids.push(c);
                    }
                }
            }
        }
    }
    let mut truncated = None;
    if ids.len() > MAX_FRAMES {
        truncated = Some(format!("{} of {} frames compared; pass `frames` or `page` to cover the rest", MAX_FRAMES, ids.len()));
        ids.truncate(MAX_FRAMES);
    }

    let targeted = |id: NodeId| -> Option<bool> {
        let t = touched.as_ref()?;
        Some(t.iter().any(|x| *x == id || now.ancestors(*x).contains(&id) || (!now.contains(*x) && before.ancestors(*x).contains(&id))))
    };

    let (mut changed, mut skipped, mut unchanged) = (Vec::new(), Vec::new(), 0usize);
    let mut cells: Vec<(NodeId, [Pixmap; 3])> = Vec::new();
    for id in &ids {
        let name = now.node(*id).or_else(|| before.node(*id)).map(|n| n.name.clone()).unwrap_or_default();
        let entry = |status: &str, pct: f64, px: usize| {
            let mut e = json!({"id": id, "name": name, "status": status, "changedPercent": pct, "changedPixels": px});
            match targeted(*id) {
                Some(t) => {
                    e["targeted"] = json!(t);
                    if !t {
                        e["untargeted"] = json!(true);
                    }
                }
                None => e["targeted"] = Value::Null,
            }
            e
        };
        let (sn, sb) = (size_of(&now, *id), size_of(&before, *id));
        match (now.contains(*id), before.contains(*id)) {
            (true, false) => changed.push(entry("added", 100.0, 0)),
            (false, true) => changed.push(entry("removed", 100.0, 0)),
            _ => {
                let (Some(sn), Some(sb)) = (sn, sb) else {
                    skipped.push(json!({"id": id, "name": name, "reason": "the layer has nothing to draw now or in the version"}));
                    continue;
                };
                let (w, h) = (sn.0.max(sb.0), sn.1.max(sb.1));
                let eff = scale.min((MAX_PIXELS / (w * h).max(1.0)).sqrt()).max(0.01);
                let (Some(pn), Some(pb)) = (render(&now, *id, eff), render(&before, *id, eff)) else {
                    skipped.push(json!({"id": id, "name": name, "reason": "too large to render"}));
                    continue;
                };
                let Some(d) = compare(&pb, &pn, sheet.is_some()) else {
                    skipped.push(json!({"id": id, "name": name, "reason": "out of memory"}));
                    continue;
                };
                let pct = d.changed as f64 * 100.0 / d.total.max(1) as f64;
                if d.changed == 0 || pct <= threshold {
                    unchanged += 1;
                    continue;
                }
                let mut e = entry("changed", round(pct, 3), d.changed);
                if let Some((x0, y0, x1, y1)) = d.bbox {
                    e["bbox"] = json!({
                        "x": round(f64::from(x0) / eff, 1), "y": round(f64::from(y0) / eff, 1),
                        "width": round(f64::from(x1 - x0) / eff, 1), "height": round(f64::from(y1 - y0) / eff, 1),
                    });
                }
                if (sn.0 - sb.0).abs() > 0.5 || (sn.1 - sb.1).abs() > 0.5 {
                    e["resized"] = json!({"from": [round(sb.0, 1), round(sb.1, 1)], "to": [round(sn.0, 1), round(sn.1, 1)]});
                }
                if let Some(mask) = d.mask
                    && cells.len() < SHEET_FRAMES
                {
                    cells.push((*id, [pb, pn, mask]));
                }
                changed.push(e);
            }
        }
    }
    changed.sort_by(|a, b| b["changedPercent"].as_f64().partial_cmp(&a["changedPercent"].as_f64()).unwrap_or(std::cmp::Ordering::Equal));
    let untargeted: Vec<Value> = changed.iter().filter(|c| c["untargeted"] == json!(true)).map(|c| c["id"].clone()).collect();
    let mut out = json!({
        "since": since, "compared": ids.len(), "unchanged": unchanged, "changed": changed, "untargeted": untargeted,
        "lastEdit": {"known": touched.is_some(), "layers": touched.as_ref().map_or(0, Vec::len)},
    });
    if !skipped.is_empty() {
        out["skipped"] = json!(skipped);
    }
    if let Some(t) = truncated {
        out["truncated"] = json!(t);
    }
    if let Some(path) = sheet {
        out["sheet"] = if cells.is_empty() {
            json!({"written": false, "note": "no frame changed in a way a sheet can show"})
        } else {
            let pics: Vec<Pixmap> = cells.iter().flat_map(|(_, c)| c.iter().cloned()).collect();
            let png = crate::sheet::compose(&pics, 3, 2400).and_then(|px| figcraft_render::encode_png(&px)).map_err(|e| bad(CMD, e))?;
            super::file::write(&path, &png)?;
            json!({"path": path, "bytes": png.len(), "frames": cells.iter().map(|(i, _)| i).collect::<Vec<_>>(), "columns": 3, "cells": "before, after, diff"})
        };
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use figcraft_doc::NodeId;
    use serde_json::{Value, json};

    use crate::Session;

    fn ex(s: &mut Session, cmd: &str, p: Value) -> Value {
        match s.execute(cmd, &p) {
            Ok(v) => v,
            Err(e) => panic!("{cmd} {p}: {e}"),
        }
    }

    fn id(v: &Value) -> NodeId {
        NodeId(v["id"].as_u64().unwrap())
    }

    /// Two frames, each with a rectangle, and a checkpoint of that.
    fn setup() -> (Session, [NodeId; 2], [NodeId; 2], String) {
        let mut s = Session::new();
        let mut frames = [NodeId(0); 2];
        let mut rects = [NodeId(0); 2];
        for i in 0..2 {
            let f = id(&ex(&mut s, "frame.create", json!({"x": i * 300, "y": 0, "width": 200, "height": 100, "props": {"fill": "#FFFFFF"}})));
            let r = id(&ex(&mut s, "shape.rectangle", json!({"x": 20, "y": 20, "width": 60, "height": 40, "parent": f})));
            ex(&mut s, "node.set", json!({"ids": [r], "props": {"fills": [{"type": "solid", "color": "#336699"}]}}));
            frames[i as usize] = f;
            rects[i as usize] = r;
        }
        let v = ex(&mut s, "file.saveVersion", json!({"name": "before"}));
        let vid = v["id"].as_str().unwrap().to_string();
        (s, frames, rects, vid)
    }

    fn guarded_set(s: &mut Session, ids: &[NodeId], props: Value) {
        s.execute_as(Some("Claude"), "node.set", &json!({"ids": ids, "props": props, "maxAffected": 200})).unwrap();
    }

    #[test]
    fn check_changes_lists_only_the_issue_an_edit_introduced() {
        let (mut s, frames, rects, v) = setup();
        let quiet = ex(&mut s, "check.changes", json!({"since": v}));
        assert_eq!((quiet["ok"].clone(), quiet["new"].clone(), quiet["resolved"].clone()), (json!(true), json!(0), json!(0)), "{quiet}");
        // Pushes the rectangle out of its frame.
        guarded_set(&mut s, &[rects[1]], json!({"x": 180}));
        let r = ex(&mut s, "check.changes", json!({"since": v, "checks": ["overflow"]}));
        assert_eq!((r["ok"].clone(), r["new"].clone()), (json!(false), json!(1)), "{r}");
        assert_eq!(r["counts"], json!({"overflow": 1}), "{r}");
        assert_eq!(r["issues"][0]["id"], json!(rects[1]), "{r}");
        assert_eq!(r["issues"][0]["check"], json!("overflow/outside-parent-clipped"), "{r}");
        // Scoped to the other frame it is not seen.
        let other = ex(&mut s, "check.changes", json!({"since": v, "root": frames[0]}));
        assert_eq!(other["new"], json!(0), "{other}");
        // A checkpoint that already holds the problem has nothing new, and moving the layer back resolves it.
        let v2 = ex(&mut s, "file.saveVersion", json!({"name": "after"}));
        assert_eq!(ex(&mut s, "check.changes", json!({"since": v2["id"]}))["new"], json!(0));
        guarded_set(&mut s, &[rects[1]], json!({"x": 20}));
        let fixed = ex(&mut s, "check.changes", json!({"since": v2["id"]}));
        assert_eq!((fixed["new"].clone(), fixed["resolved"].clone()), (json!(0), json!(1)), "{fixed}");
    }

    #[test]
    fn check_changes_sees_new_layers_with_all_their_issues_and_a_limit() {
        let (mut s, frames, _, v) = setup();
        for i in 0..3 {
            ex(&mut s, "shape.rectangle", json!({"x": 190 + i, "y": 0, "width": 40, "height": 40, "parent": frames[0]}));
        }
        let r = ex(&mut s, "check.changes", json!({"since": v, "checks": ["overflow"], "limit": 2}));
        assert_eq!(r["new"], json!(3), "{r}");
        assert_eq!(r["issues"].as_array().unwrap().len(), 2, "{r}");
        assert_eq!(r["truncated"], json!(true), "{r}");
        // The version can also be named by its index.
        let by_index = ex(&mut s, "check.changes", json!({"since": 0, "checks": ["overflow"]}));
        assert_eq!(by_index["new"], json!(3), "{by_index}");
    }

    #[test]
    fn check_changes_junk_parameters_are_errors_not_panics() {
        let (mut s, _, _, v) = setup();
        for junk in [
            json!({}),
            json!({"since": "nope"}),
            json!({"since": 99}),
            json!({"since": -1}),
            json!({"since": [1]}),
            json!({"since": v, "root": "x"}),
            json!({"since": v, "root": 99999}),
            json!({"since": v, "page": 3}),
            json!({"since": v, "checks": "layout"}),
            json!({"since": v, "checks": []}),
            json!({"since": v, "checks": ["nope"]}),
            json!({"since": v, "limit": -1}),
            json!({"since": v, "limit": 100000}),
        ] {
            assert!(s.execute("check.changes", &junk).is_err(), "{junk}");
        }
    }

    #[test]
    fn visual_diff_reports_the_frame_that_changed_and_where() {
        let (mut s, frames, rects, v) = setup();
        let same = ex(&mut s, "visual.diff", json!({"since": v}));
        assert_eq!((same["compared"].clone(), same["unchanged"].clone(), same["changed"].clone()), (json!(2), json!(2), json!([])), "{same}");
        guarded_set(&mut s, &[rects[0]], json!({"fills": [{"type": "solid", "color": "#CC3300"}]}));
        let r = ex(&mut s, "visual.diff", json!({"since": v}));
        assert_eq!(r["unchanged"], json!(1), "{r}");
        let c = &r["changed"][0];
        assert_eq!((c["id"].clone(), c["status"].clone(), c["targeted"].clone()), (json!(frames[0]), json!("changed"), json!(true)), "{r}");
        assert_eq!(c["bbox"], json!({"x": 20.0, "y": 20.0, "width": 60.0, "height": 40.0}), "{r}");
        // 60 x 40 of 200 x 100 pixels.
        assert_eq!(c["changedPixels"], json!(2400), "{r}");
        assert_eq!(c["changedPercent"], json!(12.0), "{r}");
        assert_eq!(r["untargeted"], json!([]), "{r}");
        assert_eq!(r["lastEdit"], json!({"known": true, "layers": 1}), "{r}");
        // Under the threshold it is not listed; over it, it is.
        assert_eq!(ex(&mut s, "visual.diff", json!({"since": v, "threshold": 12.5}))["changed"], json!([]));
        assert_eq!(ex(&mut s, "visual.diff", json!({"since": v, "threshold": 11, "frames": [frames[0]]}))["changed"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn visual_diff_flags_a_frame_the_last_edit_did_not_aim_at() {
        let (mut s, frames, rects, v) = setup();
        // Both frames change, but the edit names one layer in the first.
        ex(&mut s, "node.set", json!({"ids": [rects[1]], "props": {"fills": [{"type": "solid", "color": "#00AA00"}]}}));
        guarded_set(&mut s, &[rects[0]], json!({"fills": [{"type": "solid", "color": "#CC3300"}]}));
        let r = ex(&mut s, "visual.diff", json!({"since": v}));
        assert_eq!(r["changed"].as_array().unwrap().len(), 2, "{r}");
        assert_eq!(r["untargeted"], json!([frames[1]]), "{r}");
        let second = r["changed"].as_array().unwrap().iter().find(|c| c["id"] == json!(frames[1])).unwrap();
        assert_eq!((second["targeted"].clone(), second["untargeted"].clone()), (json!(false), json!(true)), "{r}");
        // Any later edit that is not a guarded one makes the target unknown.
        ex(&mut s, "node.set", json!({"ids": [rects[1]], "props": {"opacity": 0.5}}));
        let r = ex(&mut s, "visual.diff", json!({"since": v}));
        assert_eq!(r["lastEdit"]["known"], json!(false), "{r}");
        assert_eq!(r["changed"][0]["targeted"], Value::Null, "{r}");
        assert_eq!(r["untargeted"], json!([]), "{r}");
    }

    #[test]
    fn visual_diff_reports_added_removed_and_resized_frames() {
        let (mut s, frames, _, v) = setup();
        guarded_set(&mut s, &[frames[0]], json!({"width": 260}));
        let added = id(&ex(&mut s, "frame.create", json!({"x": 700, "y": 0, "width": 50, "height": 50, "props": {"fill": "#FF0000"}})));
        ex(&mut s, "edit.delete", json!({"ids": [frames[1]]}));
        let r = ex(&mut s, "visual.diff", json!({"since": v}));
        let by = |i: NodeId| r["changed"].as_array().unwrap().iter().find(|c| c["id"] == json!(i)).cloned().unwrap_or(Value::Null);
        assert_eq!(by(frames[0])["resized"], json!({"from": [200.0, 100.0], "to": [260.0, 100.0]}), "{r}");
        assert_eq!(by(added)["status"], json!("added"), "{r}");
        assert_eq!(by(frames[1])["status"], json!("removed"), "{r}");
    }

    #[test]
    fn visual_diff_writes_a_contact_sheet_of_the_changed_frames() {
        let (mut s, _, rects, v) = setup();
        let dir = std::env::temp_dir().join(format!("figcraft-visual-diff-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sheet.png").to_string_lossy().to_string();
        // No change: nothing is written.
        let none = ex(&mut s, "visual.diff", json!({"since": v, "sheet": path}));
        assert_eq!(none["sheet"]["written"], json!(false), "{none}");
        assert!(!std::path::Path::new(&path).exists());
        guarded_set(&mut s, &[rects[0]], json!({"fills": [{"type": "solid", "color": "#CC3300"}]}));
        let r = ex(&mut s, "visual.diff", json!({"since": v, "sheet": path}));
        assert_eq!(r["sheet"]["frames"].as_array().unwrap().len(), 1, "{r}");
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.starts_with(&[0x89, b'P', b'N', b'G']), "a PNG file");
        assert_eq!(r["sheet"]["bytes"], json!(bytes.len()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn visual_diff_junk_parameters_are_errors_not_panics() {
        let (mut s, _, _, v) = setup();
        for junk in [
            json!({}),
            json!({"since": "nope"}),
            json!({"since": 99}),
            json!({"since": v, "threshold": -1}),
            json!({"since": v, "threshold": 101}),
            json!({"since": v, "threshold": "x"}),
            json!({"since": v, "scale": 0}),
            json!({"since": v, "scale": 50}),
            json!({"since": v, "frames": "all"}),
            json!({"since": v, "frames": ["x"]}),
            json!({"since": v, "frames": [999999]}),
            json!({"since": v, "frames": [s.page().unwrap().0]}),
            json!({"since": v, "sheet": 5}),
            json!({"since": v, "sheet": ""}),
            json!({"since": v, "sheet": "x.txt"}),
            json!({"since": v, "page": 7}),
        ] {
            assert!(s.execute("visual.diff", &junk).is_err(), "{junk}");
        }
        // And an empty frame list compares nothing.
        let r = ex(&mut s, "visual.diff", json!({"since": v, "frames": []}));
        assert_eq!(r["compared"], json!(0), "{r}");
    }
}
