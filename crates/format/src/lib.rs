//! The native `.figcraft` format.
//!
//! A zip archive holding `document.json` (the [`Document`] as JSON, with a `format`/`version`
//! header; layer fields at their default values are left out and read back as those defaults),
//! `images/<sha256>` for every image the document (or a saved version) uses, an optional
//! `thumbnail.png` and an optional version history: `versions/index.json` (`{"versions": [{id,
//! name, description, time, images}]}`, oldest first) plus one `versions/<id>.json` snapshot per
//! version (a `document.json` of its own). Branches are kept the same way: `branches/index.json`
//! (`{"branches": [{id, name, time, client, state, baseImages, headImages}]}`) plus
//! `branches/<id>/base.json` and `branches/<id>/head.json`; a branch's own version history is
//! kept like the file's, under `branches/<id>/versions/`. A published file also holds its
//! library (see [`library`]). Automatic versions may be stored as deltas under history/ (see
//! docs/version-journal.md; the full ones stay under ersions/). Readers that don't know about
//! versions, branches, the journal or libraries ignore those entries. Plain
//! JSON (the `document.json` alone) also opens. Reading is lenient: structural problems are
//! repaired (see [`Document::repair`]) and reported, damaged versions and libraries are skipped.
//!
//! [`rest`] imports the public REST API file JSON; [`fig`] imports `.fig` files (best effort).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod fig;
mod journal;
pub mod jpeg;
pub mod library;
pub mod penpot;
pub mod rest;
pub mod sketch;
pub mod source;
pub mod thumbnail;

pub use density::{pixel_density, placed_size};
pub use library::{is_valid_library_id, read_library, read_library_within};

mod density;

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
#[cfg(not(target_arch = "wasm32"))]
use std::io::{Seek, SeekFrom};
use std::sync::{Arc, OnceLock};

use figcraft_doc::{Document, ImageData};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use sha2::Digest;

pub use craft_figcraft_format::{
    BRANCH_INDEX, BranchEntry as NativeBranchEntry, BranchIndex as NativeBranchIndex, BranchState as NativeBranchState, DOCUMENT_RATIO, EXTENSION,
    FORMAT, FormatError, IMAGE_BUDGET, ImageBudget, MAX_BRANCHES, MAX_DOCUMENT, MAX_MEMBER, MAX_UNPACKED, MAX_VERSION_BYTES, MAX_VERSION_DESCRIPTION,
    MAX_VERSION_INDEX, MAX_VERSION_NAME, MAX_VERSIONS, ReadBudget, VERSION, VersionEntry as NativeVersionEntry, VersionIndex as NativeVersionIndex,
    clip, document_limit, is_valid_version_id,
};
#[cfg(test)]
const JSON_LEVEL: i64 = 3;
/// The export warning for colour calculations that resolve to nothing (an operand missing, or more
/// than 32 variables to read), shared by the Penpot and `.fig` exports.
pub(crate) const CALCULATIONS_LEFT_OUT: &str = "colour calculations that don't resolve were left out";
/// What one file's version snapshots may take, enforced when reading and respected when writing:
/// `member` per snapshot, `total` together. A snapshot may be as large as the whole budget on
/// 64-bit targets (the total already bounds memory), so one very large document's version reloads.
#[derive(Clone, Copy, Debug)]
pub(crate) struct VersionLimits {
    pub member: u64,
    pub total: u64,
}

impl VersionLimits {
    #[cfg(not(target_pointer_width = "32"))]
    pub(crate) const DEFAULT: Self = Self { member: MAX_VERSION_BYTES, total: MAX_VERSION_BYTES };
    #[cfg(target_pointer_width = "32")]
    pub(crate) const DEFAULT: Self = Self { member: MAX_MEMBER, total: MAX_VERSION_BYTES };
}

/// One entry of a file's version history: a named snapshot of the document.
#[derive(Clone, Debug)]
pub struct Version {
    /// Unique within the file; ASCII letters, digits, `-` and `_` (see [`is_valid_version_id`]).
    pub id: String,
    pub name: String,
    pub description: String,
    /// When it was saved (Unix seconds).
    pub time: u64,
    /// Saved automatically (a checkpoint) rather than by the user.
    pub auto: bool,
    snapshot: Snapshot,
    /// The snapshot as an already compressed archive member, made on first save and shared by
    /// clones: a snapshot never changes, so later saves copy it instead of encoding it again.
    packed: Arc<OnceLock<Arc<Packed>>>,
    /// The snapshot as a delta against the version .0 (see [journal]), made on first save.
    delta: Arc<OnceLock<(String, Arc<Packed>)>>,
}

/// A snapshot compressed once (zlib): the compressed bytes, the unpacked size and its CRC-32.
pub(crate) struct Packed {
    data: Vec<u8>,
    pub(crate) size: u64,
    pub(crate) crc: u32,
}

impl Packed {
    /// Compress `bytes`.
    pub(crate) fn pack(bytes: &[u8]) -> Result<Self, FormatError> {
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(3));
        enc.write_all(bytes).map_err(io_err)?;
        Ok(Self { data: enc.finish().map_err(io_err)?, size: bytes.len() as u64, crc: journal::crc32(bytes) })
    }

    /// The snapshot's bytes.
    pub(crate) fn unpack(&self) -> Result<Vec<u8>, FormatError> {
        let mut out = Vec::with_capacity(usize::try_from(self.size).unwrap_or(0));
        flate2::read::ZlibDecoder::new(&self.data[..]).read_to_end(&mut out).map_err(io_err)?;
        Ok(out)
    }
}

impl std::fmt::Debug for Packed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Packed({} bytes, {} unpacked)", self.data.len(), self.size)
    }
}

#[derive(Clone, Debug)]
enum Snapshot {
    /// Saved in this session (or already read).
    Doc(Arc<Document>),
    /// As read from a file: its `document.json`-style JSON, parsed on first use, and the images
    /// it uses.
    Stored { json: Arc<Vec<u8>>, images: BTreeMap<String, ImageData> },
    /// As read from a file: a delta (see [journal]) on the version before it.
    Delta { base: Box<Version>, bytes: Arc<Vec<u8>>, images: BTreeMap<String, ImageData> },
}

impl Version {
    /// A version of `doc`. The name and description are clipped to their maximum lengths.
    pub fn new(id: impl Into<String>, name: &str, description: &str, time: u64, doc: Arc<Document>) -> Self {
        Self {
            id: id.into(),
            name: clip(name, MAX_VERSION_NAME),
            description: clip(description, MAX_VERSION_DESCRIPTION),
            time,
            auto: false,
            snapshot: Snapshot::Doc(doc),
            packed: Arc::default(),
            delta: Arc::default(),
        }
    }

