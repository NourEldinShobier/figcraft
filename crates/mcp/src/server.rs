//! JSON-RPC 2.0 framing and the MCP lifecycle / tools / resources / prompts methods.

use std::collections::{HashSet, VecDeque};
use std::io::{BufRead, Read, Write};
use std::sync::Arc;
use std::time::Instant;

use figcraft_engine::file_access::{Roots, confined};

use serde_json::{Value, json};

use crate::backend::Backend;
use crate::prompts;
use crate::resources;
use crate::tools::{call_tool, tool_definitions};

/// The MCP revision we implement.
pub const PROTOCOL_VERSION: &str = "2025-06-18";
/// Revisions we also accept from clients.
pub const SUPPORTED_VERSIONS: &[&str] = &[PROTOCOL_VERSION, "2025-03-26", "2024-11-05"];

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;
pub const RESOURCE_NOT_FOUND: i64 = -32002;

/// The longest line we accept (32 MiB); longer lines are rejected without parsing.
const MAX_LINE: usize = 32 * 1024 * 1024;

/// The `instructions` string sent on initialize.
pub const INSTRUCTIONS: &str = "FigCraft is an interface design tool (Figma-class): pages hold frames, \
groups, shapes, text, components and instances, with auto layout, styles, variables and prototyping. \
Coordinates are page units (1 unit = 1 px at 100% zoom), x right and y DOWN. create_node and set_properties \
take x/y RELATIVE TO THE PARENT (the page when there is no parent, so page coordinates); inspect_document \
reports both x/y relative to the parent frame (groups don't count) and absoluteBounds in page coordinates. Children of an auto-layout frame are \
positioned by the layout (set layout, gap, padding, mainAlign, crossAlign, sizingH/sizingV instead). \
Every menu item, panel action and shortcut is a command: find ids and params with list_commands (use a filter) \
and run them with run_command, or several as one undo step with batch. Commands act on `ids` when given, else \
on the selection; new layers become the selection. Colours are \"#RRGGBB\" or \"#RRGGBBAA\". Verify your work: \
inspect_document / get_node return the layer tree as JSON (check sizes, fills, layout, overrides), and \
screenshot returns a PNG of the page or of given layers to look at. undo/redo step through history. \
Cheap reads that touch neither the selection nor the view: find_nodes (search), get_nodes (an outline of many layers), \
contact_sheet (many frames on one PNG), get_selection (what the user has selected) and diagnostics (backend and open files). \
Every command also takes `where`, `list`, `dryRun` and `keepSelection`. For loops and conditions over many layers, run_script runs one script as one undo step. Comments: comment.list returns threads (text, anchor node id, frame, page, x/y, replies); fix open ones, \
reply with what changed, then resolve (a reply reopens a thread). To react to new feedback, call \
wait_for_changes {after, timeoutMs} and pass the returned cursor back. \
Documents: with the app attached, open_file opens a file in the background and later calls act on it (the target \
document, use_document sets another) without touching the user's tab; `document` names another file for one call, \
and `documents` (\"all\" or a list) on find_nodes, set_properties, run_command and get_nodes runs it in each file, \
e.g. to change every screen that has an element; copy_between_documents moves layers across files. The same element repeated on several screens is a family: find_similar lists a layer's twins, set_properties results show them as similarOn, and propagate_change (previewed until confirm) copies a change to them. \
Call checkpoint before a big change (restore_version goes back). \
Resources: figcraft://document, figcraft://node/{id}, figcraft://command/{id}, figcraft://comments. \
prompts/list has workflows, including fix_comments.";

/// An MCP server bound to one backend.
pub struct Server {
    backend: Box<dyn Backend>,
    initialized: bool,
    /// The automation roots every request's file access is confined to (`--automation-read-root`,
    /// `--automation-write-root`), if any.
    roots: Option<Arc<Roots>>,
    /// The client's name from `initialize` (`clientInfo.title`, else `.name`; empty until then):
    /// every command runs as this AI agent, so its comments are signed with it, not the user's name.
    agent: String,
    /// Ids (as JSON text) of requests the client cancelled before they started.
    cancelled: HashSet<String>,
    /// Requests read but not started (only the concurrent loop sees any).
    queued: usize,
    /// Requests dropped unstarted because the client cancelled them.
    dropped: u64,
    /// The last finished tool call: name, milliseconds, whether it failed.
    last_call: Option<(String, u128, bool)>,
}

