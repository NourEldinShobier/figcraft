//! `figcraft-cli mcp` without flags: edit the open FigCraft window when one listens on the default
//! control port, a headless copy otherwise, and keep looking: a server that started before the
//! app, or whose app restarted, attaches on a later call instead of staying detached for good.

use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use figcraft_collab_net::control_auth::DEFAULT_CONTROL_PORT;
use serde_json::{Value, json};

use crate::{Attached, Backend, Headless};

/// How often a detached server looks for the app.
const LOOK_EVERY: Duration = Duration::from_secs(3);

/// Said once per attach, in the next tool result, after the server switched auto-save on.
const AUTOSAVE_NOTE: &str = "auto-save was off: the MCP server turned it on, so the agent's changes reach the file";

pub struct Auto {
    headless: Headless,
    attached: Option<Attached>,
    /// The earliest time to look for the app again.
    next_look: Instant,
    /// The control port the app listens on.
    port: u16,
    /// Whether an app listened at the last look.
    app_listening: bool,
    /// [`Backend::target`]: ids belong to one app run, so an attach clears it.
    target: Option<Value>,
    /// The note for the next tool result ([`Backend::notice`]).
    note: Option<String>,
}

impl Default for Auto {
    fn default() -> Self {
        Self::new()
    }
}

impl Auto {
    /// Attaches on the default control port, or on `FIGCRAFT_MCP_PORT` (1-65535) when it is set.
    pub fn new() -> Self {
        Self::with_port(port_from_env(std::env::var("FIGCRAFT_MCP_PORT").ok().as_deref()))
    }

    pub fn with_port(port: u16) -> Self {
        let attached = Attached::port(port).ok();
        let app_listening = attached.is_some() || listening(port);
        let mut auto =
            Self { headless: Headless::new(), attached, next_look: Instant::now() + LOOK_EVERY, port, app_listening, target: None, note: None };
        auto.on_attach();
        auto
    }

    /// Attach when the app is there and the headless copy holds nothing unsaved; note whether it
    /// listens for [`Backend::detached`].
    fn look(&mut self) {
        if self.attached.is_some() || Instant::now() < self.next_look {
            return;
        }
        self.next_look = Instant::now() + LOOK_EVERY;
        self.app_listening = listening(self.port);
        if self.app_listening && !self.headless.has_unsaved() {
            self.attached = Attached::port(self.port).ok();
            self.on_attach();
        }
    }

    /// Just attached (or not): forget the old target, and make sure the app saves what the agent
    /// changes -- an agent's edit to a file nobody saved is lost when the app closes. Turned on
    /// unless `FIGCRAFT_MCP_NO_AUTOSAVE=1`; the next tool result says so.
    fn on_attach(&mut self) {
        self.target = None;
        self.note = None;
        let Some(app) = self.attached.as_mut() else { return };
        if !autosave_guard(std::env::var("FIGCRAFT_MCP_NO_AUTOSAVE").ok().as_deref()) {
            return;
        }
        let mut pref = |params: Value| app.call("engine.execute", json!({"command": "pref.autoSave", "params": params}));
        if pref(json!({})).is_ok_and(|v| v.get("on") == Some(&Value::Bool(false))) && pref(json!({"on": true})).is_ok() {
            self.note = Some(AUTOSAVE_NOTE.into());
        }
    }

    fn current(&mut self) -> &mut dyn Backend {
        match self.attached.as_mut() {
            Some(a) => a,
            None => &mut self.headless,
        }
    }
}

/// The port `FIGCRAFT_MCP_PORT` names; the default when it is unset, not a number or 0.
fn port_from_env(value: Option<&str>) -> u16 {
    value.and_then(|v| v.trim().parse::<u16>().ok()).filter(|p| *p != 0).unwrap_or(DEFAULT_CONTROL_PORT)
}

/// Whether the auto-save guard runs: always, except when `FIGCRAFT_MCP_NO_AUTOSAVE` is `1`.
fn autosave_guard(value: Option<&str>) -> bool {
    value.map(str::trim) != Some("1")
}

/// Whether something accepts connections on the control port.
fn listening(port: u16) -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok()
}

impl Backend for Auto {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.look();
        let r = self.current().call(method, params);
        // The app went away (its connection could not be made again): carry on in a headless copy
        // until it is back, and say so.
        match r {
            Err(e) if self.attached.is_some() && e.contains("is not reachable") => {
                self.attached = None;
                self.target = None;
                self.next_look = Instant::now();
                Err(format!("{e}. This server now works on a headless copy until the app is back: open_file the document again to continue."))
            }
            r => r,
        }
    }

    fn has_ui(&self) -> bool {
        self.attached.is_some()
    }

    fn target(&self) -> Option<Value> {
        self.target.clone()
    }

    fn set_target(&mut self, doc: Option<Value>) {
        self.target = doc;
    }

    fn notice(&mut self) -> Option<String> {
        self.note.take()
    }

    fn describe(&self) -> String {
        match &self.attached {
            Some(a) => a.describe(),
            None => format!(
                "{} (it attaches to the FigCraft app on port {} by itself once the app is open and this copy has nothing unsaved)",
                self.headless.describe(),
                self.port
            ),
        }
    }

    fn detached(&mut self) -> Option<String> {
        self.look();
        if self.attached.is_some() || !self.app_listening {
            return None;
        }
        let unsaved = self.headless.has_unsaved();
        Some(format!(
            "NOT LIVE: the FigCraft app is open on port {}, but this server edits a headless copy, so its window does not show these edits. {}",
            self.port,
            if unsaved {
                "The copy has unsaved edits: save_file (the window reloads the file and keeps its view), then call any tool; this server attaches to the app by itself."
            } else {
                "This server attaches on its next call."
            }
        ))
    }

    fn render_png(&mut self, ids: &[u64], scale: f64) -> Result<Vec<u8>, String> {
        self.look();
        self.current().render_png(ids, scale)
    }

    fn screenshot_png(&mut self) -> Result<Vec<u8>, String> {
        self.look();
        self.current().screenshot_png()
    }

    fn window_png(&mut self, window: u64) -> Result<Vec<u8>, String> {
        self.look();
        self.current().window_png(window)
    }
}

#[cfg(test)]
#[path = "auto_tests.rs"]
mod tests;
