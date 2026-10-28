//! The version journal: autosave versions stored as a delta against the version before them.
//! See `docs/version-journal.md` for the file layout.
//!
//! A delta holds, per layer that changed, only the fields that differ (a long array such as a
//! frame's `children` as a splice), new layers whole, the ids of layers removed, and the same
//! kind of patch for the document's other fields. Replaying it on the previous snapshot gives
//! back the document exactly.

use super::*;
use serde_json::Map;

/// A keyframe (a full snapshot) at least every this many versions.
pub(crate) const KEYFRAME_EVERY: usize = 8;
/// ... and when the deltas since the last keyframe pass this many (unpacked) bytes.
pub(crate) const LOG_BYTES: u64 = 20 * 1024 * 1024;
/// The version DAG and checksums; only written when the file holds a delta.
pub(crate) const META: &str = "history/meta.json";
/// Arrays shorter than this are replaced whole instead of spliced.
const SPLICE_MIN: usize = 16;

/// What differs between two JSON objects: fields set (whole), fields gone, and arrays changed
/// in one run, as `[start, removed count, inserted items]`.
#[derive(Serialize, Deserialize, Default)]
struct Patch {
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    set: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    absent: Vec<String>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    splice: Map<String, Value>,
}

fn diff_map(old: &Map<String, Value>, new: &Map<String, Value>) -> Patch {
    let mut p = Patch::default();
    for (k, v) in new {
        let Some(o) = old.get(k) else {
            p.set.insert(k.clone(), v.clone());
            continue;
        };
        if o == v {
            continue;
        }
        if let (Value::Array(a), Value::Array(b)) = (o, v)
            && a.len().max(b.len()) >= SPLICE_MIN
        {
            let pre = a.iter().zip(b).take_while(|(x, y)| x == y).count();
            let suf = a[pre..].iter().rev().zip(b[pre..].iter().rev()).take_while(|(x, y)| x == y).count();
            p.splice.insert(k.clone(), json!([pre, a.len() - pre - suf, &b[pre..b.len() - suf]]));
        } else {
            p.set.insert(k.clone(), v.clone());
        }
    }
    p.absent = old.keys().filter(|k| !new.contains_key(*k)).cloned().collect();
    p
}

fn patch_map(m: &mut Map<String, Value>, p: Patch) -> Result<(), FormatError> {
    let bad = || FormatError::Corrupt("a version delta doesn't fit the version it builds on".into());
    for k in &p.absent {
        m.remove(k);
    }
    for (k, v) in p.splice {
        let (Some(Value::Array(arr)), Some(args)) = (m.get_mut(&k), v.as_array()) else { return Err(bad()) };
        let n = |i: usize| args.get(i).and_then(Value::as_u64).and_then(|n| usize::try_from(n).ok());
        let (Some(start), Some(del), Some(Value::Array(ins))) = (n(0), n(1), args.get(2)) else { return Err(bad()) };
        if start.checked_add(del).is_none_or(|end| end > arr.len()) {
            return Err(bad());
        }
        arr.splice(start..start + del, ins.iter().cloned());
    }
    m.extend(p.set);
    Ok(())
}

/// One layer in a delta: new ones whole (`node`), changed ones as a patch.
#[derive(Serialize, Deserialize)]
struct NodeDelta {
    id: figcraft_doc::NodeId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    node: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    patch: Option<Patch>,
}

#[derive(Serialize, Deserialize, Default)]
struct DeltaJson {
    #[serde(default)]
    shell: Patch,
    #[serde(default)]
    nodes: Vec<NodeDelta>,
    #[serde(default)]
    removed: Vec<figcraft_doc::NodeId>,
}

fn to_value<T: Serialize + ?Sized>(t: &T) -> Result<Value, FormatError> {
    t.serialize(Finite(serde_json::value::Serializer)).map_err(io_err)
}

