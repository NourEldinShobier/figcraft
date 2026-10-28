//! Files dropped on the home screen or the canvas open a frame later, behind a card that says what
//! is happening: opening a large `.fig` blocks the UI thread for a while, and the card is on screen
//! before it does (rather than a window that freezes without a word). A file with no path to read
//! (every drop in the browser) waits behind the card until its contents arrive.

use std::sync::Arc;
use std::task::{Context as TaskContext, Poll, Waker};

use figcraft_doc::NodeId;
use figcraft_engine::cmd::file::Preloaded;
use serde_json::{Value, json};

use crate::i18n::{tr, trf, trn};
use crate::theme::{Tokens, semibold};
use crate::{FigApp, ReadFuture, RepaintWaker};

/// A dropped file waiting to be opened (`file.open`, a document of its own) or imported
/// (`file.import`, into the current document).
pub struct PendingOpen {
    /// What the card shows for it (the file name).
    pub label: String,
    pub params: Value,
    pub command: &'static str,
    /// Canvas imports stay with the document and page that received the drop.
    pub target: Option<ImportTarget>,
    /// The file's contents still on their way; they go into `params` as `data` once read.
    pub read: Option<ReadFuture>,
    /// The file being decoded off the UI thread (see [`Load`]).
    pub load: Load,
}

/// Decoding a file on a worker thread, so the window keeps painting and answering input while a
/// large file imports; the UI thread then only builds the document from the result.
#[derive(Default)]
pub enum Load {
    /// Not started (or nothing to decode off-thread: other commands, browser builds, a confined caller).
    #[default]
    Idle,
    Running {
        rx: std::sync::mpsc::Receiver<Option<Preloaded>>,
        since: std::time::Instant,
    },
    /// Finished: `file.open` gets the result (none: it reads the file itself).
    Done(Option<Preloaded>),
}

/// Whether `command` with `params` is an open that [`Load`] can decode ahead of time.
fn preloadable(command: &str, params: &Value) -> bool {
    matches!(command, "file.open" | "file.openRecent") && (params.get("path").is_some() || (params.get("data").is_some() && params.get("name").is_some()))
}

/// Starts decoding the first pending open that is ready for it. Only one file decodes at a time.
/// Where there are no threads (the browser) or file access is confined to automation roots (the
/// worker would not honour them), nothing starts and the open reads the file itself, as before.
#[cfg(not(target_arch = "wasm32"))]
fn start_load(app: &mut FigApp, ctx: &egui::Context) {
    if figcraft_engine::file_access::current().is_some() || app.ui.pending_opens.iter().any(|p| matches!(p.load, Load::Running { .. })) {
        return;
    }
    let Some(pending) = app.ui.pending_opens.iter_mut().find(|p| {
        matches!(p.load, Load::Idle) && p.read.is_none() && p.target.is_none() && preloadable(p.command, &p.params)
    }) else {
        return;
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let (params, ctx) = (pending.params.clone(), ctx.clone());
    let spawned = std::thread::Builder::new().name("open-file".into()).spawn(move || {
        // A panic in a decoder only loses the preload: the open then reads the file on the UI thread,
        // where the engine's guard turns it into an error. A large `.fig` file is sent in two steps:
        // the first page now, the whole file through the document it opens (`poll_loading_tabs`).
        let sent = std::cell::Cell::new(false);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            figcraft_engine::cmd::file::preload_staged(&params, figcraft_engine::cmd::file::STAGED_MIN_NODES, &mut |result| {
                sent.set(true);
                let _ = tx.send(result);
                ctx.request_repaint_of(egui::ViewportId::ROOT);
            })
        }));
        if !sent.get() {
            let _ = tx.send(None);
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
    });
    pending.load = if spawned.is_ok() { Load::Running { rx, since: std::time::Instant::now() } } else { Load::Done(None) };
}

/// Whether files can decode off-thread now.
fn threaded() -> bool {
    cfg!(not(target_arch = "wasm32")) && figcraft_engine::file_access::current().is_none()
}

#[cfg(target_arch = "wasm32")]
fn start_load(_: &mut FigApp, _: &egui::Context) {}

/// Collects finished decodes. Whether any is still running.
fn poll_loads(app: &mut FigApp) -> bool {
    let mut running = false;
    for pending in &mut app.ui.pending_opens {
        if let Load::Running { rx, .. } = &pending.load {
            match rx.try_recv() {
                Ok(result) => pending.load = Load::Done(result),
                Err(std::sync::mpsc::TryRecvError::Empty) => running = true,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => pending.load = Load::Done(None),
            }
        }
    }
    running
}