/// Tools that can run long enough for a client to want progress notifications.
const PROGRESS_TOOLS: &[&str] = &["run_script", "import_html", "export_many"];

/// One framed line from the client.
enum Frame {
    Line(Vec<u8>),
    TooLong,
}

/// The next line of `input`, without its newline; `None` at the end.
fn read_frame(input: &mut impl BufRead) -> std::io::Result<Option<Frame>> {
    let mut line = Vec::new();
    if input.take((MAX_LINE + 1) as u64).read_until(b'\n', &mut line)? == 0 {
        return Ok(None);
    }
    if line.last() == Some(&b'\n') {
        line.pop();
    }
    Ok(Some(if line.len() > MAX_LINE { Frame::TooLong } else { Frame::Line(line) }))
}

/// The backend as the MCP client's AI agent: `engine.execute` carries its name (`agent`).
struct AsAgent<'a> {
    inner: &'a mut dyn Backend,
    agent: &'a str,
}

impl Backend for AsAgent<'_> {
    fn call(&mut self, method: &str, mut params: Value) -> Result<Value, String> {
        if method == "engine.execute"
            && let Some(p) = params.as_object_mut()
        {
            p.insert("agent".into(), json!(self.agent));
        }
        self.inner.call(method, params)
    }

    fn has_ui(&self) -> bool {
        self.inner.has_ui()
    }

    fn describe(&self) -> String {
        self.inner.describe()
    }

    fn detached(&mut self) -> Option<String> {
        self.inner.detached()
    }

    fn target(&self) -> Option<Value> {
        self.inner.target()
    }

    fn set_target(&mut self, doc: Option<Value>) {
        self.inner.set_target(doc);
    }

    fn notice(&mut self) -> Option<String> {
        self.inner.notice()
    }

    fn render_png(&mut self, ids: &[u64], scale: f64) -> Result<Vec<u8>, String> {
        self.inner.render_png(ids, scale)
    }

    fn screenshot_png(&mut self) -> Result<Vec<u8>, String> {
        self.inner.screenshot_png()
    }

    fn window_png(&mut self, window: u64) -> Result<Vec<u8>, String> {
        self.inner.window_png(window)
    }
}

fn response(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message.into()}})
}

/// The name an `initialize` request gives its client (`clientInfo.title`, else `.name`; empty
/// without one). The engine keeps 64 characters of it; this only bounds what each call carries.
fn client_name(params: &Value) -> String {
    let name = |k: &str| params.get("clientInfo").and_then(|i| i.get(k)).and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty());
    name("title").or_else(|| name("name")).unwrap_or_default().chars().take(256).collect()
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload.downcast_ref::<&str>().map(ToString::to_string).or_else(|| payload.downcast_ref::<String>().cloned()).unwrap_or_else(|| "panic".into())
}

impl Server {
    pub fn new(backend: Box<dyn Backend>) -> Self {
        Self { backend, initialized: false, roots: None, agent: String::new(), cancelled: HashSet::new(), queued: 0, dropped: 0, last_call: None }
    }

    /// Confine the files every request reads and writes to `roots` (see
    /// [`figcraft_engine::file_access`]).
    pub fn with_roots(mut self, roots: Option<Roots>) -> Self {
        self.roots = roots.map(Arc::new);
        self
    }

    /// A server over a fresh headless session.
    pub fn headless() -> Self {
        Self::new(Box::new(crate::Headless::new()))
    }

    pub fn backend(&mut self) -> &mut dyn Backend {
        self.backend.as_mut()
    }

