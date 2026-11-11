//! `script.run`: write to the document by code. One call runs a Rhai script (a small pure-Rust
//! scripting language, so it works in the browser build too) with loops and conditions, calling
//! any engine command through `cmd`, reading with `find` and `get`. The whole script is one undo
//! step and leaves the document untouched when it fails.
//!
//! The sandbox: no modules (`import`), no `eval`, no clock, no files (the commands a plugin may
//! not call are closed to scripts too), and five budgets: operations, commands, data size, call depth
//! and the seconds the commands may take together (a script that loops over a large file would
//! otherwise freeze the app: commands past it fail). The other four do not depend on wall-clock time.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use figcraft_doc::NodeId;
use rhai::{Array, Dynamic, Engine, EvalAltResult, ImmutableString, Map, Position};
use serde_json::{Value, json};

use super::CommandSpec;
use crate::extensions::plugins::sandbox_refusal;
use crate::params::get;
use crate::{EngineError, Result, Session, bad, guard, spec};

const CMD: &str = "script.run";
const DEFAULT_OPERATIONS: u64 = 200_000;
const MAX_OPERATIONS: u64 = 5_000_000;
const MAX_CODE_BYTES: usize = 100_000;
/// Commands one script may run.
const MAX_COMMANDS: u32 = 10_000;
/// Seconds a script may spend in its commands before they fail (one second in tests).
const MAX_SECONDS: u64 = if cfg!(test) { 1 } else { 20 };
/// Log lines kept, and the longest one in characters.
const MAX_LOG_LINES: usize = 200;
const MAX_LOG_CHARS: usize = 1_000;
/// Created layer ids listed in the result.
const MAX_LISTED: usize = 500;
/// Elements one array holds, entries one map holds.
const MAX_ARRAY: usize = 1_000;
const MAX_MAP: usize = 250;
/// Deepest nesting of arrays and maps passed between the script and the commands.
const MAX_DEPTH: usize = 32;
/// Log lines of a failed script that join its error.
const ERROR_LOG_LINES: usize = 10;

pub fn specs() -> Vec<CommandSpec> {
    vec![spec!(
        "script.run",
        "Run a script",
        "{code, maxOperations? (default 200000, at most 5000000)}  runs a Rhai script as one undo step; any error undoes it all. Host functions: cmd(id, params_map) runs an engine command and returns its result (an error stops the script), cmd(id) the same without params, find(filters_map) the ids node.find finds, get(id) the layer as a map, log(text). Last expression is the result. Maps are `#{x: 10, name: \"A\"}`. No files, no import, no eval, at most 10000 commands → {value, logs, created: [ids], changed, deleted, commands, operations}; `dryRun` reports what it would change and undoes it",
        false,
        run
    )]
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    if get(p, "ids").is_some() || get(p, "id").is_some() {
        return Err(bad(CMD, "takes `code` and `maxOperations` only; look layers up with find(...) in the script"));
    }
    let code =
        get(p, "code").and_then(Value::as_str).filter(|c| !c.trim().is_empty()).ok_or_else(|| bad(CMD, "`code` required (the script as text)"))?;
    if code.len() > MAX_CODE_BYTES {
        return Err(bad(CMD, format!("`code` is limited to {MAX_CODE_BYTES} bytes")));
    }
    let max_ops = match get(p, "maxOperations") {
        None | Some(Value::Null) => DEFAULT_OPERATIONS,
        Some(v) => v
            .as_u64()
            .filter(|n| (1..=MAX_OPERATIONS).contains(n))
            .ok_or_else(|| bad(CMD, format!("`maxOperations` must be an integer from 1 to {MAX_OPERATIONS}")))?,
    };
    let ui_before = s.ui_requests.len();
    let r = s.grouped("Script", |s| execute(s, code, max_ops));
    // Commands of a script open no dialogs.
    s.ui_requests.truncate(ui_before);
    r
}

/// The session while the script runs, shared by the host functions. The session itself moves in
/// here for the run (the functions must be `'static`) and moves back at the end.
struct Host {
    s: Session,
    logs: Vec<String>,
    logs_dropped: usize,
    commands: u32,
    started: web_time::Instant,
}

type Shared = Rc<RefCell<Host>>;