fn object<T: Serialize + ?Sized>(t: &T) -> Result<Map<String, Value>, FormatError> {
    match to_value(t)? {
        Value::Object(m) => Ok(m),
        _ => Err(io_err("expected a JSON object")),
    }
}

/// The document's fields other than its layers, as JSON.
fn shell_map(doc: &Document) -> Result<Map<String, Value>, FormatError> {
    let mut m = object(&doc.shell())?;
    m.remove("nodes");
    Ok(m)
}

/// The delta that turns `base` into `new`. Cost follows the size of the edit when both share
/// structure (see [`Document::changed_ids`]).
pub(crate) fn delta_between(base: &Document, new: &Document) -> Result<Vec<u8>, FormatError> {
    let mut d = DeltaJson::default();
    for id in new.changed_ids(base, usize::MAX).unwrap_or_default() {
        match (base.node(id), new.node(id)) {
            (Some(o), Some(n)) => d.nodes.push(NodeDelta { id, node: None, patch: Some(diff_map(&object(o)?, &object(n)?)) }),
            (None, Some(n)) => d.nodes.push(NodeDelta { id, node: Some(to_value(n)?), patch: None }),
            (_, None) => d.removed.push(id),
        }
    }
    d.shell = diff_map(&shell_map(base)?, &shell_map(new)?);
    serde_json::to_vec(&d).map_err(io_err)
}

/// `base` with `delta` replayed on it (images are the caller's to set).
pub(crate) fn apply_delta(base: &Document, delta: &[u8]) -> Result<Document, FormatError> {
    let corrupt = |e: serde_json::Error| FormatError::Corrupt(e.to_string());
    let d: DeltaJson = serde_json::from_slice(delta).map_err(corrupt)?;
    let mut shell = shell_map(base)?;
    patch_map(&mut shell, d.shell)?;
    shell.insert("nodes".into(), Value::Object(Default::default()));
    let mut doc: Document = serde_json::from_value(Value::Object(shell)).map_err(corrupt)?;
    doc.set_nodes_of(base);
    for id in d.removed {
        doc.drop_node(id);
    }
    for n in d.nodes {
        let value = match (n.node, n.patch) {
            (Some(v), _) => v,
            (None, p) => {
                let mut m = object(base.node(n.id).ok_or_else(|| FormatError::Corrupt("a version delta changes a missing layer".into()))?)?;
                patch_map(&mut m, p.unwrap_or_default())?;
                Value::Object(m)
            }
        };
        doc.put_node(serde_json::from_value(value).map_err(corrupt)?);
    }
    Ok(doc)
}

pub(crate) fn crc32(bytes: &[u8]) -> u32 {
    let mut c = flate2::Crc::new();
    c.update(bytes);
    c.sum()
}

/// `bytes` as a one-member zip, like a packed snapshot.
fn pack_bytes(bytes: &[u8]) -> Result<Packed, FormatError> {
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    put(&mut z, "v", json_options(), bytes)?;
    Ok(Packed { zip: z.finish().map_err(io_err)?.into_inner(), size: bytes.len() as u64 })
}

impl Version {
    /// This version as a delta against `base` (compressed once; later calls reuse it).
    pub(crate) fn delta_against(&self, base: &Version) -> Result<Arc<Packed>, FormatError> {
        if let Some((id, p)) = self.delta.get()
            && *id == base.id
        {
            return Ok(p.clone());
        }
        let p = match &self.snapshot {
            Snapshot::Delta { base: b, bytes, .. } if b.id == base.id => pack_bytes(bytes)?,
            _ => pack_bytes(&delta_between(&*base.document()?, &*self.document()?)?)?,
        };
        let p = Arc::new(p);
        let _ = self.delta.set((base.id.clone(), p.clone()));
        Ok(p)
    }
}

/// How a version goes into the file.
pub(crate) enum Kind {
    Key(Arc<Packed>),
    Delta(String, Arc<Packed>),
}