    /// The snapshot compressed (once; later calls and clones reuse it).
    fn packed(&self) -> Result<Arc<Packed>, FormatError> {
        if let Some(p) = self.packed.get() {
            return Ok(p.clone());
        }
        let mut bytes = Vec::new();
        match &self.snapshot {
            Snapshot::Doc(d) => write_json(d, &mut bytes)?,
            Snapshot::Stored { json, .. } => bytes.extend_from_slice(json),
            Snapshot::Delta { .. } => write_json(&*self.document()?, &mut bytes)?,
        }
        let packed = Arc::new(Packed::pack(&bytes)?);
        Ok(self.packed.get_or_init(|| packed).clone())
    }

    /// The snapshot (with its images). Versions read from a file are parsed here, so a damaged
    /// one fails now rather than when the file opens.
    pub fn document(&self) -> Result<Arc<Document>, FormatError> {
        match &self.snapshot {
            Snapshot::Doc(d) => Ok(d.clone()),
            Snapshot::Stored { json, images } => {
                let text = std::str::from_utf8(json).map_err(|e| FormatError::Corrupt(e.to_string()))?;
                let (mut doc, _) = from_json(text)?;
                doc.images = images.clone();
                Ok(Arc::new(doc))
            }
            Snapshot::Delta { base, bytes, images } => {
                let mut doc = journal::apply_delta(&*base.document()?, bytes)?;
                doc.images = images.clone();
                Ok(Arc::new(doc))
            }
        }
    }

    /// The images the snapshot uses, by hash.
    pub fn images(&self) -> &BTreeMap<String, ImageData> {
        match &self.snapshot {
            Snapshot::Doc(d) => &d.images,
            Snapshot::Stored { images, .. } | Snapshot::Delta { images, .. } => images,
        }
    }
}

/// Where a branch is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BranchState {
    Active,
    /// Merged into the main file (and archived).
    Merged,
    /// Archived without merging.
    Archived,
}

/// A branch of the file: a copy of the document to work on separately, and the main document as
/// it was when the branch was made or last updated from it (its base).
#[derive(Clone, Debug)]
pub struct Branch {
    /// Unique among the file's branches; a valid version id ([`is_valid_version_id`]).
    pub id: String,
    pub name: String,
    /// When it was made (Unix seconds).
    pub time: u64,
    /// The client id space the branch makes ids in ([`Document::set_client_id`]), so they never
    /// clash with ids made on the main file or on other branches.
    pub client: u32,
    pub state: BranchState,
    pub base: Arc<Document>,
    pub head: Arc<Document>,
    /// The branch's own version history, oldest first.
    pub versions: Vec<Version>,
}

/// A file as read: the document, the repairs made (and versions skipped), and its version
/// history, oldest first.
#[derive(Debug)]
pub struct Loaded {
    pub doc: Document,
    pub issues: Vec<String>,
    pub versions: Vec<Version>,
    pub branches: Vec<Branch>,
    pub images_past_budget: usize,
}

impl From<Document> for Loaded {
    fn from(doc: Document) -> Self {
        Self { doc, issues: Vec::new(), versions: Vec::new(), branches: Vec::new(), images_past_budget: 0 }
    }
}

/// Content hash used as an image's key.
pub fn image_hash(bytes: &[u8]) -> String {
    let d = sha2::Sha256::digest(bytes);
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// Stored pixel dimensions of encoded image `bytes` (without EXIF orientation swap).
fn stored_dimensions(bytes: &[u8]) -> Result<(u32, u32), craft_image_codecs::CodecError> {
    match craft_image_codecs::probe_stored_size(bytes, &craft_image_codecs::Limits::default()) {
        Ok(dims) => Ok(dims),
        Err(craft_image_codecs::CodecError::Unsupported { .. }) => craft_image_codecs::probe_allocation_size(bytes),
        Err(e) => Err(e),
    }
}

/// Register encoded image bytes with the document; returns the hash and pixel size.
pub fn add_image(doc: &mut Document, bytes: Vec<u8>) -> Result<(String, u32, u32), FormatError> {
    let (w, h) = stored_dimensions(&bytes).map_err(|e| FormatError::Corrupt(format!("unsupported image: {e}")))?;
    let hash = image_hash(&bytes);
    doc.images.insert(hash.clone(), ImageData { bytes: Arc::new(bytes), width: w, height: h });
    Ok((hash, w, h))
}

/// Image bytes with their pixel size (0×0 when they can't be decoded).
pub(crate) fn image_data(data: Vec<u8>) -> ImageData {
    let (w, h) = stored_dimensions(&data).unwrap_or((0, 0));
    ImageData { bytes: Arc::new(data), width: w, height: h }
}

/// The image hashes a `document.json` lists in its `imageIndex` (`None` without one).
type ImageIndex = Option<BTreeSet<String>>;

fn native_image_index(doc: &Document) -> Vec<craft_figcraft_format::ImageIndexEntry> {
    doc.images
        .iter()
        .map(|(hash, image)| craft_figcraft_format::ImageIndexEntry { hash: hash.clone(), width: image.width, height: image.height })
        .collect()
}

/// A serializer adapter that writes `0` in place of every NaN or infinite float. JSON has no such
/// numbers (`serde_json` writes `null`, which the reader rejects), so one bad value must not stop
/// Save, autosave or recovery copies of the whole document. Everything else passes through.
struct Finite<S>(S);

/// Serializes a value through a [`Finite`] adapter around the serializer it is written with.
struct FiniteValue<'a, T: ?Sized>(&'a T);

impl<T: ?Sized + Serialize> Serialize for FiniteValue<'_, T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(Finite(s))
    }
}

macro_rules! finite_pass {
    ($($name:ident($ty:ty)),* $(,)?) => {
        $(fn $name(self, v: $ty) -> Result<S::Ok, S::Error> { self.0.$name(v) })*
    };
}