    /// Whether the client has sent `notifications/initialized`.
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Serve newline-delimited JSON-RPC until `input` closes. stdout is the protocol stream; logs
    /// go to stderr. An oversized frame gets an error and closes the stream without draining it.
    /// Requests run in the order they arrive; a `notifications/cancelled` is seen only after the
    /// request it names (use [`Server::serve_concurrent`] to cancel queued requests).
    pub fn serve(&mut self, mut input: impl BufRead, mut output: impl Write) -> std::io::Result<()> {
        while let Some(frame) = read_frame(&mut input)? {
            if !self.process(frame, &mut output)? {
                break;
            }
        }
        Ok(())
    }

    /// Like [`Server::serve`], but a thread reads `input` while a request runs: a
    /// `notifications/cancelled` that arrives meanwhile drops the request it names if that has not
    /// started yet (a request already running cannot be interrupted: the backend call blocks), and
    /// `diagnostics` shows how many requests wait.
    pub fn serve_concurrent(&mut self, input: impl Read + Send + 'static, mut output: impl Write) -> std::io::Result<()> {
        let (tx, rx) = std::sync::mpsc::sync_channel::<std::io::Result<Frame>>(256);
        std::thread::spawn(move || {
            let mut input = std::io::BufReader::new(input);
            loop {
                let frame = match read_frame(&mut input) {
                    Ok(Some(f)) => Ok(f),
                    Ok(None) => return,
                    Err(e) => Err(e),
                };
                let stop = frame.is_err() || matches!(frame, Ok(Frame::TooLong));
                if tx.send(frame).is_err() || stop {
                    return;
                }
            }
        });
        let mut backlog: VecDeque<Frame> = VecDeque::new();
        loop {
            if backlog.is_empty() {
                match rx.recv() {
                    Ok(f) => backlog.push_back(f?),
                    Err(_) => break,
                }
            }
            while let Ok(f) = rx.try_recv() {
                backlog.push_back(f?);
            }
            for f in &backlog {
                if let Frame::Line(l) = f {
                    self.note_cancel(&String::from_utf8_lossy(l));
                }
            }
            let Some(frame) = backlog.pop_front() else { continue };
            self.queued = backlog.len();
            if !self.process(frame, &mut output)? {
                break;
            }
        }
        Ok(())
    }

