//! The version journal: automatic versions as deltas replay exactly, damaged tails are dropped,
//! old files open, and the delta files are far smaller than full snapshots.

use std::io::Write;
use std::sync::Arc;

use figcraft_doc::{Document, NodeId, NodeKind};

use super::*;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    /// A multiple of 0.5 (floats that print and parse back the same).
    fn coord(&mut self) -> f64 {
        self.below(2000) as f64 * 0.5
    }
}

fn json(d: &Document) -> Value {
    to_json(d).unwrap()
}

fn auto(id: &str, doc: &Document) -> Version {
    let mut v = Version::new(id, &format!("Autosave {id}"), "", 1_700_000_000, Arc::new(doc.clone()));
    v.auto = true;
    v
}

fn names(z: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>) -> Vec<String> {
    z.file_names().map(str::to_string).collect()
}

/// The archive with member `name` replaced by `bytes` (or left out when `None`).
fn rewrite(archive: &[u8], name: &str, bytes: Option<&[u8]>) -> Vec<u8> {
    let mut src = zip::ZipArchive::new(std::io::Cursor::new(archive)).unwrap();
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for i in 0..src.len() {
        let f = src.by_index(i).unwrap();
        if f.name() == name {
            if let Some(b) = bytes {
                out.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
                out.write_all(b).unwrap();
            }
        } else {
            out.raw_copy_file(f).unwrap();
        }
    }
    out.finish().unwrap().into_inner()
}

fn random_edit(rng: &mut Rng, doc: &mut Document, made: &mut Vec<NodeId>) {
    let page = doc.first_page().unwrap();
    let frames: Vec<NodeId> = made.iter().copied().filter(|id| doc.node(*id).is_some_and(|n| matches!(n.kind, NodeKind::Frame))).collect();
    let live: Vec<NodeId> = made.iter().copied().filter(|id| doc.contains(*id)).collect();
    *made = live.clone();
    let parent = |rng: &mut Rng| if frames.is_empty() || rng.below(3) == 0 { page } else { frames[rng.below(frames.len())] };
    let pick = |rng: &mut Rng| if live.is_empty() { None } else { Some(live[rng.below(live.len())]) };
    match rng.below(10) {
        0..=2 => {
            let p = parent(rng);
            let kind = if rng.below(4) == 0 { NodeKind::Frame } else { NodeKind::Rectangle };
            let name = format!("L{}", rng.below(1000));
            let (x, y, w, h) = (rng.coord(), rng.coord(), rng.coord() + 1.0, rng.coord() + 1.0);
            made.extend(doc.create(p, kind, &name, x, y, w, h));
        }
        3..=5 => {
            if let Some(id) = pick(rng) {
                let (x, w) = (rng.coord(), rng.coord());
                if let Some(n) = doc.node_mut(id) {
                    n.height = x + 1.0;
                    n.width = w + 1.0;
                }
            }
        }
        6 => {
            if let Some(id) = pick(rng) {
                let name = format!("renamed {}", rng.below(100));
                let opacity = rng.below(5) as f32 * 0.25;
                if let Some(n) = doc.node_mut(id) {
                    n.name = name;
                    n.opacity = opacity;
                    n.visible = rng.below(2) == 0;
                }
            }
        }
        7 => {
            // Move a layer that is not a frame (no cycles to worry about) to another parent.
            if let Some(id) = pick(rng).filter(|id| doc.node(*id).is_some_and(|n| !matches!(n.kind, NodeKind::Frame))) {
                let p = parent(rng);
                let _ = doc.reparent(id, p, None);
            }
        }
        8 => {
            if let Some(id) = pick(rng) {
                let _ = doc.remove(id);
            }
        }
        _ => {
            if rng.below(5) == 0 {
                let name = format!("Page {}", rng.below(50));
                doc.add_page(&name);
            } else {
                doc.name = format!("File {}", rng.below(50));
            }
        }
    }
}

