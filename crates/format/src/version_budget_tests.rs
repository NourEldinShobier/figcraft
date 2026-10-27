//! Saving only the version history a reader accepts, and reusing compressed snapshots.

use std::io::Read;
use std::sync::Arc;

use figcraft_doc::{Document, NodeKind};

use super::*;

fn doc_with(rects: usize) -> Arc<Document> {
    let mut d = Document::new("File");
    let p = d.first_page().unwrap();
    for i in 0..rects {
        d.create(p, NodeKind::Rectangle, &format!("R{i}"), i as f64, 0.0, 10.0, 10.0).unwrap();
    }
    Arc::new(d)
}

fn version(id: &str, auto: bool, rects: usize) -> Version {
    let mut v = Version::new(id, &format!("name {id}"), "", 1, doc_with(rects));
    v.auto = auto;
    v
}

fn size(v: &Version) -> u64 {
    v.packed().unwrap().size
}

fn ids(l: &Loaded) -> Vec<&str> {
    l.versions.iter().map(|v| v.id.as_str()).collect()
}

fn load(bytes: &[u8], limits: VersionLimits) -> Loaded {
    load_within_limits(bytes, IMAGE_BUDGET, limits).unwrap()
}

#[test]
fn everything_is_kept_when_it_fits() {
    let versions = vec![version("a", true, 5), version("b", false, 6), version("c", true, 7)];
    let (bytes, notes) = save_bytes_reporting(&Document::new("File"), None, &versions, &[]).unwrap();
    assert!(notes.is_empty(), "{notes:?}");
    let l = load_bytes_with_versions(&bytes).unwrap();
    assert!(l.issues.is_empty(), "{:?}", l.issues);
    assert_eq!(ids(&l), ["a", "b", "c"]);
    assert_eq!(l.versions[1].name, "name b");
    assert!(!l.versions[1].auto && l.versions[2].auto);
}

#[test]
fn oldest_checkpoints_go_first_and_the_file_reopens_whole() {
    let versions = vec![version("a", true, 5), version("n1", false, 6), version("b", true, 7), version("n2", false, 8)];
    let all: u64 = versions.iter().map(size).sum();
    // One byte short: the oldest checkpoint is the one dropped, named versions stay.
    let limits = VersionLimits { member: all, total: all - 1 };
    let (bytes, notes) = save_within(&Document::new("File"), None, &versions, &[], limits).unwrap();
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("1 automatic version"), "{notes:?}");
    let l = load(&bytes, limits);
    assert!(l.issues.is_empty(), "no version lost on reopening: {:?}", l.issues);
    assert_eq!(ids(&l), ["n1", "b", "n2"]);
    // Room for the two named only: both checkpoints go.
    let limits = VersionLimits { member: all, total: size(&versions[1]) + size(&versions[3]) };
    let (bytes, notes) = save_within(&Document::new("File"), None, &versions, &[], limits).unwrap();
    assert!(notes[0].contains("2 automatic versions"), "{notes:?}");
    let l = load(&bytes, limits);
    assert!(l.issues.is_empty(), "{:?}", l.issues);
    assert_eq!(ids(&l), ["n1", "n2"]);
    assert_eq!((l.versions[0].name.as_str(), l.versions[1].name.as_str()), ("name n1", "name n2"));
}

#[test]
fn named_versions_go_only_when_nothing_else_can() {
    let versions = vec![version("n1", false, 5), version("n2", false, 6), version("n3", false, 7)];
    let limits = VersionLimits { member: u64::MAX, total: size(&versions[1]) + size(&versions[2]) };
    let (bytes, notes) = save_within(&Document::new("File"), None, &versions, &[], limits).unwrap();
    assert!(notes[0].contains("name n1"), "the user is told which named version: {notes:?}");
    let l = load(&bytes, limits);
    assert!(l.issues.is_empty(), "{:?}", l.issues);
    assert_eq!(ids(&l), ["n2", "n3"]);
}

#[test]
fn a_snapshot_over_the_member_cap_is_not_written() {
    let versions = vec![version("small", false, 2), version("huge", false, 40)];
    let limits = VersionLimits { member: size(&versions[0]), total: u64::MAX };
    let (bytes, notes) = save_within(&Document::new("File"), None, &versions, &[], limits).unwrap();
    assert!(notes[0].contains("name huge"), "{notes:?}");
    let l = load(&bytes, limits);
    assert!(l.issues.is_empty(), "{:?}", l.issues);
    assert_eq!(ids(&l), ["small"]);
}

#[test]
fn branch_versions_obey_the_budget_too() {
    let b = Branch {
        id: "br".into(),
        name: "Try".into(),
        time: 1,
        client: 0,
        state: BranchState::Active,
        base: doc_with(1),
        head: doc_with(2),
        versions: vec![version("x", true, 5), version("y", false, 6)],
    };
    let parts: u64 = [&b.base, &b.head].iter().map(|d| serde_json::to_vec(&to_json(d).unwrap()).unwrap().len() as u64).sum();
    let limits = VersionLimits { member: u64::MAX, total: parts + size(&b.versions[1]) + 50 };
    let (bytes, notes) = save_within(&Document::new("File"), None, &[], std::slice::from_ref(&b), limits).unwrap();
    assert!(notes[0].contains("branch Try"), "{notes:?}");
    let l = load(&bytes, limits);
    assert_eq!(l.branches.len(), 1);
    assert_eq!(l.branches[0].versions.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(), ["y"]);
}

#[test]
fn bad_ids_still_fail_even_when_the_version_would_be_dropped() {
    let versions = vec![version("a/b", true, 5)];
    let limits = VersionLimits { member: 1, total: 1 };
    assert!(save_within(&Document::new("File"), None, &versions, &[], limits).is_err());
}

#[test]
fn compressed_snapshots_are_reused_and_older_readers_can_read_them() {
    let versions = vec![version("a", false, 30), version("b", true, 31)];
    let first = versions[0].packed().unwrap();
    // Clones (what the recovery thread and the save thread hold) share the cache.
    assert!(Arc::ptr_eq(&first, &versions.clone()[0].packed().unwrap()));
    let cur = Document::new("File");
    let one = save_bytes_with_versions(&cur, None, &versions).unwrap();
    assert!(Arc::ptr_eq(&first, &versions[0].packed().unwrap()), "saving keeps the cached bytes");
    let two = save_bytes_with_versions(&cur, None, &versions).unwrap();
    assert_eq!(one.len(), two.len());

    // Same member names, plain deflate streams: a reader that only knows zip and JSON reads them.
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(&two[..])).unwrap();
    for (id, n) in [("a", 30), ("b", 31)] {
        let mut m = z.by_name(&format!("versions/{id}.json")).unwrap();
        assert_eq!(m.compression(), zip::CompressionMethod::Deflated);
        let mut text = String::new();
        m.read_to_string(&mut text).unwrap();
        assert_eq!(from_json(&text).unwrap().0.node_count(), doc_with(n).node_count());
    }
    // The plain loader (versions ignored) and the history loader both open it.
    assert_eq!(load_bytes(&two).unwrap().0.node_count(), cur.node_count());
    let l = load_bytes_with_versions(&two).unwrap();
    assert_eq!(ids(&l), ["a", "b"]);
    // Saving what was read compresses it once, then reuses it.
    let again = save_bytes_with_versions(&l.doc, None, &l.versions).unwrap();
    assert_eq!(ids(&load_bytes_with_versions(&again).unwrap()), ["a", "b"]);
    assert_eq!(l.versions[0].document().unwrap().node_count(), doc_with(30).node_count());
}
