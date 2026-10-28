use crate::doc_state::Loading;

use super::load::{
    ext, file_stem, import_summary, load_path, load_path_staged, load_with, load_with_staged, read, rest_images, sibling_variables, variables_param,
};
use super::*;

/// How many files File ▸ Open recent keeps.
pub const MAX_RECENT: usize = 20;

/// Put `path` at the top of the recent files.
pub(super) fn remember(s: &mut Session, path: &str) {
    let r = &mut s.prefs.recent_files;
    r.retain(|p| p != path);
    r.insert(0, path.to_string());
    r.truncate(MAX_RECENT);
}

pub(super) fn open_recent(s: &mut Session, p: &Value) -> Result<Value> {
    use crate::file_access::{Access, allow, allowed};
    const CMD: &str = "file.openRecent";
    // Confined to automation roots, the recent files outside them are never listed, opened, removed
    // or named: `index` counts the files listed.
    let listed = |r: &String| allowed(r, Access::Read);
    if p.get("clear").and_then(Value::as_bool) == Some(true) {
        s.prefs.recent_files.retain(|r| !listed(r));
    }
    if let Some(gone) = string(p, "remove") {
        s.prefs.recent_files.retain(|r| *r != gone || !listed(r));
    }
    let path = match (string(p, "path"), num(p, "index")) {
        (Some(path), _) => {
            allow(&path, Access::Read)?;
            Some(path)
        }
        (None, Some(i)) => {
            Some(s.prefs.recent_files.iter().filter(|r| listed(r)).nth(i.max(0.0) as usize).cloned().ok_or_else(|| bad(CMD, "no such recent file"))?)
        }
        _ => None,
    };
    if let Some(path) = path {
        if !crate::file_access::exists(&path) {
            s.prefs.recent_files.retain(|r| *r != path);
            return Err(bad(CMD, format!("{path} no longer exists")));
        }
        return open(s, &json!({"path": path}));
    }
    let files: Vec<Value> = s
        .prefs
        .recent_files
        .iter()
        .filter(|r| listed(r))
        .map(|r| {
            let name = std::path::Path::new(r).file_name().map_or_else(|| r.clone(), |f| f.to_string_lossy().into_owned());
            json!({"path": r, "name": name, "exists": crate::file_access::exists(r)})
        })
        .collect();
    Ok(json!({"files": files}))
}

/// A file decoded ahead of `file.open` (on a worker thread, so the window stays responsive while a
/// large file imports). Put in [`Session::preloaded`]; the next `file.open` for the same path (or
/// `data` and `name`) uses it instead of reading the file, once. The result, errors included, is
/// what `file.open` would have produced.
pub struct Preloaded {
    key: String,
    result: Result<(figcraft_format::Loaded, bool, Option<crate::library::Stamp>)>,
    /// Set when `result` holds only the first page of a large `.fig` file: the whole file arrives
    /// here later (see [`preload_staged`]).
    later: Option<std::sync::mpsc::Receiver<Preloaded>>,
}

/// `.fig` files with at least this many node changes (and several pages) open page-first in the
/// app: the first page shows while the rest of the file is still being built.
pub const STAGED_MIN_NODES: usize = 100_000;

fn data_key(name: &str) -> String {
    format!("data:{name}")
}

/// Decode the file `file.open` would open for `p` (`path`, or `data` and `name`) with no session,
/// so it can run on any thread. `None` when `p` names no file to decode (the command reports why).
pub fn preload(p: &Value) -> Option<Preloaded> {
    let mut whole = None;
    preload_staged(p, usize::MAX, &mut |r| whole = r)?;
    whole
}