    /// Handle one frame and write its replies; false when the stream must close.
    fn process(&mut self, frame: Frame, output: &mut impl Write) -> std::io::Result<bool> {
        let Frame::Line(line) = frame else {
            writeln!(output, "{}", error(Value::Null, INVALID_REQUEST, "message too large"))?;
            output.flush()?;
            return Ok(false);
        };
        let text = String::from_utf8_lossy(&line);
        let mut failed = None;
        let reply = self.handle_line_with(&text, &mut |note| {
            if failed.is_none() {
                failed = writeln!(output, "{note}").and_then(|()| output.flush()).err();
            }
        });
        if let Some(e) = failed {
            return Err(e);
        }
        if let Some(reply) = reply {
            output.write_all(reply.as_bytes())?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
        Ok(true)
    }

    /// Remember the request a `notifications/cancelled` line names (it is queued behind the
    /// running one, or already finished).
    fn note_cancel(&mut self, line: &str) {
        if !line.contains("notifications/cancelled") {
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { return };
        if v.get("method").and_then(Value::as_str) == Some("notifications/cancelled")
            && let Some(id) = v.pointer("/params/requestId").filter(|i| i.is_string() || i.is_number())
        {
            if self.cancelled.len() >= 256 {
                self.cancelled.clear();
            }
            self.cancelled.insert(id.to_string());
        }
    }

    /// Handle one line; returns the reply line (None for notifications and blank lines).
    pub fn handle_line(&mut self, line: &str) -> Option<String> {
        self.handle_line_with(line, &mut |_| {})
    }

    /// [`Server::handle_line`], handing notifications the server sends while it works (progress)
    /// to `emit` as they happen.
    pub fn handle_line_with(&mut self, line: &str, emit: &mut dyn FnMut(Value)) -> Option<String> {
        if line.len() > MAX_LINE {
            return Some(error(Value::Null, INVALID_REQUEST, "message too large").to_string());
        }
        let line = line.trim();
        if line.is_empty() {
            return None;
        }
        let reply = match serde_json::from_str::<Value>(line) {
            Ok(Value::Array(batch)) => {
                // Batches were removed in 2025-06-18; still answer older clients sensibly.
                if batch.is_empty() {
                    Some(error(Value::Null, INVALID_REQUEST, "empty batch"))
                } else {
                    let replies: Vec<Value> = batch.into_iter().take(1000).filter_map(|m| self.handle_with(m, emit)).collect();
                    (!replies.is_empty()).then_some(Value::Array(replies))
                }
            }
            Ok(msg) => self.handle_with(msg, emit),
            Err(e) => Some(error(Value::Null, PARSE_ERROR, format!("parse error: {e}"))),
        };
        reply.map(|r| r.to_string())
    }

    /// Handle one JSON-RPC message; `None` for notifications and responses.
    pub fn handle(&mut self, msg: Value) -> Option<Value> {
        self.handle_with(msg, &mut |_| {})
    }

    fn handle_with(&mut self, msg: Value, emit: &mut dyn FnMut(Value)) -> Option<Value> {
        let Value::Object(o) = &msg else { return Some(error(Value::Null, INVALID_REQUEST, "message must be a JSON object")) };
        let id = o.get("id").cloned();
        let Some(method) = o.get("method").and_then(Value::as_str) else {
            // A response to a server→client request (we send none): ignore.
            if o.contains_key("result") || o.contains_key("error") {
                return None;
            }
            return Some(error(id.filter(|i| i.is_string() || i.is_number()).unwrap_or(Value::Null), INVALID_REQUEST, "missing `method` (a string)"));
        };
        let params = o.get("params").cloned().unwrap_or(Value::Null);
        let Some(id) = id else {
            self.notification(method);
            return None;
        };
        if !(id.is_string() || id.is_number()) {
            return Some(error(Value::Null, INVALID_REQUEST, "`id` must be a string or number"));
        }
        if !(params.is_object() || params.is_null()) {
            return Some(error(id, INVALID_PARAMS, "`params` must be an object"));
        }
        // The client cancelled it while it waited: no work and, per the spec, no reply.
        if method != "initialize" && self.cancelled.remove(&id.to_string()) {
            self.dropped += 1;
            return None;
        }
        // A bug that panics fails this request only; the server keeps serving.
        let roots = self.roots.clone();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| confined(roots.as_ref(), || self.request(method, &params, &id, emit))))
            .unwrap_or_else(|p| {
                Err((INTERNAL_ERROR, format!("internal error in `{method}`: {} (please report this bug)", panic_message(p.as_ref()))))
            });
        Some(match r {
            Ok(r) => response(id, r),
            Err((code, m)) => error(id, code, m),
        })
    }

    /// `diagnostics`: the request being served, the ones waiting behind it, the last finished call.
    fn add_request_state(&self, r: &mut crate::tools::ToolResult, id: &Value, name: &str) {
        let Some(Ok(Value::Object(mut v))) = r.content.first().and_then(|c| c.get("text")).and_then(Value::as_str).map(serde_json::from_str::<Value>)
        else {
            return;
        };
        let last = self.last_call.as_ref().map(|(t, ms, failed)| json!({"tool": t, "ms": ms, "failed": failed}));
        v.insert(
            "requests".into(),
            json!({"inFlight": {"id": id, "tool": name}, "queued": self.queued, "cancelledBeforeStart": self.dropped, "lastFinished": last}),
        );
        *r = crate::tools::ToolResult::json(&Value::Object(v));
    }

    /// What `initialize` says about the automation roots.
    fn roots_note(&self) -> String {
        let Some(r) = &self.roots else { return String::new() };
        let j = r.to_json();
        format!(" Files: only these folders may be read: {}; written: {} (any other path is refused).", j["read"], j["write"])
    }