impl<S: Serializer> Serializer for Finite<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    type SerializeSeq = Finite<S::SerializeSeq>;
    type SerializeTuple = Finite<S::SerializeTuple>;
    type SerializeTupleStruct = Finite<S::SerializeTupleStruct>;
    type SerializeTupleVariant = Finite<S::SerializeTupleVariant>;
    type SerializeMap = Finite<S::SerializeMap>;
    type SerializeStruct = Finite<S::SerializeStruct>;
    type SerializeStructVariant = Finite<S::SerializeStructVariant>;

    finite_pass!(
        serialize_bool(bool),
        serialize_i8(i8),
        serialize_i16(i16),
        serialize_i32(i32),
        serialize_i64(i64),
        serialize_u8(u8),
        serialize_u16(u16),
        serialize_u32(u32),
        serialize_u64(u64),
        serialize_i128(i128),
        serialize_u128(u128),
        serialize_char(char),
        serialize_str(&str),
        serialize_bytes(&[u8])
    );

    fn serialize_f32(self, v: f32) -> Result<S::Ok, S::Error> {
        self.0.serialize_f32(if v.is_finite() { v } else { 0.0 })
    }
    fn serialize_f64(self, v: f64) -> Result<S::Ok, S::Error> {
        self.0.serialize_f64(if v.is_finite() { v } else { 0.0 })
    }
    fn serialize_none(self) -> Result<S::Ok, S::Error> {
        self.0.serialize_none()
    }
    fn serialize_some<T: ?Sized + Serialize>(self, v: &T) -> Result<S::Ok, S::Error> {
        self.0.serialize_some(&FiniteValue(v))
    }
    fn serialize_unit(self) -> Result<S::Ok, S::Error> {
        self.0.serialize_unit()
    }
    fn serialize_unit_struct(self, name: &'static str) -> Result<S::Ok, S::Error> {
        self.0.serialize_unit_struct(name)
    }
    fn serialize_unit_variant(self, name: &'static str, idx: u32, variant: &'static str) -> Result<S::Ok, S::Error> {
        self.0.serialize_unit_variant(name, idx, variant)
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(self, name: &'static str, v: &T) -> Result<S::Ok, S::Error> {
        self.0.serialize_newtype_struct(name, &FiniteValue(v))
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(self, name: &'static str, idx: u32, variant: &'static str, v: &T) -> Result<S::Ok, S::Error> {
        self.0.serialize_newtype_variant(name, idx, variant, &FiniteValue(v))
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<Self::SerializeSeq, S::Error> {
        self.0.serialize_seq(len).map(Finite)
    }
    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple, S::Error> {
        self.0.serialize_tuple(len).map(Finite)
    }
    fn serialize_tuple_struct(self, name: &'static str, len: usize) -> Result<Self::SerializeTupleStruct, S::Error> {
        self.0.serialize_tuple_struct(name, len).map(Finite)
    }
    fn serialize_tuple_variant(
        self,
        name: &'static str,
        idx: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleVariant, S::Error> {
        self.0.serialize_tuple_variant(name, idx, variant, len).map(Finite)
    }
    fn serialize_map(self, len: Option<usize>) -> Result<Self::SerializeMap, S::Error> {
        self.0.serialize_map(len).map(Finite)
    }
    fn serialize_struct(self, name: &'static str, len: usize) -> Result<Self::SerializeStruct, S::Error> {
        self.0.serialize_struct(name, len).map(Finite)
    }
    fn serialize_struct_variant(
        self,
        name: &'static str,
        idx: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStructVariant, S::Error> {
        self.0.serialize_struct_variant(name, idx, variant, len).map(Finite)
    }
    fn is_human_readable(&self) -> bool {
        self.0.is_human_readable()
    }
}

macro_rules! finite_elements {
    ($($trait:ident::$method:ident),* $(,)?) => {
        $(impl<S: serde::ser::$trait> serde::ser::$trait for Finite<S> {
            type Ok = S::Ok;
            type Error = S::Error;
            fn $method<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), S::Error> {
                self.0.$method(&FiniteValue(v))
            }
            fn end(self) -> Result<S::Ok, S::Error> {
                self.0.end()
            }
        })*
    };
}

finite_elements!(
    SerializeSeq::serialize_element,
    SerializeTuple::serialize_element,
    SerializeTupleStruct::serialize_field,
    SerializeTupleVariant::serialize_field
);

impl<S: serde::ser::SerializeMap> serde::ser::SerializeMap for Finite<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    fn serialize_key<T: ?Sized + Serialize>(&mut self, k: &T) -> Result<(), S::Error> {
        self.0.serialize_key(&FiniteValue(k))
    }
    fn serialize_value<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), S::Error> {
        self.0.serialize_value(&FiniteValue(v))
    }
    fn end(self) -> Result<S::Ok, S::Error> {
        self.0.end()
    }
}

impl<S: serde::ser::SerializeStruct> serde::ser::SerializeStruct for Finite<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, key: &'static str, v: &T) -> Result<(), S::Error> {
        self.0.serialize_field(key, &FiniteValue(v))
    }
    fn skip_field(&mut self, key: &'static str) -> Result<(), S::Error> {
        self.0.skip_field(key)
    }
    fn end(self) -> Result<S::Ok, S::Error> {
        self.0.end()
    }
}

impl<S: serde::ser::SerializeStructVariant> serde::ser::SerializeStructVariant for Finite<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, key: &'static str, v: &T) -> Result<(), S::Error> {
        self.0.serialize_field(key, &FiniteValue(v))
    }
    fn skip_field(&mut self, key: &'static str) -> Result<(), S::Error> {
        self.0.skip_field(key)
    }
    fn end(self) -> Result<S::Ok, S::Error> {
        self.0.end()
    }
}

/// Serialize the document (without images) to the JSON value stored in `document.json`.
/// NaN and infinite numbers are written as `0`.
pub fn to_json(doc: &Document) -> Result<Value, FormatError> {
    {
        let mut bytes = Vec::new();
        craft_figcraft_format::write_document_json(&FiniteValue(doc), &native_image_index(doc), &mut bytes).map_err(io_err)?;
        serde_json::from_slice(&bytes).map_err(io_err)
    }
}

/// Write the document (without images) as `document.json` content to `w`.
/// NaN and infinite numbers are written as `0`.
pub fn write_json(doc: &Document, w: impl Write) -> Result<(), FormatError> {
    craft_figcraft_format::write_document_json(&FiniteValue(doc), &native_image_index(doc), w).map_err(io_err)
}

/// Parse `document.json` content.
pub fn from_json(text: &str) -> Result<(Document, Vec<String>), FormatError> {
    parse_json(text).map(|(doc, issues, _)| (doc, issues))
}

/// Parse `document.json` content; also returns its [`ImageIndex`]. The header is checked first
/// (a newer file is refused before its document is read), then the document is deserialized
/// straight from the text.
fn parse_json(text: &str) -> Result<(Document, Vec<String>, ImageIndex), FormatError> {
    let (mut doc, header) = craft_figcraft_format::read_document_json::<Document>(text).map_err(|e| match e {
        craft_figcraft_format::ReadDocumentError::Json(error) => FormatError::Corrupt(error.to_string()),
        craft_figcraft_format::ReadDocumentError::NewerVersion(error) => FormatError::Newer(error.0),
    })?;
    let images = header.images.map(|hashes| hashes.into_iter().collect());
    let issues = doc.validate();
    if !issues.is_empty() {
        doc.repair();
    }
    Ok((doc, issues, images))
}