/// Queues `command` (`file.open`, `file.openRecent`) with `params` for the file picked by the user
/// (Open dialog, recent files): it decodes on a worker thread behind the progress card, then runs
/// as the same command. The label is the file's name.
pub(crate) fn queue_open(app: &mut FigApp, command: &'static str, params: Value) {
    let label = params
        .get("path")
        .and_then(Value::as_str)
        .and_then(|p| std::path::Path::new(p).file_name())
        .map(|n| n.to_string_lossy().to_string())
        .or_else(|| params.get("name").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_default();
    app.ui.pending_opens.push(PendingOpen { label, params, command, target: None, read: None, load: Load::Idle });
}

pub struct ImportTarget {
    pub uid: u64,
    pub page: NodeId,
    pub name: String,
}

/// Queues dropped file `f` to open as a document of its own: by path, or by its contents when there
/// is no file on disk to read. Callers checked its extension, so there is a name to tell the format by.
pub(crate) fn open_dropped(app: &mut FigApp, f: &egui::DroppedFileHandle) {
    let path = f.path();
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let (params, read) =
        if path.exists() { (json!({"path": path.to_string_lossy()}), None) } else { (json!({"name": name}), Some(crate::dropped_bytes(f, &name))) };
    app.ui.pending_opens.push(PendingOpen { label: name, params, command: "file.open", target: None, read, load: Load::Idle });
}

fn run_pending(app: &mut FigApp, pending: PendingOpen) -> Option<Value> {
    let target = match pending.target {
        Some(target) => target,
        None if pending.command != "file.import" => return app.run_user(pending.command, pending.params),
        None => {
            cancel(app, tr("The destination document is unavailable. Nothing was imported.").to_string());
            return None;
        }
    };
    let Some(index) = app.session.docs.iter().position(|doc| doc.uid == target.uid) else {
        cancel(app, tr("The destination document was closed. Nothing was imported.").to_string());
        return None;
    };
    if !app.session.docs[index].doc.pages.contains(&target.page) {
        cancel(app, tr("The destination page was removed. Nothing was imported.").to_string());
        return None;
    }

    let previous = app.session.ds().ok().map(|doc| doc.uid);
    let previous_page = app.session.docs[index].page;
    let previous_selection = app.session.docs[index].selection.clone();
    app.session.active = index;
    app.session.docs[index].page = target.page;
    let result = app.run_user(pending.command, pending.params);
    if previous_page != target.page
        && let Some(doc) = app.session.docs.iter_mut().find(|doc| doc.uid == target.uid)
        && doc.doc.pages.contains(&previous_page)
    {
        doc.page = previous_page;
        doc.selection = previous_selection;
        doc.clean_selection();
    }
    if let Some(index) = previous.and_then(|uid| app.session.docs.iter().position(|doc| doc.uid == uid)) {
        app.session.active = index;
    }
    result
}

/// An import that has no destination left: nothing is imported, and the user is told why.
fn cancel(app: &mut FigApp, message: String) {
    app.show_error(message);
}

/// Space between files dropped together on the canvas.
const IMPORT_GAP: f64 = 20.0;

/// Frames the card is drawn before the files open: egui lays a new card out invisibly on its first
/// frame, so the second one is the first the window shows.
const FRAMES_BEFORE_OPENING: u8 = 2;

/// Polls the contents still being read: those that arrived go into their file's parameters, files
/// that couldn't be read are dropped with a message. Whether any are still on their way.
fn read_contents(app: &mut FigApp, ctx: &egui::Context) -> bool {
    let waker = Waker::from(Arc::new(RepaintWaker(ctx.clone())));
    let mut cx = TaskContext::from_waker(&waker);
    let mut reading = false;
    let mut unread = Vec::new();
    app.ui.pending_opens.retain_mut(|pending| {
        let Some(read) = pending.read.as_mut() else { return true };
        match read.as_mut().poll(&mut cx) {
            Poll::Pending => {
                reading = true;
                true
            }
            Poll::Ready(Ok(bytes)) => {
                use base64::Engine as _;
                pending.params["data"] = json!(base64::engine::general_purpose::STANDARD.encode(&bytes));
                pending.read = None;
                true
            }
            Poll::Ready(Err(why)) => {
                unread.push(why);
                false
            }
        }
    });
    for why in unread {
        app.show_error(why);
    }
    reading
}

/// Tabs opened with only their first page: when the rest of the file is built the whole document
/// replaces it, and the import report and font check that wait for the whole file run then.
fn poll_loading_tabs(app: &mut FigApp, ctx: &egui::Context) {
    if !app.session.is_loading() {
        return;
    }
    // The worker can't wake the window for the second step: look again soon.
    ctx.request_repaint_after(std::time::Duration::from_millis(100));
    if app.session.ds().is_ok_and(|d| d.is_loading()) {
        let t = Tokens::get(ctx);
        egui::Area::new(egui::Id::new("loading_pages")).order(egui::Order::Foreground).anchor(egui::Align2::CENTER_BOTTOM, [0.0, -24.0]).interactable(false).show(
            ctx,
            |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(12.0));
                        ui.label(egui::RichText::new(tr("Loading the other pages…")).color(t.text_secondary));
                    });
                });
            },
        );
    }
    for done in app.session.poll_loading() {
        match done.result {
            Ok(result) => {
                if let Some(mut report) = crate::import_report::Report::from_result("file.open", &json!({}), &result) {
                    report.uid = done.uid;
                    app.ui.import_report = Some(report);
                }
                if !cfg!(target_arch = "wasm32") {
                    app.ui.font_checks.push(done.uid);
                }
            }
            Err(why) => app.show_error(why),
        }
        app.observe_presentation();
    }
}

