//! Plain-text design source: a canonical, diffable text form of a document (see
//! `docs/source-format.md`). JSON Lines: one object per line, keys sorted, no volatile fields, so
//! saving the same document twice gives identical bytes and changing one layer's fill changes one
//! line. [`import_source`] rebuilds the document; source -> import -> source is the identity.

use std::collections::HashMap;

use figcraft_doc::Document;
use serde_json::{Map, Value, json};

use crate::{FormatError, from_json, image_hash, to_json};

/// The version of the source layout. A source written by a newer build is refused.
pub const SOURCE_VERSION: u32 = 1;

/// Live-session state that is not part of the design: left out of the source.
const LIVE_STATE: [&str; 3] = ["timer", "vote", "votes"];

/// An image the source lists (the bytes are kept beside it, by hash).
#[derive(Clone, Debug, PartialEq)]
pub struct ImageRef {
    pub hash: String,
    pub width: u32,
    pub height: u32,
}

/// A document read from source, with what the loader repaired and the images it still needs.
pub struct Imported {
    pub doc: Document,
    pub issues: Vec<String>,
    /// The images the document refers to; add their bytes with [`attach_image`].
    pub images: Vec<ImageRef>,
}

fn corrupt(msg: impl Into<String>) -> FormatError {
    FormatError::Corrupt(msg.into())
}