impl Host {
    fn log(&mut self, text: &str) {
        if self.logs.len() >= MAX_LOG_LINES {
            self.logs_dropped += 1;
        } else {
            self.logs.push(text.chars().take(MAX_LOG_CHARS).collect());
        }
    }

    /// One engine command; a failing one leaves nothing behind, so a script may catch its error.
    fn command(&mut self, id: &str, params: &Value) -> std::result::Result<Value, String> {
        if let Some(why) = sandbox_refusal(id, params) {
            return Err(why);
        }
        if self.commands >= MAX_COMMANDS {
            return Err(format!("a script may run at most {MAX_COMMANDS} commands"));
        }
        if self.started.elapsed().as_secs() >= MAX_SECONDS {
            return Err(format!(
                "the script ran for {MAX_SECONDS} seconds ({} commands): its commands are slow on this file; run fewer commands per call: one command.each call, or a narrower find()",
                self.commands
            ));
        }
        self.commands += 1;
        let ui_before = self.s.ui_requests.len();
        self.s
            .grouped(id, |s| {
                let v = s.execute(id, params)?;
                if s.ui_requests.len() > ui_before {
                    s.ui_requests.truncate(ui_before);
                    return Err(EngineError::Disabled(id.into(), "it opens a dialog or panel, which scripts can't".into()));
                }
                Ok(v)
            })
            .map_err(|e| e.to_string())
    }
}

/// Run `code` on the document as one group (the caller's), and report what it did.
fn execute(s: &mut Session, code: &str, max_ops: u64) -> Result<Value> {
    let before = s.ds()?.doc.clone();
    let host = Rc::new(RefCell::new(Host {
        s: std::mem::replace(s, Session::empty()),
        logs: Vec::new(),
        logs_dropped: 0,
        commands: 0,
        started: web_time::Instant::now(),
    }));
    let outcome = guard::run(CMD, || Ok(eval(&host, code, max_ops)));
    // Back to its owner, also after a panic.
    if let Ok(mut h) = host.try_borrow_mut() {
        std::mem::swap(s, &mut h.s);
    }
    let (logs, dropped, commands) = match host.try_borrow() {
        Ok(h) => (h.logs.clone(), h.logs_dropped, h.commands),
        Err(_) => (Vec::new(), 0, 0),
    };
    let (value, operations) = match outcome? {
        Ok(done) => done,
        Err(message) => {
            let log_tail: Vec<&str> = logs.iter().take(ERROR_LOG_LINES).map(String::as_str).collect();
            let tail = if log_tail.is_empty() { String::new() } else { format!("\nlogs before the error: {}", log_tail.join(" | ")) };
            return Err(bad(CMD, format!("{message}{tail}\nnothing was changed")));
        }
    };

    let after = s.ds()?.doc.clone();
    let (mut created, mut changed, mut deleted) = (Vec::<NodeId>::new(), 0usize, 0usize);
    for id in after.changed_ids(&before, usize::MAX).unwrap_or_default() {
        match (before.contains(id), after.contains(id)) {
            (false, true) => created.push(id),
            (true, false) => deleted += 1,
            (true, true) => changed += 1,
            (false, false) => {}
        }
    }
    let created_count = created.len();
    created.truncate(MAX_LISTED);
    let mut out = json!({
        "value": value,
        "logs": logs,
        "created": created,
        "changed": changed,
        "deleted": deleted,
        "commands": commands,
        "operations": operations,
    });
    if let Some(o) = out.as_object_mut() {
        if dropped > 0 {
            o.insert("logsTruncated".into(), json!(dropped));
        }
        if created_count > MAX_LISTED {
            o.insert("createdTotal".into(), json!(created_count));
            o.insert("truncated".into(), json!(true));
        }
    }
    Ok(out)
}