/// The card while files wait, then (once it has been on screen and their contents are read) open
/// them. Called before the import report, so a report for these files shows after them.
pub fn pending_opens(app: &mut FigApp, ctx: &egui::Context) {
    poll_loading_tabs(app, ctx);
    let reading = read_contents(app, ctx);
    if app.ui.pending_opens.is_empty() {
        app.ui.pending_open_frames = 0;
        return;
    }
    ctx.request_repaint();
    // Decoding starts once the card has been on screen (and a file's contents are read).
    let threaded = threaded();
    if threaded && app.ui.pending_open_frames >= 1 {
        start_load(app, ctx);
    }
    let running = poll_loads(app);
    let queued = threaded && app.ui.pending_opens.iter().any(|p| matches!(p.load, Load::Idle) && preloadable(p.command, &p.params));
    let reading = reading || running || queued;
    if app.ui.pending_open_frames >= FRAMES_BEFORE_OPENING && !reading {
        app.ui.pending_open_frames = 0;
        // Files dropped together land side by side: each starts where the previous one ends.
        let mut next_x: Option<f64> = None;
        for mut pending in std::mem::take(&mut app.ui.pending_opens) {
            let command = pending.command;
            app.session.preloaded = match std::mem::take(&mut pending.load) {
                Load::Done(result) => result,
                _ => None,
            };
            let canvas = command == "file.import" && pending.params.get("x").is_some();
            if canvas && let Some(x) = next_x {
                pending.params["x"] = json!(x);
            }
            let page = pending.target.as_ref().map(|t| (t.uid, t.page));
            let result = run_pending(app, pending);
            app.session.preloaded = None;
            // `file.openRecent` also answers a bare list or a removal: only an opened file counts.
            let opened = result.as_ref().is_some_and(|r| command == "file.open" || r.get("index").is_some());
            if opened && command != "file.import" {
                app.ui.home = false;
            }
            if canvas {
                let right = result.as_ref().and_then(|r| r.get("id")?.as_u64()).and_then(|id| {
                    let doc = &app.session.docs.iter().find(|d| Some(d.uid) == page.map(|p| p.0))?.doc;
                    doc.world_bounds(NodeId(id)).map(|b| b.x1)
                });
                next_x = right.map(|r| r + IMPORT_GAP).or(next_x);
            }
        }
        return;
    }
    app.ui.pending_open_frames = app.ui.pending_open_frames.saturating_add(1);
    let title = match app.ui.pending_opens.first().map(|p| p.command) {
        Some("file.import") => {
            let name = app
                .ui
                .pending_opens
                .first()
                .and_then(|pending| pending.target.as_ref().map(|target| target.name.clone()))
                .or_else(|| app.session.doc().ok().map(|d| d.name.clone()))
                .unwrap_or_default();
            trf("Adding to {name}", &[("name", &name)])
        }
        _ => trn("Opening {n} files as new files", app.ui.pending_opens.len(), &[]),
    };
    let t = Tokens::get(ctx);
    let pending = &app.ui.pending_opens;
    // Shown at once: the frame after this one may block the window while the file opens.
    crate::dialogs::window("", ctx).id(egui::Id::unique("pending_open")).title_bar(false).movable(false).interactable(false).fade_in(false).show(
        ctx,
        |ui| {
            ui.set_width(480.0);
            ui.label(egui::RichText::new(title).font(semibold(13.0)));
            ui.add_space(12.0);
            for p in pending {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(16.0));
                    ui.label(p.label.as_str());
                });
            }
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(12.0));
                let secs = pending.iter().find_map(|p| match &p.load {
                    Load::Running { since, .. } => Some(since.elapsed().as_secs()),
                    _ => None,
                });
                let text = match secs {
                    Some(s) if s >= 1 => format!("{} {s} s", tr("Opening the file…")),
                    _ => tr("Opening the file…").to_string(),
                };
                ui.label(egui::RichText::new(text).color(t.text_secondary));
            });
        },
    );
}
