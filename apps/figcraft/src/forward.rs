//! Opening a file while FigCraft already runs adds a tab to the running window instead of starting
//! a second app. A launch with files or an invite link (a double-click in the file manager,
//! `figcraft file`, a `figcraft://join` link) hands them to the window that listens on the default
//! control port, asks it to come to the front and exits. Anything that goes wrong (no listener, no
//! token, an error reply, no answer within [`TIMEOUT`]) leaves the files in the arguments, and the
//! launch starts as usual: nothing is lost, at worst the file opens in a window of its own.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

use figcraft_collab_net::control_auth::{ControlToken, DEFAULT_CONTROL_PORT};
use serde_json::{Value, json};

use crate::args::Args;

/// The longest the connection and each reply may take.
const TIMEOUT: Duration = Duration::from_secs(3);
/// More than a reply to our few small requests can hold.
const MAX_REPLY: u64 = 64 * 1024;

/// Whether this launch hands its files and link to a running window (`running`: something listens
/// on the default control port). A launch with no file or link starts a window of its own, as does
/// one that names its own control port or confines file access (`--automation-*-root`).
pub fn should_forward(args: &Args, running: bool) -> bool {
    running && args.control.is_none() && args.read_roots.is_empty() && args.write_roots.is_empty() && (!args.files.is_empty() || args.join.is_some())
}

/// Hand `args`' files and link to the window on the default control port. `true` when all went over
/// (the launch can end); `args` then holds what is left, which the launch opens itself.
pub fn forward_to_running(args: &mut Args) -> bool {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, DEFAULT_CONTROL_PORT));
    if !should_forward(args, true) || !should_forward(args, TcpStream::connect_timeout(&addr, TIMEOUT).is_ok()) {
        return false;
    }
    match ControlToken::for_client(DEFAULT_CONTROL_PORT) {
        Ok(token) => forward(args, addr, &token, TIMEOUT),
        Err(e) => {
            log::info!("not handing the files to a running window: {e}");
            false
        }
    }
}

/// One connection to the app's control server.
struct Client<'a> {
    out: TcpStream,
    reader: BufReader<TcpStream>,
    token: &'a ControlToken,
    next_id: u64,
}

impl<'a> Client<'a> {
    fn connect(addr: SocketAddr, token: &'a ControlToken, timeout: Duration) -> std::io::Result<Self> {
        let out = TcpStream::connect_timeout(&addr, timeout)?;
        out.set_read_timeout(Some(timeout))?;
        out.set_write_timeout(Some(timeout))?;
        let reader = BufReader::new(out.try_clone()?);
        Ok(Self { out, reader, token, next_id: 0 })
    }

    /// Send one request and say whether the app answered it with `ok`.
    fn ask(&mut self, method: &str, params: Value) -> bool {
        self.next_id += 1;
        let request = json!({"id": self.next_id, "method": method, "params": params, "token": self.token.as_str()});
        let mut reply = String::new();
        let answered = writeln!(self.out, "{request}").is_ok() && (&mut self.reader).take(MAX_REPLY).read_line(&mut reply).is_ok_and(|n| n > 0);
        let ok = answered
            && serde_json::from_str::<Value>(&reply)
                .is_ok_and(|v| v.get("id").and_then(Value::as_u64) == Some(self.next_id) && v.get("ok").and_then(Value::as_bool) == Some(true));
        if !ok {
            log::info!("the running window did not accept {method}");
        }
        ok
    }
}

/// The path as the running window can use it: its working folder is not ours.
fn absolute(path: &str) -> String {
    std::path::absolute(path).map_or_else(|_| path.to_string(), |p| p.to_string_lossy().into_owned())
}

/// Send each file (`ui.open`: queued behind the progress card and opened as a tab, as the Open
/// dialog does) and the invite link (`ui.invite`: the Join dialog) to the app at `addr`, then ask
/// its window to come to the front (`ui.focus`). What the app took is removed from `args`. `true`
/// when nothing is left.
pub fn forward(args: &mut Args, addr: SocketAddr, token: &ControlToken, timeout: Duration) -> bool {
    let Ok(mut app) = Client::connect(addr, token, timeout) else { return false };
    let files = std::mem::take(&mut args.files);
    // Stops at the first file the app refuses; it and the files after it stay.
    let sent = files.iter().take_while(|f| app.ask("ui.open", json!({"path": absolute(f)}))).count();
    args.files = files.into_iter().skip(sent).collect();
    let mut delivered = sent > 0;
    if args.files.is_empty()
        && let Some(link) = args.join.take()
    {
        if app.ask("ui.invite", json!({"link": link})) {
            delivered = true;
        } else {
            args.join = Some(link);
        }
    }
    if delivered {
        // Whether it comes to the front is up to the system; the files are open either way.
        app.ask("ui.focus", json!({}));
    }
    args.files.is_empty() && args.join.is_none()
}