impl Kind {
    fn size(&self) -> u64 {
        match self {
            Kind::Key(p) | Kind::Delta(_, p) => p.size,
        }
    }
}

/// Keyframe or delta for each of `kept` (oldest first): named versions, the first one and every
/// [`KEYFRAME_EVERY`]th (or [`LOG_BYTES`]th) get a full snapshot.
fn classify(kept: &[&Version]) -> Result<Vec<Kind>, FormatError> {
    let (mut since, mut bytes) = (0usize, 0u64);
    let mut out = Vec::with_capacity(kept.len());
    for (i, v) in kept.iter().enumerate() {
        if let Some(prev) = i.checked_sub(1).and_then(|p| kept.get(p))
            && v.auto
            && since < KEYFRAME_EVERY
            && bytes < LOG_BYTES
            && let Ok(p) = v.delta_against(prev)
        {
            since += 1;
            bytes += p.size;
            out.push(Kind::Delta(prev.id.clone(), p));
            continue;
        }
        out.push(Kind::Key(v.packed()?));
        (since, bytes) = (0, 0);
    }
    Ok(out)
}

/// [`fit_versions`] for the journal: the versions of `versions` that fit `limits`, each with how it
/// is written. Dropping a version re-plans, because the next one then needs a new base.
pub(crate) fn fit_journal<'a>(
    versions: &'a [Version],
    limits: VersionLimits,
    total: &mut u64,
    dropped: &mut Vec<&'a Version>,
) -> Result<Vec<(&'a Version, Kind)>, FormatError> {
    let mut ids = BTreeSet::new();
    for v in versions {
        if !is_valid_version_id(&v.id) || !ids.insert(v.id.as_str()) {
            return Err(FormatError::Io(format!("invalid or duplicate version id `{}`", v.id)));
        }
    }
    let mut kept: Vec<&Version> = versions.iter().collect();
    loop {
        let kinds = classify(&kept)?;
        if let Some(i) = kinds.iter().position(|k| matches!(k, Kind::Key(p) if p.size > limits.member)) {
            dropped.push(kept.remove(i));
            continue;
        }
        let sum: u64 = kinds.iter().map(Kind::size).sum();
        if sum > *total && !kept.is_empty() {
            let i = kept.iter().position(|v| v.auto).unwrap_or(0);
            dropped.push(kept.remove(i));
            continue;
        }
        *total = total.saturating_sub(sum);
        return Ok(kept.into_iter().zip(kinds).collect());
    }
}

/// Write the chosen versions: keyframes where older readers look for them (`versions/`), and,
/// when there are deltas, those under `history/` with the metadata that ties them together.
pub(crate) fn put_journal<W: Write + Seek>(z: &mut zip::ZipWriter<W>, entries: &[(&Version, Kind)]) -> Result<(), FormatError> {
    let keys: Vec<(&Version, Arc<Packed>)> = entries
        .iter()
        .filter_map(|(v, k)| if let Kind::Key(p) = k { Some((*v, p.clone())) } else { None })
        .collect();
    put_versions(z, "versions", &keys)?;
    if keys.len() == entries.len() {
        return Ok(());
    }
    let mut meta = Vec::with_capacity(entries.len());
    let mut parent: Option<&str> = None;
    for (v, k) in entries {
        let p = match k {
            Kind::Key(p) | Kind::Delta(_, p) => p,
        };
        let mut packed = zip::ZipArchive::new(std::io::Cursor::new(&p.zip[..])).map_err(io_err)?;
        let crc = packed.by_index(0).map_err(io_err)?.crc32();
        let images: Vec<&String> = v.images().keys().collect();
        let mut e = json!({"id": v.id, "name": v.name, "description": v.description, "time": v.time, "images": images,
            "parent": parent, "crc": crc, "size": p.size});
        if v.auto {
            e["auto"] = json!(true);
        }
        if let Kind::Delta(base, _) = k {
            e["kind"] = json!("delta");
            e["base"] = json!(base);
            z.raw_copy_file_rename(packed.by_index(0).map_err(io_err)?, format!("history/{}.delta", v.id)).map_err(io_err)?;
        } else {
            e["kind"] = json!("key");
        }
        meta.push(e);
        parent = Some(&v.id);
    }
    put(z, META, json_options(), &serde_json::to_vec(&json!({"format": 1, "versions": meta})).map_err(io_err)?)
}

