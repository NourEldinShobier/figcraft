#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! A launch with files or a link hands them to the running window (a fake app here) and leaves
//! them in the arguments when that fails.

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener};
use std::time::Duration;

use figcraft_collab_net::control_auth::ControlToken;
use serde_json::{Value, json};

use crate::args::Args;
use crate::forward::{forward, should_forward};

const TIMEOUT: Duration = Duration::from_secs(5);

fn token() -> ControlToken {
    ControlToken::new(&"a".repeat(43)).unwrap()
}

fn files(names: &[&str]) -> Vec<String> {
    names.iter().map(ToString::to_string).collect()
}

/// An absolute path on the platform running the tests.
fn abs(name: &str) -> String {
    if cfg!(windows) { format!("C:\\work\\{name}") } else { format!("/work/{name}") }
}

/// A fake app: serves one connection, answers every request with `ok` except the `refuse`th (counted
/// from 0), which it answers with an error. Returns the requests it read once the client hangs up.
fn fake_app(refuse: Option<usize>) -> (SocketAddr, std::thread::JoinHandle<Vec<Value>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let app = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut out = stream.try_clone().unwrap();
        let mut seen = Vec::new();
        for line in BufReader::new(stream).lines() {
            let request: Value = serde_json::from_str(&line.unwrap()).unwrap();
            let reply = if Some(seen.len()) == refuse {
                json!({"id": request["id"], "ok": false, "error": "refused"})
            } else {
                json!({"id": request["id"], "ok": true, "result": {"queued": true}})
            };
            writeln!(out, "{reply}").unwrap();
            seen.push(request);
        }
        seen
    });
    (addr, app)
}

fn methods(seen: &[Value]) -> Vec<&str> {
    seen.iter().map(|r| r["method"].as_str().unwrap()).collect()
}

#[test]
fn only_a_launch_with_a_file_or_link_for_a_running_window_is_forwarded() {
    let with_file = Args { files: files(&["a.figcraft"]), ..Args::default() };
    let with_link = Args { join: Some("figcraft://join?host=h".into()), ..Args::default() };
    assert!(should_forward(&with_file, true));
    assert!(should_forward(&with_link, true));
    assert!(!should_forward(&with_file, false), "no window is running");
    assert!(!should_forward(&Args::default(), true), "a bare launch opens its own window");
    assert!(!should_forward(&Args { control: Some(4242), ..with_file }, true), "its own control port");
    let confined = Args { read_roots: files(&["/data"]), files: files(&["a.figcraft"]), ..Args::default() };
    assert!(!should_forward(&confined, true), "file access is confined");
    let confined = Args { write_roots: files(&["/data"]), join: Some("figcraft://join?host=h".into()), ..Args::default() };
    assert!(!should_forward(&confined, true));
}

#[test]
fn files_then_the_link_go_to_the_window_which_is_then_brought_forward() {
    let (addr, app) = fake_app(None);
    let mut args = Args { files: vec![abs("a.figcraft"), abs("b.svg")], join: Some("figcraft://join?host=h&file=f".into()), ..Args::default() };
    assert!(forward(&mut args, addr, &token(), TIMEOUT));
    assert_eq!((args.files, args.join), (Vec::new(), None), "nothing is left to open here");
    let seen = app.join().unwrap();
    assert_eq!(methods(&seen), ["ui.open", "ui.open", "ui.invite", "ui.focus"]);
    assert_eq!(seen[0]["params"]["path"], abs("a.figcraft"));
    assert_eq!(seen[1]["params"]["path"], abs("b.svg"));
    assert_eq!(seen[2]["params"]["link"], "figcraft://join?host=h&file=f");
    assert!(seen.iter().all(|r| r["token"] == "a".repeat(43)), "every request carries the token");
}

#[test]
fn a_relative_path_is_sent_as_an_absolute_one() {
    let (addr, app) = fake_app(None);
    let mut args = Args { files: files(&["rel.figcraft"]), ..Args::default() };
    assert!(forward(&mut args, addr, &token(), TIMEOUT));
    let seen = app.join().unwrap();
    let sent = seen[0]["params"]["path"].as_str().unwrap();
    assert!(std::path::Path::new(sent).is_absolute(), "{sent}");
    assert!(sent.ends_with("rel.figcraft"), "{sent}");
}

#[test]
fn a_refused_file_stays_for_this_launch_to_open_with_the_ones_after_it() {
    let (addr, app) = fake_app(Some(1));
    let mut args =
        Args { files: vec![abs("a.figcraft"), abs("b.figcraft"), abs("c.figcraft")], join: Some("figcraft://join?host=h".into()), ..Args::default() };
    assert!(!forward(&mut args, addr, &token(), TIMEOUT));
    assert_eq!(args.files, vec![abs("b.figcraft"), abs("c.figcraft")]);
    assert_eq!(args.join.as_deref(), Some("figcraft://join?host=h"), "the link was not tried after a refusal");
    let seen = app.join().unwrap();
    assert_eq!(methods(&seen), ["ui.open", "ui.open", "ui.focus"], "the first file arrived, so the window is brought forward");
}

#[test]
fn a_refused_link_stays_for_this_launch() {
    let (addr, app) = fake_app(Some(0));
    let mut args = Args { join: Some("figcraft://join?host=h".into()), ..Args::default() };
    assert!(!forward(&mut args, addr, &token(), TIMEOUT));
    assert_eq!(args.join.as_deref(), Some("figcraft://join?host=h"));
    assert_eq!(methods(&app.join().unwrap()), ["ui.invite"], "nothing arrived: no focus request");
}

#[test]
fn nothing_listening_leaves_the_arguments_alone() {
    // A port that was free a moment ago.
    let addr = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    let mut args = Args { files: files(&["a.figcraft"]), join: Some("figcraft://join?host=h".into()), ..Args::default() };
    assert!(!forward(&mut args, addr, &token(), Duration::from_millis(500)));
    assert_eq!(args.files, files(&["a.figcraft"]));
    assert_eq!(args.join.as_deref(), Some("figcraft://join?host=h"));
}

#[test]
fn an_app_that_never_answers_is_given_up_on_after_the_timeout() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    // Reads the request and says nothing; the stream stays open until the test ends.
    let silent = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(stream.try_clone().unwrap()).read_line(&mut line).unwrap();
        stream
    });
    let mut args = Args { files: files(&["a.figcraft"]), ..Args::default() };
    let started = std::time::Instant::now();
    assert!(!forward(&mut args, addr, &token(), Duration::from_millis(300)));
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(args.files, files(&["a.figcraft"]));
    drop(silent.join().unwrap());
}
