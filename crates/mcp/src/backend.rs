//! Where MCP tool calls end up: a control-channel method call, answered either by an in-process
//! engine session ([`crate::Headless`]) or by a running FigCraft app ([`Attached`]).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use base64::Engine as _;
use serde_json::{Value, json};

/// Something that answers the FigCraft control-channel methods (`engine.execute`,
/// `engine.commands`, `document.inspect`, `node.get`, `ui.pointer`, `ui.key`, `ui.type`,
/// `ui.render`, `ui.screenshot`, `ui.inspect`). See `docs/control-protocol.md`.
pub trait Backend {
    /// Call one method. `Ok` carries the `result`, `Err` the error message.
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String>;

    /// True when a real UI window is attached (`ui.screenshot` captures the window).
    fn has_ui(&self) -> bool;

    /// Short human description ("headless", "attached to 127.0.0.1:7979").
    fn describe(&self) -> String;

    /// A warning to put in every tool result while this backend works on a detached copy although
    /// a FigCraft app is open: the app's window does not show these edits.
    fn detached(&mut self) -> Option<String> {
        None
    }

    /// The document this session's tool calls act on when a call names none (an `id` from
    /// `doc.list`); `None`: whichever tab is in front.
    fn target(&self) -> Option<Value> {
        None
    }

    /// Set (or, with `None`, clear) the [`Backend::target`]. Backends that cannot keep one ignore it.
    fn set_target(&mut self, _doc: Option<Value>) {}

    /// A one-line note for the next tool result, given once (e.g. that the server switched the
    /// app's auto-save on).
    fn notice(&mut self) -> Option<String> {
        None
    }

    /// PNG bytes of the document: `ids` (in page coordinates, their union) or, when empty, the
    /// current page's content. The default goes through `ui.render`, the PNG coming back as data.
    fn render_png(&mut self, ids: &[u64], scale: f64) -> Result<Vec<u8>, String> {
        let mut params = json!({"scale": scale});
        if !ids.is_empty() {
            params["ids"] = json!(ids);
        } else {
            // `ui.render` without ids exports the selection; pass the page's children instead.
            let kids = page_children(self)?;
            if kids.is_empty() {
                return Err("the current page is empty: nothing to render".into());
            }
            params["ids"] = json!(kids);
        }
        decode_data(&self.call("ui.render", params)?)
    }

    /// PNG bytes of what the user sees: the app window when attached, the current viewport when
    /// headless.
    fn screenshot_png(&mut self) -> Result<Vec<u8>, String> {
        capture(self, json!({}))
    }

    /// PNG bytes of the attached app's window `window` (an id from `window.list`).
    fn window_png(&mut self, window: u64) -> Result<Vec<u8>, String> {
        capture(self, json!({"window": window}))
    }
}

/// A window capture, the PNG coming back as data (no file: an app confined to automation roots
/// writes none outside them).
fn capture<B: Backend + ?Sized>(b: &mut B, params: Value) -> Result<Vec<u8>, String> {
    decode_data(&b.call("ui.screenshot", params)?)
}

/// The ids of the current page's top-level layers.
pub fn page_children<B: Backend + ?Sized>(b: &mut B) -> Result<Vec<u64>, String> {
    let doc = b.call("document.inspect", json!({"depth": 1}))?;
    let current = doc.get("currentPage").cloned().unwrap_or(Value::Null);
    let pages = doc.get("pages").and_then(Value::as_array).cloned().unwrap_or_default();
    let page = pages.iter().find(|p| p.get("id") == Some(&current)).or_else(|| pages.first());
    Ok(page
        .and_then(|p| p.get("children"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|c| c.get("id").and_then(Value::as_u64)).collect())
        .unwrap_or_default())
}

/// Decode a `data` field (base64, optionally a data: URL) from an export result.
pub fn decode_data(v: &Value) -> Result<Vec<u8>, String> {
    let d = v.get("data").and_then(Value::as_str).ok_or("the export returned no data")?;
    let d = d.split_once(',').map(|(_, b)| b).unwrap_or(d);
    base64::engine::general_purpose::STANDARD.decode(d.trim()).map_err(|e| format!("bad base64 from the app: {e}"))
}