#[test]
fn random_edits_replay_exactly_through_a_file() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut doc = Document::new("Random");
    let mut made = Vec::new();
    let mut versions = Vec::new();
    let mut snaps = Vec::new();
    for step in 0..400 {
        for _ in 0..=rng.below(3) {
            random_edit(&mut rng, &mut doc, &mut made);
        }
        if step % 4 == 0 {
            let id = format!("v{step}");
            let mut v = auto(&id, &doc);
            v.auto = step % 52 != 0; // a named version now and then
            versions.push(v);
            snaps.push(json(&doc));
        }
    }
    let bytes = save_bytes_with_versions(&doc, None, &versions).unwrap();
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(&bytes[..])).unwrap();
    let deltas = names(&mut z).iter().filter(|n| n.starts_with("history/") && n.ends_with(".delta")).count();
    assert!(deltas >= 60, "most automatic versions are deltas, got {deltas} of {}", versions.len());

    fn check(l: &Loaded, versions: &[Version], snaps: &[Value]) {
        assert!(l.issues.is_empty(), "{:?}", l.issues);
        assert_eq!(l.versions.len(), versions.len());
        for (i, v) in l.versions.iter().enumerate() {
            assert_eq!((v.id.as_str(), v.name.as_str(), v.auto), (versions[i].id.as_str(), versions[i].name.as_str(), versions[i].auto));
            assert_eq!(json(&v.document().unwrap()), snaps[i], "version {} replays exactly", v.id);
        }
    }
    let l = load_bytes_with_versions(&bytes).unwrap();
    check(&l, &versions, &snaps);
    // Saved again unchanged (deltas copied) and after losing the cache: the same.
    let again = save_bytes_with_history(&l.doc, None, &l.versions, &[]).unwrap();
    check(&load_bytes_with_versions(&again).unwrap(), &versions, &snaps);
    // A version taken out of the middle re-bases the one after it.
    let mut fewer = l.versions.clone();
    fewer.remove(7);
    snaps.remove(7);
    versions.remove(7);
    let l2 = load_bytes_with_versions(&save_bytes_with_history(&l.doc, None, &fewer, &[]).unwrap()).unwrap();
    check(&l2, &versions, &snaps);
}

#[test]
fn a_damaged_tail_is_dropped_with_a_message() {
    let mut doc = Document::new("Tail");
    let p = doc.first_page().unwrap();
    let mut versions = vec![Version::new("k0", "Named", "", 1, Arc::new(doc.clone()))];
    for i in 1..=4 {
        doc.create(p, NodeKind::Rectangle, &format!("R{i}"), i as f64, 0.0, 10.0, 10.0).unwrap();
        versions.push(auto(&format!("a{i}"), &doc));
    }
    let bytes = save_bytes_with_versions(&doc, None, &versions).unwrap();
    assert_eq!(load_bytes_with_versions(&bytes).unwrap().versions.len(), 5);

    // a3's delta is garbage that still unzips: its checksum fails, a4 (built on it) goes too.
    let bad = rewrite(&bytes, "history/a3.delta", Some(b"{\"nodes\":[]}"));
    let l = load_bytes_with_versions(&bad).unwrap();
    assert_eq!(l.versions.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(), ["k0", "a1", "a2"]);
    assert!(l.issues.iter().any(|i| i.contains("`a3`") && i.contains("checksum")), "{:?}", l.issues);
    assert!(l.issues.iter().any(|i| i.contains("`a4`") && i.contains("missing")), "{:?}", l.issues);
    assert_eq!(l.doc.node_count(), doc.node_count());
    assert_eq!(l.versions[2].document().unwrap().node_count(), 3);
    // What is left saves and reopens cleanly.
    let ok = load_bytes_with_versions(&save_bytes_with_history(&l.doc, None, &l.versions, &[]).unwrap()).unwrap();
    assert!(ok.issues.is_empty(), "{:?}", ok.issues);
    assert_eq!(ok.versions.len(), 3);

    // A missing member and an unreadable meta degrade the same way, never failing the open.
    let l = load_bytes_with_versions(&rewrite(&bytes, "history/a2.delta", None)).unwrap();
    assert_eq!(l.versions.len(), 2);
    assert!(!l.issues.is_empty());
    let l = load_bytes_with_versions(&rewrite(&bytes, "history/meta.json", Some(b"not json"))).unwrap();
    assert_eq!(l.versions.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(), ["k0"]);
    assert!(l.issues.iter().any(|i| i.contains("journal")), "{:?}", l.issues);
}

#[test]
fn files_without_a_journal_open_and_are_written_back_readable() {
    let mut doc = Document::new("Old");
    let p = doc.first_page().unwrap();
    let mut versions = Vec::new();
    for i in 0..5 {
        doc.create(p, NodeKind::Rectangle, &format!("R{i}"), i as f64, 0.0, 10.0, 10.0).unwrap();
        versions.push(auto(&format!("o{i}"), &doc));
    }
    // What a build without the journal writes: every version a full snapshot.
    let (old, _) = save_within(&doc, None, &versions, &[], VersionLimits::DEFAULT).unwrap();
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(&old[..])).unwrap();
    assert!(names(&mut z).iter().all(|n| !n.starts_with("history/")));
    let l = load_bytes_with_versions(&old).unwrap();
    assert!(l.issues.is_empty(), "{:?}", l.issues);
    assert_eq!(l.versions.len(), 5);
    let back = load_bytes_with_versions(&save_bytes_with_history(&l.doc, None, &l.versions, &[]).unwrap()).unwrap();
    assert!(back.issues.is_empty(), "{:?}", back.issues);
    for (i, v) in back.versions.iter().enumerate() {
        assert_eq!(json(&v.document().unwrap()), json(&versions[i].document().unwrap()));
    }

    // A build that doesn't know the journal ignores it: the file opens, with the full versions.
    let new = save_bytes_with_versions(&doc, None, &versions).unwrap();
    let no_meta = rewrite(&new, "history/meta.json", None);
    let l = load_bytes_with_versions(&no_meta).unwrap();
    assert!(l.issues.is_empty(), "{:?}", l.issues);
    assert_eq!(l.versions.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(), ["o0"]);
    assert_eq!(l.doc.node_count(), doc.node_count());
}

