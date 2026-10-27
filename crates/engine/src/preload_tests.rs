//! A file decoded ahead of `file.open` (off the UI thread) opens exactly as one read by the command.

use serde_json::{Value, json};

use crate::Session;

fn fresh() -> Session {
    let mut s = Session::new();
    s.docs.clear();
    s
}
use crate::cmd::file::{Preloaded, preload};

const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10"/></svg>"#;

fn svg(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("figcraft-preload-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, SVG).unwrap();
    path.to_string_lossy().into_owned()
}

fn open(s: &mut Session, params: &Value) -> Result<Value, String> {
    s.execute("file.open", params).map_err(|e| e.to_string())
}

#[test]
fn preloaded_crosses_threads() {
    fn send<T: Send>() {}
    send::<Preloaded>();
}

#[test]
fn a_preloaded_file_opens_like_a_read_one_and_is_used_once() {
    let path = svg("same.svg");
    let params = json!({"path": path});
    let sync = open(&mut fresh(), &params).unwrap();

    let mut s = fresh();
    s.preloaded = preload(&params);
    // Gone from disk: only the preloaded copy can open it.
    std::fs::remove_file(&path).unwrap();
    assert_eq!(open(&mut s, &params).unwrap(), sync);
    assert!(s.preloaded.is_none(), "consumed by the open");
    assert_eq!(s.docs.len(), 1);

    s.docs.clear();
    assert!(open(&mut s, &params).is_err(), "the second open reads the (missing) file again");
}

#[test]
fn a_preload_for_another_file_is_ignored() {
    let (a, b) = (svg("a.svg"), svg("b.svg"));
    let mut s = fresh();
    s.preloaded = preload(&json!({"path": a}));
    std::fs::remove_file(&a).unwrap();
    open(&mut s, &json!({"path": b})).unwrap();
    assert!(s.preloaded.is_none());
    assert_eq!(s.docs[0].path, None, "svg opens as an import, no path");
    assert_eq!(s.docs.len(), 1);
    assert!(open(&mut s, &json!({"path": a})).is_err(), "A was not opened from the mismatched preload");
}

#[test]
fn a_failed_preload_fails_the_open_with_the_same_error() {
    let params = json!({"path": "Z:/figcraft-nope/missing.fig"});
    let expected = open(&mut fresh(), &params).unwrap_err();
    let mut s = fresh();
    s.preloaded = preload(&params);
    assert_eq!(open(&mut s, &params).unwrap_err(), expected);
    assert!(s.docs.is_empty());
}

#[test]
fn preloaded_data_opens_like_data() {
    use base64::Engine as _;
    let params = json!({"data": base64::engine::general_purpose::STANDARD.encode(SVG), "name": "Dropped.svg"});
    let sync = open(&mut fresh(), &params).unwrap();
    let mut s = fresh();
    s.preloaded = preload(&params);
    assert_eq!(open(&mut s, &params).unwrap(), sync);
    assert!(s.preloaded.is_none());
    // A preload for a path says nothing about data of the same name.
    s.preloaded = preload(&json!({"path": svg("Dropped.svg")}));
    open(&mut s, &params).unwrap();
    assert!(s.preloaded.is_none());
}

#[test]
fn a_preload_does_not_bypass_confinement() {
    use crate::file_access::{Roots, confined};
    let path = svg("confined.svg");
    let other = std::env::temp_dir().join("figcraft-preload-elsewhere");
    std::fs::create_dir_all(&other).unwrap();
    let roots = std::sync::Arc::new(Roots::new(&[other.to_string_lossy().into_owned()], &[]).unwrap());
    let mut s = fresh();
    s.preloaded = preload(&json!({"path": path}));
    let r = confined(Some(&roots), || open(&mut s, &json!({"path": path})));
    assert!(r.is_err(), "{r:?}");
    assert!(s.docs.is_empty());
}

#[test]
fn nothing_to_preload_without_a_file() {
    assert!(preload(&json!({})).is_none());
    assert!(preload(&json!({"data": "AAAA"})).is_none(), "data needs a name");
}