/// The longest reply line we read from the app (512 MiB). The app's window captures alone reach
/// 256 MiB as base64 (192 MiB PNGs); anything longer is refused without reading the rest.
const MAX_REPLY: usize = 512 * 1024 * 1024;

/// A running FigCraft app, reached through its loopback control port (`figcraft --control PORT`).
pub struct Attached {
    addr: String,
    token: figcraft_collab_net::control_auth::ControlToken,
    conn: Option<(BufReader<TcpStream>, TcpStream)>,
    next_id: u64,
    /// [`MAX_REPLY`]; smaller in tests.
    max_reply: usize,
    /// How long the app is asked to wait for each request; `None`: its own 60 s.
    timeout: Option<Duration>,
    /// [`Backend::target`].
    target: Option<Value>,
    /// How many times the app was found restarted (it makes a new token at every start): ids and
    /// the target document from before are void.
    restarts: u64,
    /// The connection has carried no request yet.
    unused: bool,
}

impl Attached {
    /// Use `token` from now on. A different one means the app restarted: count it.
    fn adopt_token(&mut self, token: figcraft_collab_net::control_auth::ControlToken) {
        if token.as_str() != self.token.as_str() {
            self.restarts += 1;
        }
        self.token = token;
    }

    /// How many times the app was found restarted since this connection was made.
    pub fn restarts(&self) -> u64 {
        self.restarts
    }

    /// Connect to `addr` (e.g. `127.0.0.1:7979`), failing fast when nothing is listening.
    pub fn connect(addr: &str) -> std::io::Result<Self> {
        let port = addr
            .rsplit(':')
            .next()
            .and_then(|p| p.parse::<u16>().ok())
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("no port in {addr}")))?;
        Self::connect_with_token(addr, figcraft_collab_net::control_auth::ControlToken::for_client(port)?)
    }

    /// Connect with an explicit validated token.
    pub fn connect_with_token(addr: &str, token: figcraft_collab_net::control_auth::ControlToken) -> std::io::Result<Self> {
        let mut r = Self {
            addr: addr.to_string(),
            token,
            conn: None,
            next_id: 1,
            max_reply: MAX_REPLY,
            timeout: None,
            target: None,
            restarts: 0,
            unused: true,
        };
        r.reconnect()?;
        Ok(r)
    }

    /// Connect to the window listening on the default control port (`figcraft` without `--control`).
    pub fn default_app() -> std::io::Result<Self> {
        Self::port(figcraft_collab_net::control_auth::DEFAULT_CONTROL_PORT)
    }

    /// Ask the app to wait up to `timeout` for each request instead of its 60 s (it caps this; an
    /// app from before this ignores it and answers within its 60 s), and read its answers as long.
    pub fn with_timeout(mut self, timeout: Duration) -> std::io::Result<Self> {
        self.timeout = Some(timeout);
        self.reconnect()?;
        Ok(self)
    }

    /// Connect to `127.0.0.1:port`.
    pub fn port(port: u16) -> std::io::Result<Self> {
        Self::connect(&format!("127.0.0.1:{port}"))
    }

    pub fn addr(&self) -> &str {
        &self.addr
    }

    fn reconnect(&mut self) -> std::io::Result<()> {
        self.conn = None;
        self.unused = true;
        let mut last = std::io::Error::new(std::io::ErrorKind::NotFound, format!("cannot resolve {}", self.addr));
        let addresses: Vec<_> = self.addr.to_socket_addrs()?.collect();
        if addresses.is_empty() {
            return Err(std::io::Error::new(std::io::ErrorKind::NotFound, format!("cannot resolve {}", self.addr)));
        }
        if addresses.iter().any(|sa| !sa.ip().is_loopback()) {
            return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "attached control connections require loopback addresses"));
        }
        for sa in addresses {
            match TcpStream::connect_timeout(&sa, Duration::from_millis(800)) {
                Ok(s) => {
                    s.set_nodelay(true).ok();
                    // The app answers within its timeout; leave headroom.
                    s.set_read_timeout(Some(self.timeout.unwrap_or(Duration::from_secs(60)).saturating_add(Duration::from_secs(30)))).ok();
                    let read = s.try_clone()?;
                    self.conn = Some((BufReader::new(read), s));
                    return Ok(());
                }
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    /// The app already closed this connection (it drops ones idle past its socket timeout), or
    /// left bytes on it nobody asked for. Checked before a request is written, when replacing the
    /// connection can't repeat anything.
    fn stale(&mut self) -> bool {
        let Some((reader, writer)) = self.conn.as_mut() else { return false };
        if !reader.buffer().is_empty() || writer.set_nonblocking(true).is_err() {
            return true;
        }
        // Nothing to read on a healthy idle connection: the peek would block.
        let idle = matches!(writer.peek(&mut [0]), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock);
        writer.set_nonblocking(false).is_err() || !idle
    }

    fn roundtrip(&mut self, line: &str) -> std::io::Result<String> {
        let max_reply = self.max_reply;
        let Some((reader, writer)) = self.conn.as_mut() else {
            return Err(std::io::Error::new(std::io::ErrorKind::NotConnected, "not connected to the app"));
        };
        writer.write_all(line.as_bytes())?;
        writer.write_all(b"\n")?;
        writer.flush()?;
        let mut reply = Vec::new();
        if reader.take((max_reply + 1) as u64).read_until(b'\n', &mut reply)? == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "control channel closed"));
        }
        if reply.last() == Some(&b'\n') {
            reply.pop();
        }
        if reply.len() > max_reply {
            let mib = max_reply / (1024 * 1024);
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("reply larger than {mib} MiB")));
        }
        String::from_utf8(reply).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}