pub(crate) fn io_err(e: impl std::fmt::Display) -> FormatError {
    FormatError::Io(e.to_string())
}

/// JSON compression for legacy Penpot test fixtures.
#[cfg(test)]
pub(crate) fn json_options() -> zip::write::SimpleFileOptions {
    zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).compression_level(Some(JSON_LEVEL))
}

/// Add one member to the native archive.
pub(crate) fn put(
    z: &mut craft_figcraft_format::ArchiveWriter,
    name: &str,
    compression: craft_figcraft_format::Compression,
    bytes: &[u8],
) -> Result<(), FormatError> {
    z.add(name, bytes, compression).map_err(io_err)
}

/// `files` (name, bytes; a `/` in a name makes a folder) as one zip archive: several exports
/// delivered as a single download (#969).
pub fn zip_files<'a>(files: impl IntoIterator<Item = (&'a str, &'a [u8])>) -> Result<Vec<u8>, FormatError> {
    craft_figcraft_format::write_archive(files.into_iter().map(|(name, bytes)| craft_figcraft_format::Member {
        name,
        bytes,
        compression: craft_figcraft_format::Compression::Deflated,
    }))
    .map_err(io_err)
}

/// Add `doc` as a `document.json`-style member, serialized straight into the compressor.
fn put_json(z: &mut craft_figcraft_format::ArchiveWriter, name: &str, doc: &Document) -> Result<(), FormatError> {
    z.add_document_json(name, &FiniteValue(doc), &native_image_index(doc)).map_err(io_err)
}

/// Write a `.figcraft` archive to bytes.
pub fn save_bytes(doc: &Document, thumbnail_png: Option<&[u8]>) -> Result<Vec<u8>, FormatError> {
    save_bytes_with_versions(doc, thumbnail_png, &[])
}

/// Write a `.figcraft` archive with a version history (oldest first) to bytes.
pub fn save_bytes_with_versions(doc: &Document, thumbnail_png: Option<&[u8]>, versions: &[Version]) -> Result<Vec<u8>, FormatError> {
    save_bytes_with_history(doc, thumbnail_png, versions, &[])
}

/// Write a `.figcraft` archive with a version history (oldest first) and branches to bytes.
pub fn save_bytes_with_history(
    doc: &Document,
    thumbnail_png: Option<&[u8]>,
    versions: &[Version],
    branches: &[Branch],
) -> Result<Vec<u8>, FormatError> {
    save_bytes_reporting(doc, thumbnail_png, versions, branches).map(|(bytes, _)| bytes)
}

/// [`save_bytes_with_history`], and what it left out so the file can be read back: a snapshot past
/// what a reader accepts (see [`VersionLimits`]) is not written, oldest checkpoints first and named
/// versions only when nothing else helps. The notes say which, for the user.
pub fn save_bytes_reporting(
    doc: &Document,
    thumbnail_png: Option<&[u8]>,
    versions: &[Version],
    branches: &[Branch],
) -> Result<(Vec<u8>, Vec<String>), FormatError> {
    save_inner(doc, thumbnail_png, versions, branches, VersionLimits::DEFAULT, true)
}

/// Choose the versions of `versions` (oldest first) that fit `limits`, with their compressed
/// snapshots. `total` is what is left of the budget; `dropped` collects the ones left out.
fn fit_versions<'a>(
    versions: &'a [Version],
    limits: VersionLimits,
    total: &mut u64,
    dropped: &mut Vec<&'a Version>,
) -> Result<Vec<(&'a Version, Arc<Packed>)>, FormatError> {
    let mut ids = BTreeSet::new();
    for v in versions {
        if !is_valid_version_id(&v.id) || !ids.insert(v.id.as_str()) {
            return Err(FormatError::Io(format!("invalid or duplicate version id `{}`", v.id)));
        }
    }
    let mut kept = Vec::with_capacity(versions.len());
    for v in versions {
        let p = v.packed()?;
        if p.size > limits.member { dropped.push(v) } else { kept.push((v, p)) }
    }
    let mut sum: u64 = kept.iter().map(|(_, p)| p.size).sum();
    while sum > *total {
        let i = kept.iter().position(|(v, _)| v.auto).unwrap_or(0);
        if i >= kept.len() {
            break;
        }
        let (v, p) = kept.remove(i);
        sum -= p.size;
        dropped.push(v);
    }
    *total = total.saturating_sub(sum);
    Ok(kept)
}

/// A sink that only counts what is written to it.
struct CountBytes(u64);