/// [`preload`] that may deliver the file in two steps, so the caller can show the first page of a
/// large `.fig` file (at least `min_nodes` node changes, several pages) early. `deliver` is called
/// once with the result, or with the first page alone: that [`Preloaded`] opens the file as a tab
/// that is still loading, and [`Session::poll_loading`] later swaps in the whole file, which this
/// function sends through the channel the first one carries. `None` when `p` names no file.
pub fn preload_staged(p: &Value, min_nodes: usize, deliver: &mut dyn FnMut(Option<Preloaded>)) -> Option<()> {
    let variables = variables_param("file.open", p).ok()?;
    let (key, on_disk, bytes_path) = if let Some(path) = string(p, "path") {
        // Stamped before reading: a change made during the read makes the next save refuse.
        (format!("path:{path}"), crate::library::stat(&path).ok(), Some(path))
    } else {
        (data_key(&string(p, "name")?), None, None)
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let mut rx = Some(rx);
    let mut early = |l: figcraft_format::Loaded| {
        deliver(Some(Preloaded { key: key.clone(), result: Ok((l, true, on_disk.clone())), later: rx.take() }));
    };
    let result = match &bytes_path {
        Some(path) => load_path_staged(path, variables.as_ref(), min_nodes, &mut early),
        None => {
            let (data, name) = (string(p, "data")?, string(p, "name")?);
            let data = data.split_once(',').map_or(data.as_str(), |(_, b)| b);
            base64::engine::general_purpose::STANDARD
                .decode(data.trim())
                .map_err(|e| bad("file.open", e.to_string()))
                .and_then(|bytes| load_with_staged(&bytes, &name, variables.as_ref(), &figcraft_format::rest::no_images, min_nodes, &mut early))
        }
    };
    let whole = Preloaded { key, result: result.map(|(l, rest)| (l, rest, on_disk)), later: None };
    // The first page went out: the rest follows it.
    if rx.is_none() {
        let _ = tx.send(whole);
    } else {
        deliver(Some(whole));
    }
    Some(())
}

pub(super) fn open(s: &mut Session, p: &Value) -> Result<Value> {
    let variables = variables_param("file.open", p)?;
    // Consumed by this call, used or not.
    let pre = s.preloaded.take();
    if string(p, "path").is_none()
        && let Some(d) = string(p, "data")
    {
        let name = req_string("file.open", p, "name")?;
        let mut pre = pre.filter(|x| x.key == data_key(&name));
        let later = pre.as_mut().and_then(|x| x.later.take());
        return open_data(s, &d, &name, variables.as_ref(), pre.map(|x| x.result), later);
    }
    let path = req_string("file.open", p, "path")?;
    // Already open: switch to it.
    if let Some(i) = s.docs.iter().position(|d| d.path.as_deref() == Some(path.as_str())) {
        s.active = i;
        remember(s, &path);
        return Ok(json!({"index": i}));
    }
    let mut later = None;
    let (loaded, rest, on_disk) = match pre.filter(|x| x.key == format!("path:{path}")) {
        Some(mut pre) => {
            later = pre.later.take();
            // The read happened elsewhere: the roots an automation caller is confined to still apply.
            crate::file_access::allow(&path, crate::file_access::Access::Read)?;
            pre.result?
        }
        None => {
            // Stamped before reading: a change made during the read makes the next save refuse.
            let on_disk = crate::library::stat(&path).ok();
            let (loaded, rest) = load_path(&path, variables.as_ref())?;
            (loaded, rest, on_disk)
        }
    };
    remember(s, &path);
    let native = !rest && matches!(ext(&path).as_str(), "figcraft" | "json");
    let warnings = rest || ext(&path) == "svg";
    // A native file that lost something (a damaged image, say): the app says so, not only the result.
    if native && !loaded.issues.is_empty() {
        s.message = Some(match loaded.issues.as_slice() {
            [one] => format!("{}: {one}", file_stem(&path)),
            more => format!("{}: {} problems were repaired or left out while opening it, e.g. {}", file_stem(&path), more.len(), more[0]),
        });
    }
    let summary = import_summary(&loaded.doc);
    let images_left_out = if native { loaded.images_past_budget } else { 0 };
    let mut ds = DocState::new(loaded.doc, native.then(|| path.clone()))
        .with_versions(loaded.versions)
        .with_branches(loaded.branches)
        .with_images_left_in_file(&path, images_left_out);
    ds.on_disk = on_disk.filter(|_| native);
    let loading = later.is_some();
    ds.loading = later.map(Loading::new);
    s.docs.push(ds);
    s.active = s.docs.len() - 1;
    let mut out = json!({"index": s.active, "summary": summary});
    if loading {
        // The notes and counts are those of the whole file, which comes later ([`Session::poll_loading`]).
        out["loading"] = json!(true);
    } else {
        out[if warnings { "warnings" } else { "repairs" }] = json!(loaded.issues);
    }
    Ok(out)
}

/// [`open`] for a file given as base64 `data` (a file dropped without a path on disk): `name` tells
/// the format. It opens as a new document with no path, so the first save asks where to put it.
fn open_data(
    s: &mut Session,
    data: &str,
    name: &str,
    variables: Option<&Value>,
    pre: Option<Result<(figcraft_format::Loaded, bool, Option<crate::library::Stamp>)>>,
    later: Option<std::sync::mpsc::Receiver<Preloaded>>,
) -> Result<Value> {
    let (loaded, rest) = match pre {
        Some(pre) => pre.map(|(l, rest, _)| (l, rest))?,
        None => decode_data(data, name, variables)?,
    };
    let warnings = rest || ext(name) == "svg";
    let summary = import_summary(&loaded.doc);
    let loading = later.is_some();
    let mut ds = DocState::new(loaded.doc, None).with_versions(loaded.versions).with_branches(loaded.branches);
    ds.loading = later.map(Loading::new);
    s.docs.push(ds);
    s.active = s.docs.len() - 1;
    let mut out = json!({"index": s.active, "summary": summary});
    if loading {
        out["loading"] = json!(true);
    } else {
        out[if warnings { "warnings" } else { "repairs" }] = json!(loaded.issues);
    }
    Ok(out)
}

/// A tab opened page-first that has its whole file now (or lost it).
pub struct LoadingDone {
    /// The tab's `uid`.
    pub uid: u64,
    /// What `file.open` would have answered for the whole file (`index`, `summary`, `warnings` or
    /// `repairs`), or why it failed: the tab is closed then.
    pub result: std::result::Result<Value, String>,
}

impl Session {
    /// Whether a tab shows the first page of a file whose other pages are still being built.
    pub fn is_loading(&self) -> bool {
        self.docs.iter().any(|d| d.loading.is_some())
    }

    /// Swaps in the whole document of each tab opened page-first whose file has arrived: the tab
    /// keeps its identity, view and page; selection restarts. A file that failed to load closes its
    /// tab and leaves a message. Call it every frame while [`Session::is_loading`].
    pub fn poll_loading(&mut self) -> Vec<LoadingDone> {
        let mut done = Vec::new();
        let waiting: Vec<(u64, crate::doc_state::Loading)> = self.docs.iter().filter_map(|d| Some((d.uid, d.loading.clone()?))).collect();
        for (uid, loading) in waiting {
            let Some(arrived) = loading.poll() else { continue };
            let outcome = match arrived {
                Ok(pre) => pre.result.map_err(|e| e.to_string()),
                Err(()) => Err("the file stopped loading before its other pages were ready".to_string()),
            };
            let Some(i) = self.docs.iter().position(|d| d.uid == uid) else { continue };
            match outcome {
                Ok((loaded, rest, _)) => {
                    let summary = import_summary(&loaded.doc);
                    let issues = json!(loaded.issues);
                    self.docs[i].finish_loading(loaded);
                    let mut result = json!({"index": i, "summary": summary});
                    result[if rest { "warnings" } else { "repairs" }] = issues;
                    done.push(LoadingDone { uid, result: Ok(result) });
                }
                Err(why) => {
                    let active_uid = self.docs.get(self.active).map(|d| d.uid);
                    self.remove_tabs(|d| d.uid == uid);
                    self.active = active_uid.and_then(|a| self.docs.iter().position(|d| d.uid == a)).unwrap_or(i.min(self.docs.len().saturating_sub(1)));
                    self.message = Some(why.clone());
                    done.push(LoadingDone { uid, result: Err(why) });
                }
            }
        }
        done
    }
}

fn decode_data(data: &str, name: &str, variables: Option<&Value>) -> Result<(figcraft_format::Loaded, bool)> {
    let data = data.split_once(',').map_or(data, |(_, b)| b);
    let bytes = base64::engine::general_purpose::STANDARD.decode(data.trim()).map_err(|e| bad("file.open", e.to_string()))?;
    load_with(&bytes, name, variables, &figcraft_format::rest::no_images)
}

pub(super) fn import(s: &mut Session, p: &Value) -> Result<Value> {
    let path = string(p, "path");
    let variables = variables_param("file.import", p)?;
    let (bytes, name) = if let Some(path) = &path {
        (read(path)?, path.clone())
    } else if let Some(d) = string(p, "data") {
        let d = d.split_once(',').map(|(_, b)| b.to_string()).unwrap_or(d);
        (
            base64::engine::general_purpose::STANDARD.decode(d.trim()).map_err(|e| bad("file.import", e.to_string()))?,
            string(p, "name").unwrap_or_else(|| "import.png".into()),
        )
    } else {
        return Err(bad("file.import", "pass `path` or `data`"));
    };
    let page = s.page()?;
    let (x, y) = (num(p, "x"), num(p, "y"));
    let center = s.ds().map(|d| (d.view.cx, d.view.cy)).unwrap_or((0.0, 0.0));
    match ext(&name).as_str() {
        "svg" => {
            let doc = s.doc_mut()?;
            let (x, y) = (x.unwrap_or(center.0), y.unwrap_or(center.1));
            let imported =
                figcraft_svg::import_svg_with_notes(doc, &bytes, page, x, y, &file_stem(&name)).map_err(|e| EngineError::Other(e.to_string()))?;
            let id = imported.frame;
            // Centred on the point (a paste lands in the middle of the view).
            if boolean(p, "center").unwrap_or(false)
                && let Some(n) = doc.node_mut(id)
            {
                n.transform = figcraft_geom::Affine::translate(((x - n.width / 2.0).round(), (y - n.height / 2.0).round()));
            }
            s.set_selection(vec![id]);
            let mut out = json!({"id": id});
            if !imported.notes.is_empty() {
                out["warnings"] = json!(imported.notes);
            }
            Ok(out)
        }
        "figcraft" | "json" | "fig" | "sketch" => {
            let resolver = path.as_deref().map(rest_images);
            let images: figcraft_format::rest::ImageResolver = match &resolver {
                Some(r) => r,
                None => &figcraft_format::rest::no_images,
            };
            let sibling = if variables.is_none() { path.as_deref().and_then(sibling_variables) } else { None };
            let (other, rest) = load_with(&bytes, &name, variables.as_ref().or(sibling.as_ref()), images)?;
            let summary = import_summary(&other.doc);
            let pages = append_pages(s.doc_mut()?, &other.doc)?;
            let mut out = json!({"pages": pages, "summary": summary});
            if rest {
                out["warnings"] = json!(other.issues);
            }
            Ok(out)
        }
        _ => {
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            crate::cmd::create::place_image(
                s,
                &json!({"data": b64, "x": x.unwrap_or(center.0), "y": y.unwrap_or(center.1), "name": file_stem(&name)}),
            )
        }
    }
}

/// Append `other`'s pages (with their settings and flows, its images, styles and variables) to
/// `doc`. Ids are remapped across all pages at once, so instances keep their main, instance layers
/// their source and interactions and flows their targets wherever those are. Styles, variables and
/// collections whose id the document already has are kept as they are.
pub(super) fn append_pages(doc: &mut Document, other: &Document) -> Result<Vec<NodeId>> {
    for (k, v) in &other.images {
        doc.images.entry(k.clone()).or_insert_with(|| v.clone());
    }
    for st in &other.styles {
        if !doc.styles.iter().any(|s| s.id == st.id) {
            doc.styles.push(st.clone());
        }
    }
    let have: std::collections::BTreeSet<String> = doc.variables.collections.iter().map(|c| c.id.clone()).collect();
    doc.variables.collections.extend(other.variables.collections.iter().filter(|c| !have.contains(&c.id)).cloned());
    let have: std::collections::BTreeSet<String> = doc.variables.variables.iter().map(|v| v.id.clone()).collect();
    doc.variables.variables.extend(other.variables.variables.iter().filter(|v| !have.contains(&v.id)).cloned());
    let mut map = std::collections::BTreeMap::new();
    for pg in &other.pages {
        for id in other.subtree(*pg).into_iter().skip(1) {
            map.insert(id, doc.alloc_id());
        }
    }
    // Library copies a .fig import keeps (cached definitions) have no library record to follow:
    // the layers of its store come along into the store of `doc`. Recorded copies are left to
    // `merge_store`, which shares them with copies `doc` already has.
    let cached: Vec<NodeId> = other
        .library_store()
        .and_then(|s| other.node(s))
        .map(|s| s.children.iter().copied().filter(|root| other.libraries.import_of(*root).is_none()).collect())
        .unwrap_or_default();
    for root in &cached {
        for id in other.subtree(*root) {
            map.insert(id, doc.alloc_id());
        }
    }
    let mut new_pages = Vec::new();
    for pg in &other.pages {
        let Some(pn) = other.node(*pg) else { continue };
        let np = doc.add_page(&pn.name);
        let mut kind = pn.kind.clone();
        if let NodeKind::Page { flows, .. } = &mut kind {
            flows.retain_mut(|f| map.get(&f.start).map(|n| f.start = *n).is_some());
        }
        if let Some(n) = doc.node_mut(np) {
            n.kind = kind;
        }
        for c in &pn.children {
            let nodes: Vec<Node> = other.subtree(*c).into_iter().filter_map(|i| other.node(i).cloned()).collect();
            doc.paste_nodes(&nodes, &map, *c, np, None)?;
        }
        new_pages.push(np);
    }
    if !cached.is_empty() {
        let store = doc.ensure_library_store();
        for root in &cached {
            let nodes: Vec<Node> = other.subtree(*root).into_iter().filter_map(|i| other.node(i).cloned()).collect();
            doc.paste_nodes(&nodes, &map, *root, store, None)?;
        }
    }
    for new in map.values() {
        let src = doc.node(*new).and_then(|n| n.source).and_then(|s| map.get(&s).copied());
        if let (Some(src), Some(n)) = (src, doc.node_mut(*new)) {
            n.source = Some(src);
        }
    }
    crate::library::merge_store(doc, other, &map)?;
    Ok(new_pages)
}
