//! The attached backend reads the app's replies with a bound (#1113).

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::Duration;

use serde_json::json;

use super::{AttachError, Attached, Backend};

/// A fake app that takes one connection per reply, reads one request on it and answers with the
/// reply. It keeps the connections open until [`finish`] (so an unterminated reply never reaches
/// EOF), then returns how many requests it read.
fn fake_app(replies: Vec<Vec<u8>>) -> (u16, mpsc::Sender<()>, std::thread::JoinHandle<usize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (done, wait) = mpsc::channel::<()>();
    let app = std::thread::spawn(move || {
        let mut open = Vec::new();
        for reply in &replies {
            let (stream, _) = listener.accept().unwrap();
            let mut out = stream.try_clone().unwrap();
            let mut line = String::new();
            BufReader::new(stream).read_line(&mut line).unwrap();
            out.write_all(reply).unwrap();
            out.flush().unwrap();
            open.push(out);
        }
        wait.recv_timeout(Duration::from_secs(30)).ok();
        open.len()
    });
    (port, done, app)
}

/// Let the fake app close its connections, and check it read `requests` requests.
fn finish(done: mpsc::Sender<()>, app: std::thread::JoinHandle<usize>, requests: usize) {
    drop(done);
    assert_eq!(app.join().unwrap(), requests);
}

fn ok_reply(text: &str) -> Vec<u8> {
    format!("{}\n", json!({"id": 1, "ok": true, "result": text})).into_bytes()
}

/// A restarted app makes a new token: the next request carries it, and the restart is counted.
#[test]
fn a_new_token_is_counted_as_a_restart_and_sent_with_the_next_request() {
    use figcraft_collab_net::control_auth::ControlToken;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut out = stream.try_clone().unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).unwrap();
        out.write_all(&ok_reply("fine")).unwrap();
        serde_json::from_str::<serde_json::Value>(&line).unwrap()["token"].clone()
    });
    let (old, new) = ("a".repeat(22), "b".repeat(22));
    let mut a = Attached::connect_with_token(&format!("127.0.0.1:{port}"), ControlToken::new(&old).unwrap()).unwrap();
    assert_eq!(a.restarts(), 0);
    a.adopt_token(ControlToken::new(&old).unwrap());
    assert_eq!(a.restarts(), 0, "the same token is not a restart");
    a.adopt_token(ControlToken::new(&new).unwrap());
    assert_eq!(a.restarts(), 1);
    a.call("engine.commands", json!({})).unwrap();
    assert_eq!(app.join().unwrap(), json!(new), "the request carries the new token");
}

/// #1696: a raised timeout goes with every request; without one the line is as it always was.
#[test]
fn a_raised_timeout_is_asked_for_on_each_request() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    // Reads the requests of two calls; `with_timeout`'s dropped connection brings none.
    let app = std::thread::spawn(move || {
        let mut lines = Vec::new();
        while lines.len() < 2 {
            let (stream, _) = listener.accept().unwrap();
            let mut out = stream.try_clone().unwrap();
            let mut line = String::new();
            if BufReader::new(stream).read_line(&mut line).unwrap() > 0 {
                out.write_all(&ok_reply("fine")).unwrap();
                lines.push(serde_json::from_str::<serde_json::Value>(&line).unwrap());
            }
        }
        lines
    });
    Attached::connect_with_token(&format!("127.0.0.1:{port}"), figcraft_collab_net::control_auth::ControlToken::new(&"a".repeat(43)).unwrap())
        .unwrap()
        .call("engine.commands", json!({}))
        .unwrap();
    let mut long =
        Attached::connect_with_token(&format!("127.0.0.1:{port}"), figcraft_collab_net::control_auth::ControlToken::new(&"a".repeat(43)).unwrap())
            .unwrap()
            .with_timeout(Duration::from_secs(300))
            .unwrap();
    long.call("engine.commands", json!({})).unwrap();
    let lines = app.join().unwrap();
    assert_eq!(lines[0].get("timeoutMs"), None, "{:?}", lines[0]);
    assert_eq!(lines[1]["timeoutMs"], 300_000, "{:?}", lines[1]);
}

