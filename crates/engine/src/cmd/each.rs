//! `command.each`: one command run on each of many layers, with a result per layer, partial
//! failure (`continueOnError`) and a resume cursor for long runs. Also the shared reading of the
//! error mode and of the `from`/`limit` window that `command.batch` takes.
//!
//! The cursor is a saved layer list ([`crate::cmd::idlists`]) named `each-<n>` that holds the layers
//! still to do, so a run needs no state of its own: the next call names it in `cursor`.

use figcraft_doc::NodeId;
use serde_json::{Value, json};

use super::CommandSpec;
use crate::params::{bool_of, get, req_string, string};
use crate::{EngineError, Result, Session, bad, find_command, spec};

const CMD: &str = "command.each";
/// Layers one call runs when `limit` is not given, and the most it may be.
const DEFAULT_LIMIT: usize = 200;
const MAX_LIMIT: usize = 5000;
/// Unfinished runs one file keeps.
const MAX_CURSORS: usize = 20;
/// Longest result (in characters, as JSON) a layer's entry carries; a longer one is replaced by a note.
const MAX_RESULT_CHARS: usize = 500;
const CURSOR_PREFIX: &str = "each-";

pub fn specs() -> Vec<CommandSpec> {
    vec![spec!(
        "command.each",
        "Run a command on each layer",
        "{command, params? (the command's own params, without ids: it runs once per layer with `ids` set to that layer), ids? (the layers; default the selection; `where` and `list` name them too), continueOnError? (default false: the first failure rolls the call back and is the error; true: a failed layer is rolled back alone and reported), limit? (layers per call, default 200, up to 5000), cursor? (the `nextCursor` of the previous call: continue with the layers it left), label? (undo label)}  one undo step per call → {command, processed, ok, failed, remaining, done, items: [{id, ok, result? | error?}], nextCursor?}",
        false,
        each
    )]
}

/// `onError: abort|continue` and its boolean form `continueOnError`: whether failed steps are
/// rolled back alone and the rest still run. Giving both and disagreeing is an error.
pub(crate) fn continue_on_error(cmd: &str, p: &Value) -> Result<bool> {
    let mode = match get(p, "onError") {
        None => None,
        Some(Value::String(m)) if m == "abort" => Some(false),
        Some(Value::String(m)) if m == "continue" => Some(true),
        Some(_) => return Err(bad(cmd, "`onError` must be \"abort\" or \"continue\"")),
    };
    let flag = match get(p, "continueOnError") {
        None => None,
        Some(v) => Some(bool_of(v).ok_or_else(|| bad(cmd, "`continueOnError` must be a boolean (true or false)"))?),
    };
    match (mode, flag) {
        (Some(a), Some(b)) if a != b => Err(bad(cmd, "`onError` and `continueOnError` disagree: give one of them")),
        (a, b) => Ok(a.or(b).unwrap_or(false)),
    }
}

/// A whole number of `min` or more from `key` (`None` when absent), or an error.
pub(crate) fn count_param(cmd: &str, p: &Value, key: &str, min: usize, max: usize) -> Result<Option<usize>> {
    let Some(v) = get(p, key) else { return Ok(None) };
    let n = crate::params::f64_of(v).filter(|n| n.fract() == 0.0 && (min as f64..=max as f64).contains(n));
    n.map(|n| Some(n as usize)).ok_or_else(|| bad(cmd, format!("`{key}` must be a whole number from {min} to {max}")))
}

/// A result short enough to list per layer: nothing for an empty one, a note for a long one.
fn short(v: Value) -> Option<Value> {
    if v.is_null() || v == json!({"ok": true}) {
        return None;
    }
    let chars = v.to_string().chars().count();
    Some(if chars > MAX_RESULT_CHARS { json!({"truncated": true, "chars": chars}) } else { v })
}