/// A layer id as a map key: numbers and strings alike.
fn id_key(v: &Value) -> Option<String> {
    match v {
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

/// `v` as JSON text with every object's keys sorted (independent of how `serde_json` orders maps).
fn canon(v: &Value, out: &mut String) {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(k.clone()).to_string());
                out.push(':');
                if let Some(x) = m.get(k) {
                    canon(x, out);
                }
            }
            out.push('}');
        }
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canon(x, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

fn line(v: Value, out: &mut String) {
    canon(&v, out);
    out.push('\n');
}

/// The document as source text.
pub fn export_source(doc: &Document) -> Result<String, FormatError> {
    let Value::Object(mut root) = to_json(doc)? else { return Err(corrupt("the document is not an object")) };
    let nodes = match root.remove("nodes") {
        Some(Value::Object(m)) => m,
        _ => Map::new(),
    };
    for k in LIVE_STATE {
        root.remove(k);
    }
    let pages: Vec<String> = root.get("pages").and_then(Value::as_array).map(|a| a.iter().filter_map(id_key).collect()).unwrap_or_default();
    let mut sections = String::new();
    let mut header = Map::new();
    let mut keys: Vec<String> = root.keys().cloned().collect();
    keys.sort();
    for k in keys {
        let Some(v) = root.remove(&k) else { continue };
        match v {
            Value::Array(a) if !a.is_empty() => {
                for el in a {
                    line(json!({"t": k, "v": el}), &mut sections);
                }
            }
            Value::Object(o) if !o.is_empty() => {
                let mut subs: Vec<(String, Value)> = o.into_iter().collect();
                subs.sort_by(|a, b| a.0.cmp(&b.0));
                for (sk, sv) in subs {
                    match sv {
                        Value::Array(a) if !a.is_empty() && !sk.contains('.') => {
                            for el in a {
                                line(json!({"t": format!("{k}.{sk}"), "v": el}), &mut sections);
                            }
                        }
                        other => line(json!({"t": k, "k": sk, "v": other}), &mut sections),
                    }
                }
            }
            other => {
                header.insert(k, other);
            }
        }
    }
    let mut out = String::new();
    line(json!({"t": "doc", "sourceVersion": SOURCE_VERSION, "v": header}), &mut out);
    out.push_str(&sections);
    // Layers in tree order: pages in page order, each layer before its children.
    let mut nodes: HashMap<String, Value> = nodes.into_iter().collect();
    let mut order: Vec<String> = Vec::with_capacity(nodes.len());
    let mut stack: Vec<String> = Vec::new();
    for p in pages.into_iter().rev() {
        stack.push(p);
    }
    let mut reached: std::collections::HashSet<String> = std::collections::HashSet::new();
    while let Some(id) = stack.pop() {
        let Some(n) = nodes.get(&id) else { continue };
        if !reached.insert(id.clone()) {
            continue;
        }
        let kids: Vec<String> = n.get("children").and_then(Value::as_array).map(|a| a.iter().filter_map(id_key).collect()).unwrap_or_default();
        order.push(id);
        stack.extend(kids.into_iter().rev());
    }
    // Anything the tree does not reach (a damaged file) is kept, by id.
    let mut rest: Vec<String> = nodes.keys().filter(|k| !reached.contains(*k)).cloned().collect();
    rest.sort_by_key(|k| (k.len(), k.clone()));
    order.extend(rest);
    for id in order {
        if let Some(Value::Object(mut n)) = nodes.remove(&id) {
            n.remove("children");
            line(json!({"t": "node", "v": n}), &mut out);
        }
    }
    Ok(out)
}

/// Source text back to a document. Images come without bytes: see [`Imported::images`].
pub fn import_source(text: &str) -> Result<Imported, FormatError> {
    let mut root = Map::new();
    let mut seen_header = false;
    let mut nodes: Vec<(String, Map<String, Value>)> = Vec::new();
    let mut images = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        if raw.trim().is_empty() {
            continue;
        }
        let at = |why: &str| corrupt(format!("source line {}: {why}", i + 1));
        let Value::Object(mut l) = serde_json::from_str::<Value>(raw).map_err(|e| at(&e.to_string()))? else {
            return Err(at("a line is a JSON object"));
        };
        let t = l.get("t").and_then(Value::as_str).ok_or_else(|| at("missing `t`"))?.to_string();
        let v = l.remove("v").ok_or_else(|| at("missing `v`"))?;
        match t.as_str() {
            "doc" => {
                if let Some(ver) = l.get("sourceVersion").and_then(Value::as_u64)
                    && ver > u64::from(SOURCE_VERSION)
                {
                    return Err(FormatError::Newer(u32::try_from(ver).unwrap_or(u32::MAX)));
                }
                let Value::Object(h) = v else { return Err(at("`doc` holds an object")) };
                root.extend(h);
                seen_header = true;
            }
            "node" => {
                let Value::Object(n) = v else { return Err(at("`node` holds an object")) };
                let id = n.get("id").and_then(id_key).ok_or_else(|| at("a node needs an `id`"))?;
                nodes.push((id, n));
            }
            _ => {
                if t == "imageIndex"
                    && let (Some(hash), Some(w), Some(h)) = (v.get("hash").and_then(Value::as_str), v.get("width").and_then(Value::as_u64), v.get("height").and_then(Value::as_u64))
                {
                    images.push(ImageRef { hash: hash.to_string(), width: u32::try_from(w).unwrap_or(0), height: u32::try_from(h).unwrap_or(0) });
                }
                match t.split_once('.') {
                    None => match l.remove("k") {
                        Some(Value::String(k)) => {
                            let Value::Object(m) = root.entry(t.clone()).or_insert_with(|| json!({})) else { return Err(at("a section is both a list and a map")) };
                            m.insert(k, v);
                        }
                        _ => {
                            let Value::Array(a) = root.entry(t.clone()).or_insert_with(|| json!([])) else { return Err(at("a section is both a list and a map")) };
                            a.push(v);
                        }
                    },
                    Some((top, sub)) => {
                        let Value::Object(m) = root.entry(top.to_string()).or_insert_with(|| json!({})) else { return Err(at("a section is both a list and a map")) };
                        let Value::Array(a) = m.entry(sub.to_string()).or_insert_with(|| json!([])) else { return Err(at("a section is both a list and a map")) };
                        a.push(v);
                    }
                }
            }
        }
    }
    if !seen_header {
        return Err(corrupt("source has no `doc` line"));
    }
    let mut kids: HashMap<String, Vec<Value>> = HashMap::new();
    for (_, n) in &nodes {
        if let (Some(parent), Some(idv)) = (n.get("parent").and_then(id_key), n.get("id")) {
            kids.entry(parent).or_default().push(idv.clone());
        }
    }
    let mut map = Map::new();
    for (id, mut n) in nodes {
        if let Some(c) = kids.remove(&id) {
            n.insert("children".into(), Value::Array(c));
        }
        map.insert(id, Value::Object(n));
    }
    root.insert("nodes".into(), Value::Object(map));
    let (doc, issues) = from_json(&Value::Object(root).to_string())?;
    Ok(Imported { doc, issues, images })
}

/// Give `doc` the image `bytes` named by `hash` in the source; refused when the bytes are another image.
pub fn attach_image(doc: &mut Document, hash: &str, bytes: Vec<u8>) -> Result<(), FormatError> {
    if image_hash(&bytes) != hash {
        return Err(corrupt(format!("image {hash}: the bytes are another image")));
    }
    crate::add_image(doc, bytes).map(|_| ())
}