#[test]
fn an_oversized_reply_is_refused_and_the_next_call_reconnects() {
    let (port, done, fake) = fake_app(vec![vec![b'x'; 200], ok_reply("fine")]);
    let mut app =
        Attached::connect_with_token(&format!("127.0.0.1:{port}"), figcraft_collab_net::control_auth::ControlToken::new(&"a".repeat(43)).unwrap())
            .unwrap();
    app.max_reply = 64;
    let err = app.call("engine.commands", json!({})).unwrap_err();
    assert!(err.contains("reply larger than"), "{err}");
    assert!(err.contains("may have completed"), "{err}");
    assert!(app.conn.is_none());
    assert_eq!(app.call("engine.commands", json!({})).unwrap(), json!("fine"));
    finish(done, fake, 2);
}

#[test]
fn a_reply_at_the_limit_is_read() {
    let reply = ok_reply("at the limit");
    let (port, done, fake) = fake_app(vec![reply.clone()]);
    let mut app =
        Attached::connect_with_token(&format!("127.0.0.1:{port}"), figcraft_collab_net::control_auth::ControlToken::new(&"a".repeat(43)).unwrap())
            .unwrap();
    app.max_reply = reply.len() - 1; // the newline doesn't count
    assert_eq!(app.call("engine.commands", json!({})).unwrap(), json!("at the limit"));
    finish(done, fake, 1);
}

#[test]
fn a_reply_that_is_not_utf8_is_an_error() {
    let (port, done, fake) = fake_app(vec![b"{\"ok\": true, \"result\": \"\xff\"}\n".to_vec()]);
    let mut app =
        Attached::connect_with_token(&format!("127.0.0.1:{port}"), figcraft_collab_net::control_auth::ControlToken::new(&"a".repeat(43)).unwrap())
            .unwrap();
    let err = app.call("engine.commands", json!({})).unwrap_err();
    assert!(err.contains("failed"), "{err}");
    assert!(app.conn.is_none());
    finish(done, fake, 1);
}

/// What the scripted fake app does with each request it reads.
enum Step {
    Reply(Vec<u8>),
    /// Read the request, then close the connection without a byte.
    Close,
}

/// A fake app that follows `steps`, one per request, on whatever connections arrive; returns the
/// port and the number of requests read.
fn scripted(steps: Vec<Step>) -> (u16, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = std::sync::Arc::new(AtomicUsize::new(0));
    let count = seen.clone();
    std::thread::spawn(move || {
        let mut steps: std::collections::VecDeque<Step> = steps.into();
        while !steps.is_empty() {
            let Ok((stream, _)) = listener.accept() else { return };
            let mut out = stream.try_clone().unwrap();
            let mut reader = BufReader::new(stream);
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                count.fetch_add(1, Ordering::SeqCst);
                match steps.pop_front() {
                    Some(Step::Reply(r)) => {
                        out.write_all(&r).unwrap();
                        out.flush().unwrap();
                    }
                    Some(Step::Close) | None => break,
                }
                if steps.is_empty() {
                    // Let the client read the last reply before the connection goes.
                    std::thread::sleep(Duration::from_millis(200));
                    return;
                }
            }
        }
    });
    (port, seen)
}

fn err_reply(msg: &str) -> Step {
    Step::Reply(format!("{}\n", json!({"id": 1, "ok": false, "error": msg})).into_bytes())
}

fn attach(port: u16) -> Attached {
    Attached::connect_with_token(&format!("127.0.0.1:{port}"), figcraft_collab_net::control_auth::ControlToken::new(&"a".repeat(43)).unwrap())
        .unwrap()
}

fn requests(seen: &std::sync::atomic::AtomicUsize) -> usize {
    seen.load(std::sync::atomic::Ordering::SeqCst)
}

#[test]
fn a_full_queue_is_retried_with_backoff_and_then_succeeds() {
    let (port, seen) = scripted(vec![err_reply("the request queue is full"), err_reply("the request queue is full"), Step::Reply(ok_reply("done"))]);
    let started = std::time::Instant::now();
    assert_eq!(attach(port).call_typed("engine.execute", json!({})).unwrap(), json!("done"));
    assert_eq!(requests(&seen), 3);
    assert!(started.elapsed() >= Duration::from_millis(400), "waited 100 then 300 ms: {:?}", started.elapsed());
}