impl Write for CountBytes {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0 = self.0.saturating_add(buf.len() as u64);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn dropped_note(dropped: &[&Version], branch: Option<&str>) -> Option<String> {
    if dropped.is_empty() {
        return None;
    }
    let named: Vec<String> = dropped.iter().filter(|v| !v.auto).map(|v| format!("\"{}\"", v.name)).collect();
    let autos = dropped.len() - named.len();
    let mut what = Vec::new();
    if autos > 0 {
        what.push(format!("{autos} automatic version{}", if autos == 1 { "" } else { "s" }));
    }
    if !named.is_empty() {
        what.push(format!("the named version{} {}", if named.len() == 1 { "" } else { "s" }, named.join(", ")));
    }
    let place = branch.map(|b| format!(" of branch {b}")).unwrap_or_default();
    Some(format!("Saved without {}{place}: the file's version history is too large to reopen otherwise", what.join(" and ")))
}

/// [save_inner] with every version written as a full snapshot.
#[cfg(test)]
pub(crate) fn save_within(
    doc: &Document,
    thumbnail_png: Option<&[u8]>,
    versions: &[Version],
    branches: &[Branch],
    limits: VersionLimits,
) -> Result<(Vec<u8>, Vec<String>), FormatError> {
    save_inner(doc, thumbnail_png, versions, branches, limits, false)
}

/// Write the archive; journal: automatic versions as deltas (see [journal]).
fn save_inner(
    doc: &Document,
    thumbnail_png: Option<&[u8]>,
    versions: &[Version],
    branches: &[Branch],
    limits: VersionLimits,
    journal: bool,
) -> Result<(Vec<u8>, Vec<String>), FormatError> {
    let mut notes = Vec::new();
    // Chosen before anything is written; the images of versions left out are left out too.
    let mut total = limits.total;
    let mut dropped = Vec::new();
    let kept: Vec<(&Version, journal::Kind)> = if journal {
        journal::fit_journal(versions, limits, &mut total, &mut dropped)?
    } else {
        fit_versions(versions, limits, &mut total, &mut dropped)?.into_iter().map(|(v, p)| (v, journal::Kind::Key(p))).collect()
    };
    notes.extend(dropped_note(&dropped, None));
    let mut branch_total = limits.total;
    let mut kept_branches = Vec::with_capacity(branches.len());
    for b in branches {
        // The reader draws a branch's base and head from the same budget as its versions.
        for part in [&b.base, &b.head] {
            let mut n = CountBytes(0);
            write_json(part, &mut n)?;
            branch_total = branch_total.saturating_sub(n.0);
        }
        let mut dropped = Vec::new();
        let k = fit_versions(&b.versions, limits, &mut branch_total, &mut dropped)?;
        notes.extend(dropped_note(&dropped, Some(&b.name)));
        kept_branches.push(k);
    }
    let mut z = craft_figcraft_format::ArchiveWriter::new();
    let stored = craft_figcraft_format::Compression::Stored;
    put_json(&mut z, "document.json", doc)?;
    // Every image once, whether the document, a version or a branch uses it.
    let mut written = BTreeSet::new();
    let branch_images =
        branches.iter().zip(&kept_branches).flat_map(|(b, k)| [&b.base.images, &b.head.images].into_iter().chain(k.iter().map(|(v, _)| v.images())));
    for (hash, img) in std::iter::once(&doc.images).chain(kept.iter().map(|(v, _)| v.images())).chain(branch_images).flatten() {
        if written.insert(hash) {
            put(&mut z, &format!("images/{hash}"), stored, img.bytes.as_slice())?;
        }
    }
    if let Some(t) = thumbnail_png {
        put(&mut z, "thumbnail.png", stored, t)?;
    }
    journal::put_journal(&mut z, &kept)?;
    if !branches.is_empty() {
        let mut ids = BTreeSet::new();
        let mut index = Vec::with_capacity(branches.len());
        for (b, k) in branches.iter().zip(&kept_branches) {
            if !is_valid_version_id(&b.id) || !ids.insert(b.id.as_str()) {
                return Err(FormatError::Io(format!("invalid or duplicate branch id `{}`", b.id)));
            }
            put_json(&mut z, &format!("branches/{}/base.json", b.id), &b.base)?;
            put_json(&mut z, &format!("branches/{}/head.json", b.id), &b.head)?;
            put_versions(&mut z, &format!("branches/{}/versions", b.id), k)?;
            let images = |d: &Document| d.images.keys().cloned().collect::<Vec<String>>();
            index.push(NativeBranchEntry {
                id: b.id.clone(),
                name: b.name.clone(),
                time: b.time,
                client: b.client,
                state: match b.state {
                    BranchState::Active => NativeBranchState::Active,
                    BranchState::Merged => NativeBranchState::Merged,
                    BranchState::Archived => NativeBranchState::Archived,
                },
                base_images: images(&b.base),
                head_images: images(&b.head),
            });
        }
        put(
            &mut z,
            BRANCH_INDEX,
            craft_figcraft_format::Compression::Level(3),
            &craft_figcraft_format::write_branch_index(&NativeBranchIndex { branches: index }).map_err(io_err)?,
        )?;
    }
    if let Some(lib) = &doc.published {
        library::write(&mut z, lib, &written)?;
    }
    z.finish().map_err(io_err).map(|bytes| (bytes, notes))
}

/// Write the chosen version history (oldest first) under `dir`: `<dir>/index.json` and one
/// `<dir>/<id>.json` snapshot per version, taken from the version's cache. Nothing when there are
/// none.
fn put_versions(z: &mut craft_figcraft_format::ArchiveWriter, dir: &str, versions: &[(&Version, Arc<Packed>)]) -> Result<(), FormatError> {
    if versions.is_empty() {
        return Ok(());
    }
    let mut index = Vec::with_capacity(versions.len());
    for (v, p) in versions {
        let name = format!("{dir}/{}.json", v.id);
        put(z, &name, craft_figcraft_format::Compression::Level(3), &p.unpack()?)?;
        index.push(NativeVersionEntry {
            id: v.id.clone(),
            name: v.name.clone(),
            description: v.description.clone(),
            time: v.time,
            auto: v.auto,
            images: v.images().keys().cloned().collect(),
        });
    }
    put(
        z,
        &format!("{dir}/index.json"),
        craft_figcraft_format::Compression::Level(3),
        &craft_figcraft_format::write_version_index(&NativeVersionIndex { versions: index }).map_err(io_err)?,
    )?;
    Ok(())
}

/// Read a `.figcraft` archive (or bare JSON) from bytes. Returns the document and any repairs made.
pub fn load_bytes(bytes: &[u8]) -> Result<(Document, Vec<String>), FormatError> {
    load_bytes_with_versions(bytes).map(|l| (l.doc, l.issues))
}

/// Read a member from a foreign ZIP archive, refusing entries larger than `limit`.
pub(crate) fn read_member<R: Read + std::io::Seek>(z: &mut zip::ZipArchive<R>, name: &str, limit: u64) -> Result<Vec<u8>, FormatError> {
    let f = z.by_name(name).map_err(|e| FormatError::Corrupt(format!("{name}: {e}")))?;
    if f.size() > limit {
        return Err(FormatError::Corrupt(format!("{name} is too large")));
    }
    let mut data = Vec::new();
    f.take(limit.saturating_add(1)).read_to_end(&mut data).map_err(|e| FormatError::Corrupt(format!("{name}: {e}")))?;
    if data.len() as u64 > limit {
        return Err(FormatError::Corrupt(format!("{name} is too large")));
    }
    Ok(data)
}

/// The remaining archive-wide expansion budget while the app maps native records.
pub(crate) type Unpacked = ReadBudget;

/// Read a `.figcraft` archive (or bare JSON) with its version history.
pub fn load_bytes_with_versions(bytes: &[u8]) -> Result<Loaded, FormatError> {
    load_within(bytes, IMAGE_BUDGET)
}

/// [`load_bytes_with_versions`] reading at most `budget` of images.
pub(crate) fn load_within(bytes: &[u8], budget: ImageBudget) -> Result<Loaded, FormatError> {
    load_core(bytes, budget, MAX_UNPACKED, VersionLimits::DEFAULT)
}

/// [`load_within`] unpacking at most `total` bytes from the archive, every area together.
#[cfg(test)]
pub(crate) fn load_limited(bytes: &[u8], budget: ImageBudget, total: u64) -> Result<Loaded, FormatError> {
    load_core(bytes, budget, total, VersionLimits::DEFAULT)
}

/// [`load_within`] with the version history's own limits.
#[cfg(test)]
pub(crate) fn load_within_limits(bytes: &[u8], budget: ImageBudget, limits: VersionLimits) -> Result<Loaded, FormatError> {
    load_core(bytes, budget, MAX_UNPACKED, limits)
}

fn load_core(bytes: &[u8], budget: ImageBudget, total: u64, limits: VersionLimits) -> Result<Loaded, FormatError> {
    let trimmed = bytes.iter().position(|b| !b.is_ascii_whitespace()).and_then(|i| bytes.get(i));
    if trimmed == Some(&b'{') {
        let text = std::str::from_utf8(bytes).map_err(|e| FormatError::Corrupt(e.to_string()))?;
        let (doc, issues) = from_json(text)?;
        return Ok(Loaded { issues, ..Loaded::from(doc) });
    }
    let mut z =
        craft_figcraft_format::Archive::open(bytes, craft_figcraft_format::ArchiveLimits { max_entries: u64::MAX, max_total_bytes: u64::MAX })
            .map_err(|e| FormatError::Corrupt(e.to_string()))?;
    load_archive(&mut z, budget, total, bytes.len() as u64, limits)
}

fn load_archive(
    z: &mut impl craft_figcraft_format::NativeArchive,
    budget: ImageBudget,
    total: u64,
    archive_len: u64,
    limits: VersionLimits,
) -> Result<Loaded, FormatError> {
    let packed = z.compressed_size("document.json").unwrap_or(0);
    let limit = document_limit(packed, archive_len);
    let mut unpacked = Unpacked::new(total);
    let text = String::from_utf8(unpacked.read(z, "document.json", limit)?).map_err(|e| FormatError::Corrupt(e.to_string()))?;
    let (mut doc, mut issues, own_images) = parse_json(&text)?;
    let ImagesRead { images, unread, past_budget } = read_images(z, own_images.as_ref(), budget, &mut unpacked);
    report_lost_images(&mut issues, &unread, &past_budget, own_images.as_ref(), &images);
    let images_past_budget = past_budget.len();
    match library::read(z, &images, &mut unpacked) {
        Ok(lib) => doc.published = lib.map(Arc::new),
        Err(e) => issues.push(format!("the published library was skipped: {e}")),
    }
    let mut budget = limits.total;
    let versions = match journal::read_journal(z, &images, &mut issues, &mut budget, limits.member, &mut unpacked) {
        Some(v) => v,
        None => read_versions(z, "versions", &images, &mut issues, &mut budget, limits.member, &mut unpacked),
    };
    let branches = read_branches(z, &images, &mut issues, limits, &mut unpacked);
    // Images only the version history or branches use stay with them.
    let branch_images =
        branches.iter().flat_map(|b| b.base.images.keys().chain(b.head.images.keys()).chain(b.versions.iter().flat_map(|v| v.images().keys())));
    let history_only: BTreeSet<&String> = versions
        .iter()
        .flat_map(|v| v.images().keys())
        .chain(branch_images)
        .filter(|h| own_images.as_ref().is_some_and(|o| !o.contains(*h)))
        .collect();
    doc.images = images.into_iter().filter(|(h, _)| !history_only.contains(h)).collect();
    Ok(Loaded { doc, issues, versions, branches, images_past_budget })
}

/// The archive's `images/` members by hash, within `budget`, and the ones that couldn't be read
/// with why. The document's own images (`own`) are read first, so a file past the budget loses
/// the images only its history uses before its own.
fn read_images(
    z: &mut impl craft_figcraft_format::NativeArchive,
    own: Option<&BTreeSet<String>>,
    budget: ImageBudget,
    unpacked: &mut Unpacked,
) -> ImagesRead {
    let mut hashes: Vec<String> =
        z.names().into_iter().filter_map(|n| n.strip_prefix("images/").map(str::to_owned)).filter(|h| !h.is_empty()).collect();
    hashes.sort_by_key(|h| !own.is_some_and(|o| o.contains(h)));
    let mut images = BTreeMap::new();
    let mut unread = Vec::new();
    // Member names that can't be real image keys (`../x`, `a/b`) are never read, so never written back.
    hashes.retain(|h| {
        let ok = ImageData::is_valid_key(h);
        if !ok {
            unread.push((h.clone(), FormatError::Corrupt("its name isn't a valid image hash".to_string())));
        }
        ok
    });
    let mut left = budget.bytes;
    let mut past_budget = BTreeSet::new();
    for (i, hash) in hashes.into_iter().enumerate() {
        let name = format!("images/{hash}");
        if i >= budget.images {
            past_budget.insert(hash.clone());
            unread.push((hash, FormatError::Corrupt(format!("the file has more than {} images", budget.images))));
            continue;
        }
        if z.declared_size(&name).is_some_and(|size| size > left) {
            let mb = budget.bytes / (1024 * 1024);
            past_budget.insert(hash.clone());
            unread.push((hash, FormatError::Corrupt(format!("the file's images together pass {mb} MB"))));
            continue;
        }
        // Past the file's total, an image is left out as one past the images' own budget: it stays
        // in the file, which can't then be saved over without losing it (#1313).
        if unpacked.exceeds_remaining(z, &name) {
            past_budget.insert(hash.clone());
            unread.push((hash, FormatError::Corrupt(unpacked.exhausted_message())));
            continue;
        }
        match unpacked.read(z, &name, left.min(MAX_MEMBER)) {
            Ok(data) => {
                left = left.saturating_sub(data.len() as u64);
                images.insert(hash, image_data(data));
            }
            Err(e) => unread.push((hash, e)),
        }
    }
    ImagesRead { images, unread, past_budget }
}

struct ImagesRead {
    images: BTreeMap<String, ImageData>,
    unread: Vec<(String, FormatError)>,
    past_budget: BTreeSet<String>,
}

/// Most lost images reported one by one (the rest as a count).
const MAX_IMAGE_ISSUES: usize = 20;

fn report_lost_images(
    issues: &mut Vec<String>,
    unread: &[(String, FormatError)],
    past_budget: &BTreeSet<String>,
    listed: Option<&BTreeSet<String>>,
    images: &BTreeMap<String, ImageData>,
) {
    let mut lost: Vec<String> = unread
        .iter()
        .map(|(hash, e)| {
            let why = match e {
                FormatError::Corrupt(m) | FormatError::Io(m) => m.clone(),
                FormatError::Newer(v) => format!("format version {v}"),
            };
            if past_budget.contains(hash) {
                format!("image {hash} couldn't be read ({why}): it is left out and stays in the file, which can't be saved over without losing it")
            } else {
                format!("image {hash} couldn't be read ({why}): it is left out, and saving this file again drops it")
            }
        })
        .collect();
    let unread_hashes: BTreeSet<&str> = unread.iter().map(|(h, _)| h.as_str()).collect();
    for hash in listed.into_iter().flatten() {
        if !images.contains_key(hash) && !unread_hashes.contains(hash.as_str()) {
            lost.push(format!("image {hash} is used by the file but its data is missing: it is left out, and saving this file again drops it"));
        }
    }
    let more = lost.len().saturating_sub(MAX_IMAGE_ISSUES);
    lost.truncate(MAX_IMAGE_ISSUES);
    issues.extend(lost);
    if more > 0 {
        issues.push(format!("{more} more images couldn't be loaded"));
    }
}

/// The version history kept under `dir` in an archive (none when it has no index), reading at
/// most `budget` bytes of snapshots. Damaged entries are skipped and reported in `issues`; nothing
/// here fails the load.
fn read_versions(
    z: &mut impl craft_figcraft_format::NativeArchive,
    dir: &str,
    images: &BTreeMap<String, ImageData>,
    issues: &mut Vec<String>,
    budget: &mut u64,
    member: u64,
    unpacked: &mut Unpacked,
) -> Vec<Version> {
    let index_name = format!("{dir}/index.json");
    if !z.contains(&index_name) {
        return Vec::new();
    }
    let index = match unpacked
        .read(z, &index_name, MAX_VERSION_INDEX)
        .and_then(|b| craft_figcraft_format::read_version_index(&b).map_err(|e| FormatError::Corrupt(e.to_string())))
    {
        Ok(v) => v,
        Err(e) => {
            issues.push(format!("the version history was skipped: {e}"));
            return Vec::new();
        }
    };
    let mut out: Vec<Version> = Vec::new();
    for e in index.versions {
        if out.len() >= MAX_VERSIONS {
            issues.push(format!("only the first {MAX_VERSIONS} versions were kept"));
            break;
        }
        let id = e.id.as_str();
        if !is_valid_version_id(id) || out.iter().any(|v| v.id == id) {
            issues.push("a version with an invalid id was skipped".into());
            continue;
        }
        let json = match unpacked.read(z, &format!("{dir}/{id}.json"), (*budget).min(member)) {
            Ok(j) => j,
            Err(err) => {
                issues.push(format!("version `{id}` was skipped: {err}"));
                continue;
            }
        };
        *budget = budget.saturating_sub(json.len() as u64);
        let images = e.images.iter().filter_map(|h| images.get(h).map(|i| (h.clone(), i.clone()))).collect();
        out.push(Version {
            id: id.to_string(),
            name: clip(&e.name, MAX_VERSION_NAME),
            description: clip(&e.description, MAX_VERSION_DESCRIPTION),
            time: e.time,
            auto: e.auto,
            snapshot: Snapshot::Stored { json: Arc::new(json), images },
            packed: Arc::default(),
            delta: Arc::default(),
        });
    }
    out
}

/// The branches of an archive (none when it has no index), parsed. Damaged entries are skipped and
/// reported in `issues`; nothing here fails the load.
fn read_branches(
    z: &mut impl craft_figcraft_format::NativeArchive,
    images: &BTreeMap<String, ImageData>,
    issues: &mut Vec<String>,
    limits: VersionLimits,
    unpacked: &mut Unpacked,
) -> Vec<Branch> {
    if !z.contains(BRANCH_INDEX) {
        return Vec::new();
    }
    let index = match unpacked
        .read(z, BRANCH_INDEX, MAX_VERSION_INDEX)
        .and_then(|b| craft_figcraft_format::read_branch_index(&b).map_err(|e| FormatError::Corrupt(e.to_string())))
    {
        Ok(v) => v,
        Err(e) => {
            issues.push(format!("the branches were skipped: {e}"));
            return Vec::new();
        }
    };
    let mut out: Vec<Branch> = Vec::new();
    let mut budget = limits.total;
    for e in index.branches {
        if out.len() >= MAX_BRANCHES {
            issues.push(format!("only the first {MAX_BRANCHES} branches were kept"));
            break;
        }
        let id = e.id.as_str();
        if !is_valid_version_id(id) || out.iter().any(|b| b.id == id) {
            issues.push("a branch with an invalid id was skipped".into());
            continue;
        }
        let mut snapshot = |part: &str, hashes: &[String]| -> Result<Arc<Document>, FormatError> {
            let json = unpacked.read(z, &format!("branches/{id}/{part}.json"), budget.min(limits.member))?;
            budget = budget.saturating_sub(json.len() as u64);
            let (mut doc, _) = from_json(std::str::from_utf8(&json).map_err(|e| FormatError::Corrupt(e.to_string()))?)?;
            doc.images = hashes.iter().filter_map(|h| images.get(h).map(|i| (h.clone(), i.clone()))).collect();
            Ok(Arc::new(doc))
        };
        let (base, head) = match (snapshot("base", &e.base_images), snapshot("head", &e.head_images)) {
            (Ok(b), Ok(h)) => (b, h),
            (Err(err), _) | (_, Err(err)) => {
                issues.push(format!("branch `{id}` was skipped: {err}"));
                continue;
            }
        };
        out.push(Branch {
            id: id.to_string(),
            name: clip(&e.name, MAX_VERSION_NAME),
            time: e.time,
            client: e.client,
            state: match e.state {
                NativeBranchState::Active => BranchState::Active,
                NativeBranchState::Merged => BranchState::Merged,
                NativeBranchState::Archived => BranchState::Archived,
            },
            versions: read_versions(z, &format!("branches/{id}/versions"), images, issues, &mut budget, limits.member, unpacked),
            base,
            head,
        });
    }
    out
}

/// Save atomically: write a staging file next to `path`, then rename over it.
#[cfg(not(target_arch = "wasm32"))]
pub fn save_file(doc: &Document, path: &std::path::Path, thumbnail_png: Option<&[u8]>) -> Result<(), FormatError> {
    let bytes = save_bytes(doc, thumbnail_png)?;
    write_file_atomic(path, &bytes).map_err(|e| FormatError::Io(format!("{}: {e}", path.display())))
}

/// Write `bytes` to `path` through a staging file and one atomic rename, so a reader of `path`
/// sees either the previous file or the complete new one, never a partial write. The staging file
/// is a uniquely named sibling (`<path>.tmp-figcraft-<process id>-<counter>`) created exclusively
/// (`create_new`) and written through its own handle: a file or symlink that already exists at a
/// staging name is never truncated or followed, and separate writers (also of separate processes)
/// never share a staging file. On failure only the staging file this call created is removed, and
/// `path` is replaced only on success. The rename replaces an existing destination on every
/// platform (`std::fs::rename` uses `MoveFileEx` with `MOVEFILE_REPLACE_EXISTING` on Windows).
/// (No `sync_all` yet: nothing around here promises durability across a power cut.)
#[cfg(not(target_arch = "wasm32"))]
pub fn write_file_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