/// Compile and run the script: its value and the operations it used, or a readable error.
fn eval(host: &Shared, code: &str, max_ops: u64) -> std::result::Result<(Value, u64), String> {
    let used = Rc::new(Cell::new(0u64));
    let mut engine = Engine::new();
    engine.set_max_call_levels(32);
    engine.set_max_expr_depths(64, 32);
    engine.set_max_string_size(1_000_000);
    // These also bound how deep data nests (every level adds one element), and with it the stack
    // the size check and the drop of a nested value use.
    engine.set_max_array_size(MAX_ARRAY);
    engine.set_max_map_size(MAX_MAP);
    engine.disable_symbol("eval");
    let counter = used.clone();
    engine.on_progress(move |n| {
        counter.set(n);
        if n > max_ops { Some(Dynamic::from("operations")) } else { None }
    });
    let sink = host.clone();
    engine.on_print(move |text| {
        if let Ok(mut h) = sink.try_borrow_mut() {
            h.log(text);
        }
    });
    let sink = host.clone();
    engine.on_debug(move |text, _, _| {
        if let Ok(mut h) = sink.try_borrow_mut() {
            h.log(text);
        }
    });
    register(&mut engine, host);

    let ast = engine.compile(code).map_err(|e| describe(code, max_ops, &EvalAltResult::ErrorParsing(*e.0, e.1)))?;
    let result = engine.eval_ast::<Dynamic>(&ast).map_err(|e| describe(code, max_ops, &e))?;
    // A value that does not convert (a function pointer) shows as its text.
    let value = to_json(&result, 0).unwrap_or_else(|_| Value::String(result.to_string()));
    Ok((value, used.get()))
}

fn runtime(message: impl Into<String>) -> Box<EvalAltResult> {
    EvalAltResult::ErrorRuntime(Dynamic::from(message.into()), Position::NONE).into()
}

/// A host function's body, with the session borrowed for its duration.
fn with_host<T>(host: &Shared, f: impl FnOnce(&mut Host) -> std::result::Result<T, String>) -> std::result::Result<T, Box<EvalAltResult>> {
    let mut h = host.try_borrow_mut().map_err(|_| runtime("the document is busy"))?;
    f(&mut h).map_err(runtime)
}

fn register(engine: &mut Engine, host: &Shared) {
    let h = host.clone();
    engine.register_fn("cmd", move |id: ImmutableString, params: Map| -> std::result::Result<Dynamic, Box<EvalAltResult>> {
        let params = to_json(&Dynamic::from_map(params), 0).map_err(|e| runtime(format!("cmd({id}): {e}")))?;
        with_host(&h, |h| h.command(&id, &params).and_then(|v| from_json(&v, 0)))
    });
    let h = host.clone();
    engine.register_fn("cmd", move |id: ImmutableString| -> std::result::Result<Dynamic, Box<EvalAltResult>> {
        with_host(&h, |h| h.command(&id, &json!({})).and_then(|v| from_json(&v, 0)))
    });
    let h = host.clone();
    engine.register_fn("find", move |filters: Map| -> std::result::Result<Array, Box<EvalAltResult>> {
        let mut filters = to_json(&Dynamic::from_map(filters), 0).map_err(|e| runtime(format!("find: {e}")))?;
        if let Some(o) = filters.as_object_mut() {
            o.entry("limit").or_insert(json!(MAX_ARRAY));
        }
        with_host(&h, |h| {
            let found = h.command("node.find", &filters)?;
            let ids: Array = found.get("ids").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_i64).map(Dynamic::from).collect();
            let total = found.get("total").and_then(Value::as_u64).unwrap_or(ids.len() as u64);
            if total > ids.len() as u64 {
                return Err(format!("find: {total} layers match but {} came back; narrow the filters or set `limit` (up to {MAX_ARRAY})", ids.len()));
            }
            Ok(ids)
        })
    });
    let h = host.clone();
    engine.register_fn("get", move |id: i64| -> std::result::Result<Dynamic, Box<EvalAltResult>> {
        with_host(&h, |h| h.command("node.get", &json!({"id": id})).and_then(|v| from_json(&v, 0)))
    });
    // `log` of a number is Rhai's own logarithm, so the number types are overridden here.
    let h = host.clone();
    let log = move |value: Dynamic| -> std::result::Result<(), Box<EvalAltResult>> {
        with_host(&h, |h| {
            h.log(&value.to_string());
            Ok(())
        })
    };
    engine.register_fn("log", log.clone());
    engine.register_fn("log", {
        let log = log.clone();
        move |n: f64| log(Dynamic::from(n))
    });
    engine.register_fn("log", {
        let log = log.clone();
        move |n: i64| log(Dynamic::from(n))
    });
}