#[test]
fn named_versions_are_keyframes_and_reopening_keeps_ids_names_and_order() {
    let mut doc = Document::new("Names");
    let p = doc.first_page().unwrap();
    let mut versions = Vec::new();
    for i in 0..12 {
        doc.create(p, NodeKind::Rectangle, &format!("R{i}"), i as f64, 0.0, 10.0, 10.0).unwrap();
        let mut v = auto(&format!("id-{i}"), &doc);
        if i % 5 == 2 {
            v.auto = false;
            v.name = format!("Named {i}");
            v.description = "kept".into();
        }
        versions.push(v);
    }
    let bytes = save_bytes_with_versions(&doc, None, &versions).unwrap();
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(&bytes[..])).unwrap();
    let all = names(&mut z);
    for i in [2, 7] {
        assert!(all.contains(&format!("versions/id-{i}.json")), "named version {i} has a full snapshot");
    }
    let l = load_bytes_with_versions(&bytes).unwrap();
    let got: Vec<(&str, &str, bool)> = l.versions.iter().map(|v| (v.id.as_str(), v.name.as_str(), v.auto)).collect();
    let want: Vec<(&str, &str, bool)> = versions.iter().map(|v| (v.id.as_str(), v.name.as_str(), v.auto)).collect();
    assert_eq!(got, want);
    assert_eq!(l.versions[2].description, "kept");
    // Twice through a file keeps them too.
    let l = load_bytes_with_versions(&save_bytes_with_history(&l.doc, None, &l.versions, &[]).unwrap()).unwrap();
    let got: Vec<(&str, &str, bool)> = l.versions.iter().map(|v| (v.id.as_str(), v.name.as_str(), v.auto)).collect();
    assert_eq!(got, want);
}

#[test]
fn autosave_versions_take_far_less_room_than_full_snapshots() {
    let mut rng = Rng(42);
    let mut doc = Document::new("Big");
    let p = doc.first_page().unwrap();
    let mut ids = Vec::new();
    for i in 0..3000 {
        let (x, y) = (rng.coord(), rng.coord());
        ids.extend(doc.create(p, NodeKind::Rectangle, &format!("Layer {i}"), x, y, 100.0, 50.0));
    }
    let mut versions = Vec::new();
    for i in 0..20 {
        // A small edit: three layers move, one is added.
        for _ in 0..3 {
            let (id, x) = (ids[rng.below(ids.len())], rng.coord());
            doc.node_mut(id).unwrap().height = x + 1.0;
        }
        ids.extend(doc.create(p, NodeKind::Rectangle, "New", 1.0, 2.0, 3.0, 4.0));
        versions.push(auto(&format!("a{i}"), &doc));
    }
    let (full, _) = save_within(&doc, None, &versions, &[], VersionLimits::DEFAULT).unwrap();
    let journal = save_bytes_with_versions(&doc, None, &versions).unwrap();
    let one = save_bytes(&doc, None).unwrap();
    eprintln!("[journal-size] document alone {} B, 20 autosaves as full snapshots {} B, as journal {} B", one.len(), full.len(), journal.len());
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(&journal[..])).unwrap();
    let (mut delta_packed, mut delta_raw, mut n) = (0, 0, 0);
    for i in 0..z.len() {
        let f = z.by_index(i).unwrap();
        if f.name().ends_with(".delta") {
            (delta_packed, delta_raw, n) = (delta_packed + f.compressed_size(), delta_raw + f.size(), n + 1);
        }
    }
    eprintln!("[journal-size] {n} deltas: {delta_packed} B packed, {delta_raw} B raw; meta.json {} B packed", z.by_name(journal::META).unwrap().compressed_size());
    assert!(journal.len() * 4 < full.len(), "journal {} vs full {}", journal.len(), full.len());
    let l = load_bytes_with_versions(&journal).unwrap();
    assert!(l.issues.is_empty(), "{:?}", l.issues);
    assert_eq!(l.versions.len(), 20);
    assert_eq!(json(&l.versions[19].document().unwrap()), json(&doc));
}