    fn notification(&mut self, method: &str) {
        match method {
            "notifications/initialized" => self.initialized = true,
            "notifications/cancelled" | "notifications/progress" | "notifications/roots/list_changed" => {}
            other => log::debug!("ignoring notification {other}"),
        }
    }

    /// The backend, acting as the client's agent.
    fn as_agent(&mut self) -> AsAgent<'_> {
        AsAgent { inner: self.backend.as_mut(), agent: &self.agent }
    }

    fn request(&mut self, method: &str, params: &Value, id: &Value, emit: &mut dyn FnMut(Value)) -> Result<Value, (i64, String)> {
        match method {
            "initialize" => {
                self.agent = client_name(params);
                let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOL_VERSION);
                let version = if SUPPORTED_VERSIONS.contains(&asked) { asked } else { PROTOCOL_VERSION };
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": {
                        "tools": {"listChanged": false},
                        "resources": {"subscribe": false, "listChanged": false},
                        "prompts": {"listChanged": false},
                    },
                    "serverInfo": {"name": "figcraft", "title": "FigCraft", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": format!("{INSTRUCTIONS} Backend: {}.{}", self.backend.describe(), self.roots_note()),
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tool_definitions()})),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).ok_or((INVALID_PARAMS, "missing tool `name` (a string)".to_string()))?;
                let args = params.get("arguments").cloned().unwrap_or(Value::Null);
                if !(args.is_object() || args.is_null()) {
                    return Err((INVALID_PARAMS, "`arguments` must be an object".into()));
                }
                if !crate::tools::TOOL_NAMES.contains(&name) {
                    return Err((INVALID_PARAMS, format!("unknown tool `{name}`")));
                }
                let token = params.pointer("/_meta/progressToken").filter(|t| t.is_string() || t.is_number()).cloned();
                let progress = |emit: &mut dyn FnMut(Value), done: bool| {
                    if let Some(t) = token.as_ref().filter(|_| PROGRESS_TOOLS.contains(&name)) {
                        let message = if done { format!("{name} finished") } else { format!("{name} started") };
                        emit(
                            json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": {"progressToken": t, "progress": u8::from(done), "total": 1, "message": message}}),
                        );
                    }
                };
                let started = Instant::now();
                progress(emit, false);
                let mut r = call_tool(&mut self.as_agent(), name, &args);
                progress(emit, true);
                if name == "diagnostics" && !r.is_error {
                    self.add_request_state(&mut r, id, name);
                }
                self.last_call = Some((name.to_string(), started.elapsed().as_millis(), r.is_error));
                Ok(r.to_value())
            }
            "resources/list" => Ok(resources::list()),
            "resources/templates/list" => Ok(resources::templates()),
            "resources/read" => {
                let uri = params.get("uri").and_then(Value::as_str).ok_or((INVALID_PARAMS, "missing `uri` (a string)".to_string()))?;
                let v = resources::read(&mut self.as_agent(), uri).map_err(|e| match e {
                    resources::ReadError::NotFound(uri) => (RESOURCE_NOT_FOUND, format!("resource not found: {uri}")),
                    resources::ReadError::Backend(why) => (INTERNAL_ERROR, why),
                })?;
                Ok(json!({"contents": [{"uri": uri, "mimeType": "application/json", "text": serde_json::to_string_pretty(&v).unwrap_or_default()}]}))
            }
            "prompts/list" => Ok(prompts::list()),
            "prompts/get" => {
                let name = params.get("name").and_then(Value::as_str).ok_or((INVALID_PARAMS, "missing prompt `name` (a string)".to_string()))?;
                let args = params.get("arguments").cloned().unwrap_or(Value::Null);
                prompts::get(name, &args).map_err(|e| (INVALID_PARAMS, e))
            }
            "logging/setLevel" => Ok(json!({})),
            other => Err((METHOD_NOT_FOUND, format!("method not found: {other}"))),
        }
    }
}