    static STAGING: AtomicU64 = AtomicU64::new(0);
    // Three tries at a fresh name: the counter is unique within this process, so only a leftover
    // of a dead process that now shares our process id can collide, and the next counter won't.
    for _ in 0..3 {
        let mut name = path.as_os_str().to_os_string();
        name.push(format!(".tmp-figcraft-{}-{}", std::process::id(), STAGING.fetch_add(1, Relaxed)));
        let tmp = std::path::PathBuf::from(name);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp) {
            Ok(mut file) => {
                // The replacement keeps the destination's permissions from the start, so a private
                // file's content is never readable under default ones (the open handle still writes).
                if let Ok(old) = std::fs::metadata(path) {
                    let _ = file.set_permissions(old.permissions());
                }
                if let Err(e) = file.write_all(bytes) {
                    let _ = std::fs::remove_file(&tmp);
                    return Err(e);
                }
                return match std::fs::rename(&tmp, path) {
                    Ok(()) => Ok(()),
                    // Leave nothing behind when the file can't take its place.
                    Err(e) => {
                        let _ = std::fs::remove_file(&tmp);
                        Err(e)
                    }
                };
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "no free staging name next to the file"))
}

#[cfg(not(target_arch = "wasm32"))]
pub fn load_file(path: &std::path::Path) -> Result<(Document, Vec<String>), FormatError> {
    let mut file = std::fs::File::open(path).map_err(|e| FormatError::Io(format!("{}: {e}", path.display())))?;
    let archive_len = file.metadata().map_err(|e| FormatError::Io(format!("{}: {e}", path.display())))?.len();
    let mut first = None;
    let mut chunk = [0u8; 8192];
    loop {
        let count = file.read(&mut chunk).map_err(|e| FormatError::Io(format!("{}: {e}", path.display())))?;
        if count == 0 {
            break;
        }
        if let Some(byte) = chunk[..count].iter().copied().find(|byte| !byte.is_ascii_whitespace()) {
            first = Some(byte);
            break;
        }
    }
    file.seek(SeekFrom::Start(0)).map_err(|e| FormatError::Io(format!("{}: {e}", path.display())))?;
    if first == Some(b'{') {
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(|e| FormatError::Io(format!("{}: {e}", path.display())))?;
        return load_bytes(&bytes);
    }
    let mut archive = craft_figcraft_format::SeekArchive::open(file, u64::MAX).map_err(|e| FormatError::Corrupt(e.to_string()))?;
    let loaded = load_archive(&mut archive, IMAGE_BUDGET, MAX_UNPACKED, archive_len, VersionLimits::DEFAULT)?;
    Ok((loaded.doc, loaded.issues))
}