#[test]
fn a_full_queue_gives_up_after_three_retries() {
    let (port, seen) = scripted((0..4).map(|_| err_reply("too many screenshots are pending (at most 4)")).collect());
    let e = attach(port).call_typed("ui.screenshot", json!({})).unwrap_err();
    assert!(matches!(e, AttachError::QueueFull(_)), "{e:?}");
    assert_eq!(requests(&seen), 4, "one try and three retries");
}

#[test]
fn an_app_error_is_not_retried() {
    let (port, seen) = scripted(vec![err_reply("no such layer"), Step::Reply(ok_reply("unused"))]);
    let e = attach(port).call_typed("engine.execute", json!({})).unwrap_err();
    assert_eq!(e, AttachError::App("no such layer".into()));
    assert_eq!(requests(&seen), 1);
}

#[test]
fn a_fresh_connection_closed_unanswered_is_retried_for_reads_but_not_writes() {
    let (port, seen) = scripted(vec![Step::Close, Step::Reply(ok_reply("done"))]);
    assert_eq!(attach(port).call_typed("document.inspect", json!({})).unwrap(), json!("done"));
    assert_eq!(requests(&seen), 2);

    let (port, seen) = scripted(vec![Step::Close, Step::Reply(ok_reply("done"))]);
    let e = attach(port).call_typed("engine.execute", json!({})).unwrap_err();
    assert!(matches!(e, AttachError::LostReply { before_any_byte: true, .. }), "{e:?}");
    assert_eq!(requests(&seen), 1, "a write the app may have run is not sent again");
}

#[test]
fn a_lost_reply_on_a_used_connection_is_replayed_only_for_read_only_methods() {
    // Write: the second call loses its reply and reports it.
    let (port, seen) = scripted(vec![Step::Reply(ok_reply("a")), Step::Close, Step::Reply(ok_reply("c"))]);
    let mut app = attach(port);
    app.call_typed("engine.execute", json!({})).unwrap();
    let e = app.call_typed("engine.execute", json!({})).unwrap_err();
    assert!(matches!(e, AttachError::LostReply { before_any_byte: false, .. }), "{e:?}");
    assert!(e.to_string().contains("may have completed"), "{e}");
    assert_eq!(requests(&seen), 2, "not repeated");

    // Read-only: the same loss is repeated once, on a new connection.
    let (port, seen) = scripted(vec![Step::Reply(ok_reply("a")), Step::Close, Step::Reply(ok_reply("c"))]);
    let mut app = attach(port);
    app.call_typed("node.get", json!({})).unwrap();
    assert_eq!(app.call_typed("node.get", json!({})).unwrap(), json!("c"));
    assert_eq!(requests(&seen), 3);

    // ... but only once.
    let (port, seen) = scripted(vec![Step::Reply(ok_reply("a")), Step::Close, Step::Close, Step::Reply(ok_reply("d"))]);
    let mut app = attach(port);
    app.call_typed("doc.list", json!({})).unwrap();
    // The replay's connection is fresh, so its silent close counts as "never taken up" and backs off.
    assert_eq!(app.call_typed("doc.list", json!({})).unwrap(), json!("d"));
    assert_eq!(requests(&seen), 4);
}

#[test]
fn a_gone_app_is_not_reachable() {
    let (port, _seen) = scripted(vec![Step::Close]);
    let mut app = attach(port);
    // The scripted app ends after one request and its listener is dropped with the thread.
    assert!(app.call_typed("engine.execute", json!({})).is_err());
    std::thread::sleep(Duration::from_millis(300));
    let e = app.call_typed("engine.execute", json!({})).unwrap_err();
    assert!(matches!(e, AttachError::RefusedAtConnect(_) | AttachError::NotReachable(_)), "{e:?}");
    assert!(e.to_string().contains("is not reachable"), "{e}");
}