/// The script's error as one readable message with where it happened.
fn describe(code: &str, max_ops: u64, err: &EvalAltResult) -> String {
    let mut err = err;
    while let EvalAltResult::ErrorInFunctionCall(_, _, inner, _) = err {
        err = inner;
    }
    let text = match err {
        EvalAltResult::ErrorTerminated(..) | EvalAltResult::ErrorTooManyOperations(_) => {
            format!(
                "the script used more than {max_ops} operations (an endless loop?); lower the work or raise `maxOperations` (up to {MAX_OPERATIONS})"
            )
        }
        other => {
            let full = other.to_string();
            full.rfind(" (line ").and_then(|i| full.get(..i)).unwrap_or(&full).to_string()
        }
    };
    let pos = err.position();
    match (pos.line(), pos.position()) {
        (Some(line), column) => {
            let source = code.lines().nth(line - 1).map(|l| l.trim().chars().take(120).collect::<String>()).unwrap_or_default();
            let at = column.map_or(format!("line {line}"), |c| format!("line {line}, column {c}"));
            format!("script error at {at}: {text}\n  {line} | {source}")
        }
        _ => format!("script error: {text}"),
    }
}

fn from_json(v: &Value, depth: usize) -> std::result::Result<Dynamic, String> {
    if depth > MAX_DEPTH {
        return Err(format!("a result nested more than {MAX_DEPTH} levels deep"));
    }
    Ok(match v {
        Value::Null => Dynamic::UNIT,
        Value::Bool(b) => Dynamic::from(*b),
        Value::Number(n) => match n.as_i64() {
            Some(i) => Dynamic::from(i),
            None => Dynamic::from(n.as_f64().unwrap_or(f64::NAN)),
        },
        Value::String(s) => Dynamic::from(s.clone()),
        Value::Array(a) => Dynamic::from_array(a.iter().map(|x| from_json(x, depth + 1)).collect::<std::result::Result<_, _>>()?),
        Value::Object(o) => {
            let mut map = Map::new();
            for (k, x) in o {
                map.insert(k.as_str().into(), from_json(x, depth + 1)?);
            }
            Dynamic::from_map(map)
        }
    })
}