/// Why a call to the attached app failed. Turned into a string only at the [`Backend`] boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachError {
    /// No connection could be made (unresolvable, timed out); the request was not sent.
    NotReachable(String),
    /// The connection was actively refused (nothing listens there, or the address is not loopback);
    /// the request was not sent.
    RefusedAtConnect(String),
    /// The app said its request queue is full; the request was not run.
    QueueFull(String),
    /// The request was written but no usable reply came back, so it may have run.
    /// `before_any_byte`: the app closed a fresh connection without a byte, i.e. it never took it up.
    LostReply { method: String, addr: String, detail: String, before_any_byte: bool },
    /// The app answered with an error (or a reply that cannot be used).
    App(String),
}

impl std::fmt::Display for AttachError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotReachable(s) | Self::RefusedAtConnect(s) | Self::QueueFull(s) | Self::App(s) => f.write_str(s),
            Self::LostReply { method, addr, detail, .. } => write!(
                f,
                "FigCraft control call `{method}` at {addr} failed: {detail}; the operation may have completed. Inspect the document before retrying."
            ),
        }
    }
}

/// Methods that change nothing, so repeating one after a lost reply is safe.
const READ_ONLY_METHODS: &[&str] = &[
    "document.inspect",
    "node.get",
    "ui.render",
    "ui.screenshot",
    "ui.inspect",
    "engine.commands",
    "events.poll",
    "doc.list",
    "doc.status",
    "doc.revision",
];

/// Waits before the 1st, 2nd and 3rd retry of a call the app did not take up.
const BACKOFF: [Duration; 3] = [Duration::from_millis(100), Duration::from_millis(300), Duration::from_millis(900)];

/// The app's own words for a full request queue.
fn is_queue_full(msg: &str) -> bool {
    let m = msg.to_lowercase();
    (m.contains("queue") && m.contains("full")) || (m.contains("too many") && m.contains("pending")) || m.contains("app is busy")
}

