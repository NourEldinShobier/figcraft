//! Files picked by the user decode on a worker thread behind the progress card.

use figcraft_engine::Session;
use serde_json::json;

use crate::layers_drag_tests::Ui;
use crate::pending_open::{Load, queue_open};

const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10"/></svg>"#;

fn svg(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("figcraft-pending-open-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, SVG).unwrap();
    path.to_string_lossy().into_owned()
}

/// Frames until nothing is pending (at most ~5 s); the frames that saw a worker running or finished.
fn settle(u: &mut Ui) -> Vec<bool> {
    let mut docs_when_loading = Vec::new();
    for _ in 0..500 {
        u.frame(vec![]);
        if u.app.ui.pending_opens.is_empty() {
            break;
        }
        if u.app.ui.pending_opens.iter().any(|p| matches!(p.load, Load::Running { .. } | Load::Done(_))) {
            docs_when_loading.push(u.app.session.docs.is_empty());
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    docs_when_loading
}

#[test]
fn a_picked_file_decodes_off_thread_then_opens_as_file_open_does() {
    let mut u = Ui::new(Session::new());
    u.app.session.docs.clear();
    u.app.ui.home = true;
    queue_open(&mut u.app, "file.open", json!({"path": svg("one.svg")}));
    let while_loading = settle(&mut u);
    assert!(!while_loading.is_empty() && while_loading.iter().all(|empty| *empty), "no document opens before the decode is collected");
    assert_eq!(u.app.session.docs.len(), 1);
    assert!(!u.app.ui.home, "the home screen gives way to the file");
    assert!(u.app.session.preloaded.is_none());
}

#[test]
fn a_failed_decode_shows_the_error_of_file_open() {
    let mut u = Ui::new(Session::new());
    let before = u.app.session.docs.len();
    let params = json!({"path": "Z:/figcraft-nope/missing.fig"});
    let expected = Session::new().execute("file.open", &params).unwrap_err().to_string();
    queue_open(&mut u.app, "file.open", params);
    settle(&mut u);
    assert_eq!(u.app.ui.last_error.as_deref(), Some(expected.as_str()));
    assert_eq!(u.app.session.docs.len(), before);
}

#[test]
fn files_opened_while_one_loads_open_in_order() {
    let mut u = Ui::new(Session::new());
    u.app.session.docs.clear();
    queue_open(&mut u.app, "file.open", json!({"path": svg("first.svg")}));
    u.frame(vec![]);
    queue_open(&mut u.app, "file.openRecent", json!({"path": svg("second.svg")}));
    settle(&mut u);
    assert_eq!(u.app.session.docs.iter().map(|d| d.doc.name.as_str()).collect::<Vec<_>>(), ["first", "second"]);
}

#[test]
fn a_page_first_open_shows_the_first_page_then_the_whole_file_with_its_report() {
    use figcraft_doc::{Document, NodeKind};
    let mut doc = Document::new("Two pages");
    let first = doc.first_page().unwrap();
    doc.create(first, NodeKind::Rectangle, "A", 0.0, 0.0, 10.0, 10.0).unwrap();
    let second = doc.add_page("Second");
    doc.create(second, NodeKind::Rectangle, "B", 0.0, 0.0, 10.0, 10.0).unwrap();
    let (bytes, _) = figcraft_format::fig::export(&doc, None).unwrap();
    let dir = std::env::temp_dir().join(format!("figcraft-pending-open-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("two.fig");
    std::fs::write(&path, bytes).unwrap();
    let params = json!({"path": path.to_string_lossy()});

    let mut u = Ui::new(Session::new());
    u.app.session.docs.clear();
    figcraft_engine::cmd::file::preload_staged(&params, figcraft_format::fig::PageFirst::ALWAYS, &mut |r| u.app.session.preloaded = r).unwrap();
    u.app.run_user("file.open", params);
    assert!(u.app.session.is_loading());
    assert_eq!(u.app.session.doc().unwrap().pages.len(), 1);
    assert!(u.app.ui.import_report.is_none(), "the report waits for the whole file");

    u.frame(vec![]);
    assert!(!u.app.session.is_loading());
    assert_eq!(u.app.session.doc().unwrap().pages.len(), 2);
    let report = u.app.ui.import_report.as_ref().expect("the report for the whole file");
    assert_eq!(report.pages, Some(2));
    assert_eq!(report.uid, u.app.session.ds().unwrap().uid);
}