fn to_json(d: &Dynamic, depth: usize) -> std::result::Result<Value, String> {
    if depth > MAX_DEPTH {
        return Err(format!("values nest at most {MAX_DEPTH} levels deep"));
    }
    if d.is_unit() {
        return Ok(Value::Null);
    }
    if let Ok(b) = d.as_bool() {
        return Ok(Value::Bool(b));
    }
    if let Ok(i) = d.as_int() {
        return Ok(json!(i));
    }
    if let Ok(f) = d.as_float() {
        return serde_json::Number::from_f64(f).map(Value::Number).ok_or_else(|| "a number that is not finite (NaN or infinity)".to_string());
    }
    if let Ok(c) = d.as_char() {
        return Ok(Value::String(c.to_string()));
    }
    if d.is_string() {
        return Ok(Value::String(d.to_string()));
    }
    if let Some(a) = d.read_lock::<Array>() {
        return a.iter().map(|x| to_json(x, depth + 1)).collect::<std::result::Result<Vec<_>, _>>().map(Value::Array);
    }
    if let Some(m) = d.read_lock::<Map>() {
        let mut out = serde_json::Map::new();
        for (k, x) in m.iter() {
            out.insert(k.to_string(), to_json(x, depth + 1)?);
        }
        return Ok(Value::Object(out));
    }
    Err(format!("a `{}` value can't be passed on", d.type_name()))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::Session;

    fn run(s: &mut Session, code: &str) -> crate::Result<serde_json::Value> {
        s.execute("script.run", &json!({"code": code}))
    }

    fn layer_count(s: &mut Session) -> usize {
        s.execute("node.find", &json!({"limit": 10000})).unwrap()["total"].as_u64().unwrap() as usize
    }

    #[test]
    fn a_loop_creates_frames_in_one_undo_step() {
        let mut s = Session::new();
        let r = run(
            &mut s,
            r#"
            let ids = [];
            for i in 0..5 {
                let r = cmd("shape.rectangle", #{x: i * 120, y: 0, width: 100, height: 60});
                ids.push(r.id);
            }
            log("made " + ids.len());
            ids.len()
            "#,
        )
        .unwrap();
        assert_eq!(r["value"], json!(5), "{r}");
        assert_eq!(r["created"].as_array().unwrap().len(), 5, "{r}");
        assert_eq!(r["logs"], json!(["made 5"]), "{r}");
        assert_eq!(r["commands"], json!(5), "{r}");
        assert_eq!(layer_count(&mut s), 5);
        s.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(layer_count(&mut s), 0, "one undo step takes all five back");
    }

    #[test]
    fn find_and_get_drive_conditions() {
        let mut s = Session::new();
        for (i, name) in ["Card", "Card", "Other"].iter().enumerate() {
            let r = s.execute("shape.rectangle", &json!({"x": i * 50, "y": 0, "width": 40, "height": 40})).unwrap();
            s.execute("node.set", &json!({"ids": [r["id"]], "props": {"name": name}})).unwrap();
        }
        let r = run(
            &mut s,
            r##"
            let n = 0;
            for id in find(#{name: "Card"}) {
                if get(id).name == "Card" {
                    cmd("node.set", #{ids: [id], props: #{fills: [#{type: "solid", color: "#FF0000"}]}});
                    n += 1;
                }
            }
            n
            "##,
        )
        .unwrap();
        assert_eq!(r["value"], json!(2), "{r}");
        assert_eq!(r["changed"], json!(2), "{r}");
    }

    #[test]
    fn a_throw_midway_leaves_the_document_unchanged() {
        let mut s = Session::new();
        let err = run(&mut s, "cmd(\"shape.rectangle\", #{x: 0, y: 0, width: 10, height: 10});\nlog(\"before\");\nthrow \"boom\";")
            .unwrap_err()
            .to_string();
        assert!(err.contains("boom") && err.contains("line 3"), "{err}");
        assert!(err.contains("before") && err.contains("nothing was changed"), "{err}");
        assert_eq!(layer_count(&mut s), 0);
        assert!(s.execute("edit.undo", &json!({})).is_err() || layer_count(&mut s) == 0);
    }

    #[test]
    fn a_failing_command_is_a_script_error_a_script_can_catch() {
        let mut s = Session::new();
        let err = run(
            &mut s,
            "cmd(\"shape.rectangle\", #{x: 0, y: 0, width: 10, height: 10});\ncmd(\"node.set\", #{ids: [999999], props: #{name: \"x\"}});",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("line 2"), "{err}");
        assert_eq!(layer_count(&mut s), 0);
        let r = run(&mut s, "let ok = true;\ntry { cmd(\"node.set\", #{ids: [999999], props: #{name: \"x\"}}); } catch (e) { ok = false; }\ncmd(\"shape.rectangle\", #{x: 0, y: 0, width: 10, height: 10});\nok").unwrap();
        assert_eq!(r["value"], json!(false), "{r}");
        assert_eq!(layer_count(&mut s), 1);
    }

    #[test]
    fn the_operation_budget_stops_an_endless_loop() {
        let mut s = Session::new();
        let err = s.execute("script.run", &json!({"code": "let i = 0; loop { i += 1; }", "maxOperations": 1000})).unwrap_err().to_string();
        assert!(err.contains("more than 1000 operations") && err.contains("maxOperations"), "{err}");
        let err = run(&mut s, "let i = 0; while true { i += 1; }").unwrap_err().to_string();
        assert!(err.contains("200000 operations"), "{err}");
    }

    #[test]
    fn slow_commands_stop_the_script_after_the_time_budget() {
        let mut s = Session::new();
        let made = s.execute("shape.rectangle", &json!({"x": 0, "y": 0, "width": 5, "height": 5})).unwrap();
        for _ in 0..600 {
            s.execute("shape.rectangle", &json!({"x": 0, "y": 0, "width": 5, "height": 5})).unwrap();
        }
        let err = run(&mut s, "for i in 0..9000 { cmd(\"a11y.audit\"); } 1").unwrap_err().to_string();
        assert!(err.contains("seconds") && err.contains("slow"), "{err}");
        assert!(err.contains("nothing was changed"), "{err}");
        assert!(s.doc().unwrap().contains(figcraft_doc::NodeId(made["id"].as_u64().unwrap())));
    }

    #[test]
    fn the_command_budget_and_data_limits_hold() {
        let mut s = Session::new();
        let err = s
            .execute("script.run", &json!({"code": "for i in 0..20000 { cmd(\"select.none\"); }", "maxOperations": 5_000_000}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("at most 10000 commands"), "{err}");
        let err = run(&mut s, "let s = \"x\"; for i in 0..40 { s += s; } s.len()").unwrap_err().to_string();
        assert!(err.to_lowercase().contains("size") || err.contains("too large"), "{err}");
        // Deeply nested data neither crashes nor reaches a command.
        // Nested data stops at the size limits, on a small stack.
        std::thread::Builder::new()
            .stack_size(768 * 1024)
            .spawn(|| {
                let mut s = Session::new();
                for code in [
                    "let x = []; for i in 0..1000000 { x = [x]; } 1",
                    "let x = #{}; for i in 0..1000000 { x = #{a: x}; } 1",
                    "let x = []; for i in 0..1000000 { x = [#{a: x}]; } 1",
                ] {
                    let err = s.execute("script.run", &json!({"code": code, "maxOperations": 5_000_000})).unwrap_err().to_string();
                    assert!(err.contains("Size of"), "{err}");
                }
            })
            .unwrap()
            .join()
            .unwrap();
        let err = run(&mut s, "let x = #{}; for i in 0..100 { x = #{a: x}; } cmd(\"select.none\", x);").unwrap_err().to_string();
        assert!(err.contains("nest"), "{err}");
    }

    #[test]
    fn a_script_cannot_reach_files_or_other_scripts() {
        let mut s = Session::new();
        let dir = std::env::temp_dir().join("figcraft-script-test-should-not-exist.fig");
        let path = dir.to_string_lossy().replace('\\', "/");
        for code in [
            format!("cmd(\"file.saveAs\", #{{path: \"{path}\"}})"),
            format!("cmd(\"file.open\", #{{path: \"{path}\"}})"),
            format!("cmd(\"image.place\", #{{path: \"{path}\"}})"),
            "cmd(\"script.run\", #{code: \"1\"})".to_string(),
            "cmd(\"command.batch\", #{commands: []})".to_string(),
            "import \"x\" as y; 1".to_string(),
            "eval(\"1\")".to_string(),
            "let t = timestamp(); 1".to_string(),
            "open(\"a\")".to_string(),
        ] {
            assert!(run(&mut s, &code).is_err(), "{code}");
        }
        assert!(!dir.exists());
    }

    #[test]
    fn junk_parameters_are_errors_not_panics() {
        let mut s = Session::new();
        for junk in [
            json!({}),
            json!({"code": ""}),
            json!({"code": 5}),
            json!({"code": null}),
            json!({"code": "1", "maxOperations": 0}),
            json!({"code": "1", "maxOperations": -3}),
            json!({"code": "1", "maxOperations": "many"}),
            json!({"code": "1", "maxOperations": 999999999999u64}),
            json!({"code": "1", "ids": [1]}),
            json!({"code": "let"}),
            json!({"code": "cmd(5)"}),
            json!({"code": "cmd(\"node.set\", 5)"}),
            json!({"code": "cmd(\"nope\")"}),
            json!({"code": "get(-1)"}),
            json!({"code": "find(#{type: 7})"}),
            json!({"code": "find(#{limit: -1})"}),
            json!({"code": "cmd(\"shape.rectangle\", #{x: 1.0 / 0.0, y: 0, width: 1, height: 1})"}),
        ] {
            let _ = s.execute("script.run", &junk);
        }
        assert_eq!(layer_count(&mut s), 0);
        assert!(Session::empty().execute("script.run", &json!({"code": "1"})).is_err());
    }

    #[test]
    fn dry_run_reports_and_undoes() {
        let mut s = Session::new();
        let r =
            s.execute("script.run", &json!({"code": "cmd(\"shape.rectangle\", #{x: 0, y: 0, width: 10, height: 10}); 7", "dryRun": true})).unwrap();
        assert_eq!(r["dryRun"], json!(true), "{r}");
        assert_eq!(r["created"].as_array().unwrap().len(), 1, "{r}");
        assert_eq!(r["result"]["value"], json!(7), "{r}");
        assert_eq!(layer_count(&mut s), 0);
    }

    #[test]
    fn log_of_a_number_logs_it() {
        let mut s = Session::new();
        let r = run(&mut s, "log(2); log(2.5); log(\"a\"); print(\"b\"); log(#{k: 1}); 1").unwrap();
        assert_eq!(r["logs"], json!(["2", "2.5", "a", "b", "#{\"k\": 1}"]), "{r}");
    }
}