impl Attached {
    /// One try: connect if needed, write the request, read the reply.
    fn attempt(&mut self, method: &str, params: &Value) -> Result<Value, AttachError> {
        let id = self.next_id;
        self.next_id += 1;
        if self.conn.is_none() || self.stale() {
            // The app makes a new token at every start: one that restarted needs the new one.
            if let Some(token) = self
                .addr
                .rsplit(':')
                .next()
                .and_then(|p| p.parse::<u16>().ok())
                .and_then(|p| figcraft_collab_net::control_auth::ControlToken::for_client(p).ok())
            {
                self.adopt_token(token);
            }
            self.reconnect().map_err(|e| {
                let msg =
                    format!("FigCraft app at {} is not reachable: {e} (request was not sent; start it with `figcraft --control PORT`)", self.addr);
                if matches!(e.kind(), std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::PermissionDenied) {
                    AttachError::RefusedAtConnect(msg)
                } else {
                    AttachError::NotReachable(msg)
                }
            })?;
        }
        let fresh = std::mem::replace(&mut self.unused, false);
        // Built after the token was refreshed: a restarted app refuses the old one.
        let mut line = json!({"id": id, "method": method, "params": params, "token": self.token.as_str()});
        if let (Some(t), Some(fields)) = (self.timeout, line.as_object_mut()) {
            fields.insert("timeoutMs".into(), json!(u64::try_from(t.as_millis()).unwrap_or(u64::MAX)));
        }
        let line = line.to_string();
        // A lost reply does not mean the app failed to apply the request.
        let reply = self.roundtrip(&line).map_err(|e| {
            self.conn = None;
            let detail = e.to_string();
            // An oversized or undecodable reply repeats the same way: no retry, no "lost".
            if e.kind() == std::io::ErrorKind::InvalidData {
                return AttachError::App(format!(
                    "FigCraft control call `{method}` at {} failed: {detail}; the operation may have completed. Inspect the document before retrying.",
                    self.addr
                ));
            }
            AttachError::LostReply {
                method: method.into(),
                addr: self.addr.clone(),
                before_any_byte: fresh && e.kind() == std::io::ErrorKind::UnexpectedEof,
                detail,
            }
        })?;
        let v: Value = serde_json::from_str(reply.trim()).map_err(|e| AttachError::App(format!("bad reply from the app: {e}")))?;
        if v.get("ok").and_then(Value::as_bool) == Some(true) {
            Ok(v.get("result").cloned().unwrap_or(Value::Null))
        } else {
            let msg = v.get("error").and_then(Value::as_str).unwrap_or("unknown error").to_string();
            Err(if is_queue_full(&msg) { AttachError::QueueFull(msg) } else { AttachError::App(msg) })
        }
    }

    /// [`Backend::call`] with its typed error: up to 3 retries (100/300/900 ms apart) when the app's
    /// queue was full, or (read-only methods only) it dropped a fresh connection unanswered; one
    /// retry of any other lost reply of a read-only method. A lost reply of a write is never repeated.
    pub fn call_typed(&mut self, method: &str, params: Value) -> Result<Value, AttachError> {
        let (mut backoffs, mut replayed) = (0, false);
        loop {
            let err = match self.attempt(method, &params) {
                Ok(v) => return Ok(v),
                Err(e) => e,
            };
            // A dropped fresh connection is only repeated for methods that change nothing: the app
            // may have run a write before it lost the reply.
            let wait = matches!(&err, AttachError::QueueFull(_))
                || (matches!(&err, AttachError::LostReply { before_any_byte: true, .. }) && READ_ONLY_METHODS.contains(&method));
            if wait && backoffs < BACKOFF.len() {
                std::thread::sleep(BACKOFF[backoffs]);
                backoffs += 1;
            } else if !replayed && matches!(err, AttachError::LostReply { .. }) && READ_ONLY_METHODS.contains(&method) {
                replayed = true;
            } else {
                return Err(err);
            }
        }
    }
}

impl Backend for Attached {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.call_typed(method, params).map_err(|e| e.to_string())
    }

    fn has_ui(&self) -> bool {
        true
    }

    fn target(&self) -> Option<Value> {
        self.target.clone()
    }

    fn set_target(&mut self, doc: Option<Value>) {
        self.target = doc;
    }

    fn describe(&self) -> String {
        format!("attached to the FigCraft app at {}", self.addr)
    }
}

#[cfg(test)]
#[path = "backend_tests.rs"]
mod tests;
