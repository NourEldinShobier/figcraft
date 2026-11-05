//! `figcraft-cli mcp` without flags: edit the open FigCraft window when one listens on the default
//! control port, a headless copy otherwise, and keep looking: a server that started before the
//! app, or whose app restarted, attaches on a later call instead of staying detached for good.

use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use figcraft_collab_net::control_auth::DEFAULT_CONTROL_PORT;
use serde_json::Value;

use crate::{Attached, Backend, Headless};

/// How often a detached server looks for the app.
const LOOK_EVERY: Duration = Duration::from_secs(3);

pub struct Auto {
    headless: Headless,
    attached: Option<Attached>,
    /// The earliest time to look for the app again.
    next_look: Instant,
    /// The control port the app listens on.
    port: u16,
    /// Whether an app listened at the last look.
    app_listening: bool,
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
        Self { headless: Headless::new(), attached, next_look: Instant::now() + LOOK_EVERY, port, app_listening }
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
                self.next_look = Instant::now();
                Err(format!("{e}. This server now works on a headless copy until the app is back: open_file the document again to continue."))
            }
            r => r,
        }
    }

    fn has_ui(&self) -> bool {
        self.attached.is_some()
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
