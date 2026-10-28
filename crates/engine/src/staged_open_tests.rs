//! A large `.fig` file opens page-first: the first page as a read-only tab, then the whole file.

use figcraft_doc::{Document, NodeKind};
use serde_json::{Value, json};

use crate::Session;
use crate::cmd::file::{Preloaded, preload_staged};

fn fresh() -> Session {
    let mut s = Session::new();
    s.docs.clear();
    s
}

/// A two-page `.fig` on disk: "First" holds a rectangle, "Second" another.
fn two_page_fig(name: &str) -> String {
    let mut doc = Document::new("Staged");
    let first = doc.first_page().unwrap();
    doc.create(first, NodeKind::Rectangle, "First rect", 0.0, 0.0, 10.0, 10.0).unwrap();
    let second = doc.add_page("Second");
    doc.create(second, NodeKind::Rectangle, "Second rect", 0.0, 0.0, 20.0, 20.0).unwrap();
    let (bytes, _) = figcraft_format::fig::export(&doc, None).unwrap();
    let dir = std::env::temp_dir().join(format!("figcraft-staged-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path.to_string_lossy().into_owned()
}

fn json_of(doc: &Document) -> String {
    let mut out = Vec::new();
    figcraft_format::write_json(doc, &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

fn delivered(params: &Value, min_nodes: usize) -> Vec<Option<Preloaded>> {
    let mut got = Vec::new();
    preload_staged(params, min_nodes, &mut |r| got.push(r)).unwrap();
    got
}

#[test]
fn the_first_page_opens_read_only_and_the_whole_file_replaces_it() {
    let path = two_page_fig("two.fig");
    let params = json!({"path": path});
    let mut got = delivered(&params, 0);
    assert_eq!(got.len(), 1, "one early delivery; the whole file follows through it");

    let mut s = fresh();
    s.preloaded = got.pop().unwrap();
    let opened = s.execute("file.open", &params).unwrap();
    assert_eq!(opened["loading"], json!(true));
    assert!(opened.get("warnings").is_none(), "the notes belong to the whole file: {opened}");
    assert_eq!(opened["summary"]["pages"], json!(1));
    assert!(s.is_loading());
    let page_names: Vec<&str> = s.doc().unwrap().pages.iter().filter_map(|p| s.doc().unwrap().node(*p)).map(|p| p.name.as_str()).collect();
    assert_eq!(page_names, ["Page 1"]);

    let edit = s.execute("shape.rectangle", &json!({"x": 0, "y": 0, "width": 10, "height": 10})).unwrap_err();
    assert!(edit.to_string().contains("still loading"), "{edit}");
    assert_eq!(s.doc().unwrap().node_count(), 2, "the refused edit left the document as it was");
    let save = s.execute("file.saveAs", &json!({"path": format!("{path}.figcraft")})).unwrap_err();
    assert!(save.to_string().contains("still loading"), "{save}");

    let uid = s.docs[0].uid;
    let done = s.poll_loading();
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].uid, uid);
    let result = done[0].result.as_ref().unwrap();
    assert_eq!(result["summary"]["pages"], json!(2));
    assert!(result.get("warnings").is_some());
    assert!(!s.is_loading());
    assert_eq!(s.docs[0].uid, uid, "same tab");
    assert_eq!(s.doc().unwrap().pages.len(), 2);
    assert!(s.doc().unwrap().nodes().any(|n| n.name == "Second rect"));
    s.execute("shape.rectangle", &json!({"x": 0, "y": 0, "width": 10, "height": 10})).unwrap();
}

#[test]
fn a_page_first_open_ends_with_the_document_of_a_plain_open() {
    let path = two_page_fig("same.fig");
    let params = json!({"path": path});
    let mut plain = fresh();
    plain.execute("file.open", &params).unwrap();

    let mut s = fresh();
    s.preloaded = delivered(&params, 0).pop().unwrap();
    s.execute("file.open", &params).unwrap();
    s.poll_loading();
    assert_eq!(json_of(s.doc().unwrap()), json_of(plain.doc().unwrap()));
}

#[test]
fn small_files_and_single_page_files_open_whole() {
    let path = two_page_fig("small.fig");
    let params = json!({"path": path});
    let mut got = delivered(&params, usize::MAX);
    assert_eq!(got.len(), 1);
    let mut s = fresh();
    s.preloaded = got.pop().unwrap();
    let opened = s.execute("file.open", &params).unwrap();
    assert!(opened.get("loading").is_none());
    assert_eq!(opened["summary"]["pages"], json!(2));
    assert!(!s.is_loading());

    let mut doc = Document::new("One");
    let page = doc.first_page().unwrap();
    doc.create(page, NodeKind::Rectangle, "R", 0.0, 0.0, 1.0, 1.0).unwrap();
    let (bytes, _) = figcraft_format::fig::export(&doc, None).unwrap();
    let one = std::env::temp_dir().join(format!("figcraft-staged-{}", std::process::id())).join("one.fig");
    std::fs::write(&one, bytes).unwrap();
    let got = delivered(&json!({"path": one.to_string_lossy()}), 0);
    assert_eq!(got.len(), 1, "a single page has no early stage");
}

#[test]
fn a_tab_whose_file_never_arrives_closes_with_a_message() {
    let path = two_page_fig("lost.fig");
    let params = json!({"path": path});
    let mut s = fresh();
    s.preloaded = delivered(&params, 0).pop().unwrap();
    s.execute("file.open", &params).unwrap();
    // The worker's sender is gone without a result: simulate by swapping in a dead channel.
    let (tx, rx) = std::sync::mpsc::channel();
    drop(tx);
    s.docs[0].loading = Some(crate::doc_state::Loading::new(rx));
    let done = s.poll_loading();
    assert_eq!(done.len(), 1);
    assert!(done[0].result.is_err());
    assert!(s.docs.is_empty());
    assert!(s.message.as_deref().is_some_and(|m| m.contains("stopped loading")));
}

/// Time to the first page and to the whole file, as the app sees them (worker thread, then the UI
/// thread's open and swap): `FIGCRAFT_FIG_SNAPSHOT=<file.fig> FIGCRAFT_STAGED=0|1 cargo test
/// --release -p figcraft-engine open_timing -- --ignored --nocapture`.
#[test]
#[ignore = "needs FIGCRAFT_FIG_SNAPSHOT=<path to a large .fig file>"]
fn open_timing() {
    use std::time::Instant;
    let Ok(path) = std::env::var("FIGCRAFT_FIG_SNAPSHOT") else { return };
    let staged = std::env::var("FIGCRAFT_STAGED").map_or(true, |v| v != "0");
    let min_nodes = if staged { crate::cmd::file::STAGED_MIN_NODES } else { usize::MAX };
    let params = json!({"path": path});
    let t0 = Instant::now();
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = {
        let params = params.clone();
        std::thread::spawn(move || {
            preload_staged(&params, min_nodes, &mut |r| {
                let _ = tx.send((r, Instant::now()));
            })
        })
    };
    let (first, at) = rx.recv().unwrap();
    let mut s = fresh();
    s.preloaded = first;
    let t_open = Instant::now();
    let opened = s.execute("file.open", &params).unwrap();
    let shown = t0.elapsed();
    println!("TIMING staged={staged} worker_to_first={:.2}s ui_open={:.2}s first_page_shown={:.2}s", (at - t0).as_secs_f64(), t_open.elapsed().as_secs_f64(), shown.as_secs_f64());
    println!("TIMING first_summary={}", opened["summary"]);
    if s.is_loading() {
        loop {
            let t = Instant::now();
            let done = s.poll_loading();
            if let Some(d) = done.first() {
                println!("TIMING swap_on_ui_thread={:.2}s whole_file_at={:.2}s summary={}", t.elapsed().as_secs_f64(), t0.elapsed().as_secs_f64(), d.result.as_ref().unwrap()["summary"]);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
    let _ = worker.join();
    let mut out = std::io::sink();
    let _ = figcraft_format::write_json(s.doc().unwrap(), &mut out);
    println!("TIMING total={:.2}s", t0.elapsed().as_secs_f64());
}