/// The main version history from `history/meta.json`, or `None` when the file has none (or it is
/// unreadable: the keyframes under `versions/` are then read as usual). A version that is damaged
/// or whose base is gone is dropped, with a message, and so is every delta built on it.
pub(crate) fn read_journal<R: Read + Seek>(
    z: &mut zip::ZipArchive<R>,
    images: &BTreeMap<String, ImageData>,
    issues: &mut Vec<String>,
    budget: &mut u64,
    member: u64,
) -> Option<Vec<Version>> {
    z.index_for_name(META)?;
    let meta = match read_member(z, META, MAX_VERSION_INDEX).and_then(|b| serde_json::from_slice::<Value>(&b).map_err(|e| FormatError::Corrupt(e.to_string())))
    {
        Ok(v) => v,
        Err(e) => {
            issues.push(format!("the version journal was skipped, only full versions are shown: {e}"));
            return None;
        }
    };
    let entries = meta.get("versions").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let mut out: Vec<Version> = Vec::new();
    for e in entries {
        if out.len() >= MAX_VERSIONS {
            issues.push(format!("only the first {MAX_VERSIONS} versions were kept"));
            break;
        }
        let id = e.get("id").and_then(Value::as_str).unwrap_or_default();
        if !is_valid_version_id(id) || out.iter().any(|v| v.id == id) {
            issues.push("a version with an invalid id was skipped".into());
            continue;
        }
        let delta = e.get("kind").and_then(Value::as_str) == Some("delta");
        let name = if delta { format!("history/{id}.delta") } else { format!("versions/{id}.json") };
        let data = match read_member(z, &name, (*budget).min(member)) {
            Ok(d) => d,
            Err(err) => {
                issues.push(format!("version `{id}` was dropped: {err}"));
                continue;
            }
        };
        if let Some(crc) = e.get("crc").and_then(Value::as_u64)
            && u64::from(crc32(&data)) != crc
        {
            issues.push(format!("version `{id}` was dropped: its checksum doesn't match, the end of the history is damaged"));
            continue;
        }
        let base = if delta {
            let b = e.get("base").and_then(Value::as_str).and_then(|b| out.iter().find(|v| v.id == b));
            match b {
                Some(b) => Some(Box::new(b.clone())),
                None => {
                    issues.push(format!("version `{id}` was dropped: the version it builds on is missing"));
                    continue;
                }
            }
        } else {
            None
        };
        *budget = budget.saturating_sub(data.len() as u64);
        let text = |k: &str, max: usize| e.get(k).and_then(Value::as_str).map(|s| clip(s, max)).unwrap_or_default();
        let imgs: BTreeMap<String, ImageData> = e
            .get("images")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter_map(|h| images.get(h).map(|i| (h.to_string(), i.clone())))
            .collect();
        let json = Arc::new(data);
        out.push(Version {
            id: id.to_string(),
            name: text("name", MAX_VERSION_NAME),
            description: text("description", MAX_VERSION_DESCRIPTION),
            time: e.get("time").and_then(Value::as_u64).unwrap_or(0),
            auto: e.get("auto").and_then(Value::as_bool).unwrap_or(false),
            snapshot: match base {
                Some(base) => Snapshot::Delta { base, bytes: json, images: imgs },
                None => Snapshot::Stored { json, images: imgs },
            },
            packed: Arc::default(),
            delta: Arc::default(),
        });
    }
    Some(out)
}