#[cfg(test)]
mod branch_tests;
#[cfg(test)]
mod guides_tests;
#[cfg(test)]
mod image_budget_tests;
#[cfg(test)]
mod image_tests;
#[cfg(test)]
mod journal_tests;
#[cfg(test)]
mod jpeg_tests;
#[cfg(test)]
mod large_tests;
#[cfg(test)]
mod source_tests;
#[cfg(test)]
mod staging_tests;
#[cfg(test)]
mod streaming_tests;
#[cfg(test)]
mod thumbnail_tests;
#[cfg(test)]
mod unpacked_total_tests;
#[cfg(test)]
mod version_budget_tests;
#[cfg(test)]
mod version_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use figcraft_doc::NodeKind;

    fn png() -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(3, 2, image::Rgba([255, 0, 0, 255]));
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    #[test]
    fn roundtrip_with_images() {
        let mut d = Document::new("My file");
        let p = d.first_page().unwrap();
        d.create(p, NodeKind::Rectangle, "R", 1.0, 2.0, 3.0, 4.0).unwrap();
        let (hash, w, h) = add_image(&mut d, png()).unwrap();
        assert_eq!((w, h), (3, 2));
        let bytes = save_bytes(&d, None).unwrap();
        let (back, issues) = load_bytes(&bytes).unwrap();
        assert!(issues.is_empty());
        assert_eq!(back.name, "My file");
        assert_eq!(back.node_count(), d.node_count());
        assert!(back.images.contains_key(&hash));
        // Bare JSON opens too.
        let json = serde_json::to_vec(&to_json(&d).unwrap()).unwrap();
        assert!(load_bytes(&json).is_ok());
    }

    #[test]
    fn non_finite_numbers_are_saved_as_zero() {
        let mut d = Document::new("NaN");
        let p = d.first_page().unwrap();
        let r = d.create(p, NodeKind::Rectangle, "R", 1.0, 2.0, 3.0, 4.0).unwrap();
        assert!(save_bytes(&d, None).is_ok());
        d.get_mut(r).unwrap().width = f64::NAN;
        d.get_mut(r).unwrap().opacity = f32::INFINITY;
        // Saving never fails; non-finite numbers are written as 0 and the file reopens.
        let bytes = save_bytes(&d, None).unwrap();
        let (back, _) = load_bytes(&bytes).unwrap();
        assert_eq!(back.node_count(), d.node_count());
        let n = back.get(r).unwrap();
        assert_eq!(n.width, 0.0);
        assert_eq!(n.opacity, 0.0);
        assert_eq!(n.height, 4.0);
        let mut out = Vec::new();
        write_json(&d, &mut out).unwrap();
        let (back, _) = from_json(std::str::from_utf8(&out).unwrap()).unwrap();
        let n = back.get(r).unwrap();
        assert_eq!(n.width, 0.0);
        assert_eq!(n.opacity, 0.0);
        assert_eq!(n.height, 4.0);
        let v = to_json(&d).unwrap();
        let (back, _) = from_json(&v.to_string()).unwrap();
        let n = back.get(r).unwrap();
        assert_eq!(n.width, 0.0);
        assert_eq!(n.opacity, 0.0);
        assert_eq!(n.height, 4.0);
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(load_bytes(b"not a zip").is_err());
        assert!(load_bytes(b"{\"nope\":1}").is_err());
        assert!(matches!(load_bytes(b"{\"version\":999}"), Err(FormatError::Newer(999))));
        assert!(add_image(&mut Document::new("x"), b"zzz".to_vec()).is_err());
    }
}