/// The layers to run on: the saved cursor list, or the call's own targets.
fn layers(s: &Session, p: &Value, cursor: Option<&str>) -> Result<Vec<NodeId>> {
    let Some(name) = cursor else { return crate::params::req_targets(CMD, s, p) };
    if get(p, "ids").is_some() || get(p, "id").is_some() {
        return Err(bad(CMD, "give `cursor` or `ids`/`where`/`list`, not both"));
    }
    let ds = s.ds()?;
    let list = name.starts_with(CURSOR_PREFIX).then(|| ds.id_lists.get(name)).flatten();
    let list = list.ok_or_else(|| bad(CMD, format!("no run to continue under cursor \"{name}\" (it was finished, or the tab was closed)")))?;
    Ok(list.iter().copied().filter(|i| ds.doc.contains(*i)).collect())
}

fn each(s: &mut Session, p: &Value) -> Result<Value> {
    let command = req_string(CMD, p, "command")?;
    if command == CMD {
        return Err(bad(CMD, "`command` can't be command.each itself"));
    }
    if find_command(&command).is_none() {
        return Err(EngineError::UnknownCommand(command));
    }
    let inner = match get(p, "params") {
        None => json!({}),
        Some(v @ Value::Object(_)) => v.clone(),
        Some(_) => return Err(bad(CMD, "`params` must be an object")),
    };
    if let Some(k) = ["ids", "id", "where", "list"].into_iter().find(|k| get(&inner, k).is_some()) {
        return Err(bad(CMD, format!("`params.{k}` can't be given: name the layers with `ids`, `where` or `list` beside `command`")));
    }
    let keep_going = continue_on_error(CMD, p)?;
    let limit = count_param(CMD, p, "limit", 1, MAX_LIMIT)?.unwrap_or(DEFAULT_LIMIT);
    let cursor = match get(p, "cursor") {
        None => None,
        Some(Value::String(c)) => Some(c.clone()),
        Some(_) => return Err(bad(CMD, "`cursor` must be the text a previous call returned")),
    };
    let label = string(p, "label").unwrap_or_else(|| "Run command on each layer".into());

    let all = layers(s, p, cursor.as_deref())?;
    if all.is_empty() {
        return Err(bad(CMD, "no layers left to run on"));
    }
    let take = limit.min(all.len());
    let chunk = all.get(..take).unwrap_or_default().to_vec();
    let rest = all.get(take..).unwrap_or_default().to_vec();

    // The cursor list holds what is left; the same name goes on until the run is done.
    let name = match (&cursor, rest.is_empty()) {
        // A dry run leaves the cursors as they are: its run is rolled back.
        _ if s.rolling_back => None,
        (Some(c), _) => Some(c.clone()),
        (None, true) => None,
        (None, false) => {
            let ds = s.ds()?;
            if ds.id_lists.keys().filter(|k| k.starts_with(CURSOR_PREFIX)).count() >= MAX_CURSORS {
                return Err(bad(CMD, format!("{MAX_CURSORS} runs are unfinished in this file: finish one or remove it with list.delete")));
            }
            (1..).map(|n| format!("{CURSOR_PREFIX}{n}")).find(|k| !ds.id_lists.contains_key(k))
        }
    };

    let items = s.grouped(&label, |s| {
        let mut items = Vec::with_capacity(chunk.len());
        for id in &chunk {
            let mut params = inner.clone();
            params["ids"] = json!([id]);
            match s.checkpointed(None, |s| s.execute(&command, &params)) {
                Ok(v) => {
                    let mut item = json!({"id": id, "ok": true});
                    if let Some(r) = short(v) {
                        item["result"] = r;
                    }
                    items.push(item);
                }
                Err(EngineError::Waiting) => return Err(EngineError::Waiting),
                Err(e) if keep_going => items.push(json!({"id": id, "ok": false, "error": e.to_string()})),
                Err(e) => {
                    let done = items.len();
                    return Err(bad(CMD, format!("{command} failed on layer {id} (after {done} done; nothing was changed): {e}")));
                }
            }
        }
        Ok(items)
    })?;

    let failed = items.iter().filter(|i| i["ok"] == json!(false)).count();
    let mut out = json!({"command": command, "processed": items.len(), "ok": items.len() - failed, "failed": failed, "remaining": rest.len(), "done": rest.is_empty(), "items": items});
    if let Some(name) = name {
        let ds = s.ds_mut()?;
        if rest.is_empty() {
            ds.id_lists.remove(&name);
        } else {
            out["nextCursor"] = json!(name);
            ds.id_lists.insert(name, rest);
        }
    }
    Ok(out)
}
