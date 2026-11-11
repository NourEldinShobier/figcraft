//! Parameters every command takes, handled before the command sees its params:
//! - `where`: a `node.find` filter; the layers it finds are the command's `ids`.
//! - `list`: the name of a saved list ([`crate::cmd::idlists`]); its layers are the command's `ids`.
//! - `dryRun`: run the command, report which layers it changed, created and deleted, and roll back.
//! - `keepSelection`: leave the selection and the current page as they were.
//! - `maxAffected`, `confirm`: refuse a mutating command that would change more layers than that
//!   (counted by a dry run first) unless `confirm` is true ([`crate::cmd::safety`]).
//! - `as`: the owner label the call edits locked layers as ([`crate::cmd::safety`]).
//! - `withChanged`: wrap the result as `{result, "$changed": {count, ids, props, noop?}}`.

use super::{Arc, Document, EngineError, NodeId, Result, Session, Value, bad, cmd, find_command, guard, params};
use crate::cmd::safety;

/// The generic parameter names. A command whose own doc lists one keeps it (`layer.autoName` has
/// a `dryRun` of its own).
const GENERIC: [&str; 8] = ["where", "list", "dryRun", "keepSelection", "maxAffected", "confirm", "as", "withChanged"];

/// Most layers `dryRun` names in each of its lists.
const MAX_LISTED: usize = 10_000;

/// Most layers one `where` can act on (`node.find`'s limit).
const MAX_WHERE: usize = 10_000;

fn takes_own(id: &str, key: &str) -> bool {
    cmd::param_names(id).is_some_and(|names| names.contains(&key))
}

/// Whether `params` hold a generic parameter that command `id` doesn't take itself.
pub(super) fn has_generic(id: &str, params: &Value) -> bool {
    params.as_object().is_some_and(|o| GENERIC.iter().any(|k| o.contains_key(*k) && !takes_own(id, k)))
}

/// The layers a dry run would touch.
struct Changes {
    changed: Vec<NodeId>,
    created: Vec<NodeId>,
    deleted: Vec<NodeId>,
}

fn changes(before: &Document, after: &Document) -> Changes {
    let mut c = Changes { changed: Vec::new(), created: Vec::new(), deleted: Vec::new() };
    for id in after.changed_ids(before, usize::MAX).unwrap_or_default() {
        match (before.contains(id), after.contains(id)) {
            (false, _) => c.created.push(id),
            (true, false) => c.deleted.push(id),
            (true, true) => c.changed.push(id),
        }
    }
    c
}

/// Why `id` can't be dry-run: only commands that merely edit the document (the undoable ones) can
/// be rolled back. A batch can when every step can, and a script (a group of its own) always can.
fn dry_run_refusal(id: &str, params: &Value) -> Option<String> {
    if id == "script.run" {
        return None;
    }
    if id == "command.batch" {
        for step in params.get("commands").and_then(Value::as_array).into_iter().flatten() {
            let Some(step_id) = step.get("command").and_then(Value::as_str) else { continue };
            if let Some(why) = dry_run_refusal(step_id, step.get("params").unwrap_or(&Value::Null)) {
                return Some(why);
            }
        }
        return None;
    }
    find_command(id).filter(|spec| !spec.history).map(|_| format!("dryRun is not available for {id}"))
}

impl Session {
    /// [`Session::execute`] for params that hold a generic parameter (see [`has_generic`]): the
    /// command runs with those removed, and `ids` set when `where` or `list` named its layers.
    pub(super) fn execute_generic(&mut self, id: &str, params: &Value) -> Result<Value> {
        let mut inner = params.as_object().cloned().unwrap_or_default();
        // A command that documents its own `confirm` keeps it; a `true` there also lifts the guard.
        let own_confirm = takes_own(id, "confirm") && params::get(params, "confirm").and_then(Value::as_bool) == Some(true);
        let [filter, list, dry, keep, max, confirm, actor, with] =
            GENERIC.map(|k| if takes_own(id, k) { None } else { inner.remove(k).filter(|v| !v.is_null()) });
        let flag = |name: &str, v: Option<Value>| match v {
            None => Ok(false),
            Some(v) => v.as_bool().ok_or_else(|| bad(id, format!("`{name}` must be a boolean (true or false)"))),
        };
        let (dry, keep, with) = (flag("dryRun", dry)?, flag("keepSelection", keep)?, flag("withChanged", with)?);
        let confirm = flag("confirm", confirm)? || own_confirm;
        let max = match max {
            None => None,
            Some(v) => Some(
                v.as_u64()
                    .filter(|n| *n >= 1)
                    .and_then(|n| usize::try_from(n).ok())
                    .ok_or_else(|| bad(id, "`maxAffected` must be a whole number of 1 or more"))?,
            ),
        };
        let actor = match actor {
            None => None,
            Some(v) => match v.as_str().map(str::trim) {
                Some(a) if !a.is_empty() && a.chars().count() <= 100 => Some(a.to_string()),
                _ => return Err(bad(id, "`as` must be an owner label (1 to 100 characters)")),
            },
        };

        let mut given = Vec::new();
        if params::get(params, "ids").is_some() || params::get(params, "id").is_some() {
            given.push("ids");
        }
        if filter.is_some() {
            given.push("where");
        }
        if list.is_some() {
            given.push("list");
        }
        if given.len() > 1 {
            let msg = match given.as_slice() {
                [one, other] => format!("give {one} or {other}, not both"),
                _ => "give only one of ids, where and list".to_string(),
            };
            return Err(bad(id, msg));
        }
        if id == "command.batch"
            && let Some(name) = given.first().filter(|g| **g != "ids")
        {
            return Err(bad(id, format!("`{name}` can't be used on a batch: put it on the steps that act on layers")));
        }
        if dry && let Some(why) = dry_run_refusal(id, params) {
            return Err(bad(id, why));
        }

        let matched = match (filter, list) {
            (Some(f), _) => Some(self.layers_where(id, &f)?),
            (None, Some(l)) => Some(self.layers_in_list(id, &l)?),
            (None, None) => None,
        };
        if let Some(ids) = &matched {
            if ids.is_empty() {
                return Ok(serde_json::json!({"matched": 0, "ids": [], "skipped": true}));
            }
            inner.remove("id");
            inner.insert("ids".into(), serde_json::json!(ids));
        }

        let inner = Value::Object(inner);
        let kept = if keep { self.ds().ok().map(|d| (d.uid, d.selection.clone(), d.page)) } else { None };
        // The guard and the summary are for edits; a read command just drops them.
        let guarded = !dry && safety::mutates(id, &inner) && (max.is_some() || with);
        let was_actor = actor.map(|a| std::mem::replace(&mut self.actor, a));
        let mut changed = None;
        let result = if dry {
            self.dry_run(id, &inner)
        } else if guarded {
            self.run_guarded(id, &inner, max, confirm, with).map(|(v, c)| {
                changed = c;
                v
            })
        } else {
            self.execute(id, &inner)
        };
        if let Some(was) = was_actor {
            self.actor = was;
        }
        // Also after an error: the command may have moved the selection before it failed.
        if let Some((uid, selection, page)) = kept
            && let Some(ds) = self.docs.iter_mut().find(|d| d.uid == uid)
        {
            if ds.doc.pages.contains(&page) {
                ds.page = page;
            }
            ds.selection = selection;
            ds.clean_selection();
            ds.revision += 1;
        }

        let mut value = result?;
        if let (Some(ids), Some(o)) = (matched, value.as_object_mut()) {
            o.insert("matched".into(), serde_json::json!(ids.len()));
            // A command's own `ids` (the copies a duplicate made) stay.
            let key = if o.contains_key("ids") { "matchedIds" } else { "ids" };
            o.insert(key.into(), serde_json::json!(ids));
        }
        Ok(match changed {
            Some(c) => serde_json::json!({"result": value, "$changed": c}),
            None => value,
        })
    }

    /// Run a mutating command under `maxAffected` (a dry run counts the layers first; more than
    /// `max` is refused unless `confirm`) and, with `with`, summarise what it changed.
    fn run_guarded(&mut self, id: &str, params: &Value, max: Option<usize>, confirm: bool, with: bool) -> Result<(Value, Option<Value>)> {
        // A batch with a step that cannot be rolled back (a save, say) is not run twice.
        if let Some(max) = max.filter(|_| !confirm && dry_run_refusal(id, params).is_none()) {
            let (_, before, after) = self.dry_run_docs(id, params)?;
            let a = safety::affected(&before, &after);
            if a.count() > max {
                return Err(safety::over_limit(id, &a, max));
            }
        }
        let before = self.ds().ok().map(|d| (d.uid, d.doc.clone()));
        let targets = params::targets(self, params);
        let value = self.execute(id, params)?;
        // Remember what this edit touched, for `visual.diff`'s untargeted flag.
        if let Some((uid, b)) = &before
            && let Some(ds) = self.docs.iter_mut().find(|d| d.uid == *uid)
        {
            let mut touched = safety::affected(b, &ds.doc).all();
            touched.extend(targets.iter().copied());
            touched.sort_unstable();
            touched.dedup();
            touched.truncate(10_000);
            ds.last_touched = Some((std::sync::Arc::downgrade(&ds.doc), touched));
        }
        let changed = before.and_then(|(uid, b)| {
            let ds = self.docs.iter().find(|d| d.uid == uid)?;
            with.then(|| safety::summary(&b, &ds.doc, &targets, !matches!(id, "command.batch" | "script.run")))
        });
        Ok((value, changed))
    }

    /// The layers `node.find` finds for `filter` (the current page unless it names a `root`),
    /// refusing a set it could not return whole.
    fn layers_where(&mut self, id: &str, filter: &Value) -> Result<Vec<NodeId>> {
        if !filter.is_object() {
            return Err(bad(id, "`where` must be an object of node.find filters (name, type, text, …)"));
        }
        cmd::refuse_unknown_params("node.find", filter)?;
        let find = find_command("node.find").ok_or_else(|| EngineError::UnknownCommand("node.find".into()))?;
        let found = guard::run("node.find", || (find.run)(self, filter))?;
        let ids: Vec<NodeId> = found.get("ids").and_then(Value::as_array).into_iter().flatten().filter_map(params::id_of).collect();
        let total = found.get("total").and_then(Value::as_u64).and_then(|t| usize::try_from(t).ok()).unwrap_or(ids.len());
        if total > ids.len() {
            let hint = if ids.len() >= MAX_WHERE { "narrow the filter" } else { "raise `where.limit` (up to 10000) or narrow the filter" };
            return Err(bad(id, format!("`where` matches {total} layers but only {} came back: {hint} (the command was not run)", ids.len())));
        }
        Ok(ids)
    }

    /// The layers still in the saved list `name`.
    fn layers_in_list(&self, id: &str, name: &Value) -> Result<Vec<NodeId>> {
        let name = name.as_str().ok_or_else(|| bad(id, "`list` must be the name of a saved list (a string)"))?;
        let ds = self.ds()?;
        let Some(ids) = ds.id_lists.get(name) else { return Err(bad(id, cmd::idlists::unknown_list(ds, name))) };
        Ok(ids.iter().copied().filter(|i| ds.doc.contains(*i)).collect())
    }

    /// Run `id` with `params`, then put everything back like a failed batch does (the document,
    /// selection, page, undo and redo steps, journal, events) and report what it changed.
    fn dry_run_docs(&mut self, id: &str, params: &Value) -> Result<(Value, Arc<Document>, Arc<Document>)> {
        let last_command = self.last_command.clone();
        let clipboard = self.clipboard.clone();
        let ui_requests = self.ui_requests.len();
        let mut report = None;
        let outcome = self.grouped::<()>("Dry run", |s| {
            let before = s.ds()?.doc.clone();
            let result = s.execute(id, params)?;
            report = Some((result, before, s.ds()?.doc.clone()));
            // The group rolls back on an error: this one only stops it.
            Err(EngineError::Other("dry run".into()))
        });
        self.last_command = last_command;
        self.clipboard = clipboard;
        self.ui_requests.truncate(ui_requests);
        report.ok_or_else(|| outcome.err().unwrap_or_else(|| EngineError::Other("the dry run did not finish".into())))
    }

    fn dry_run(&mut self, id: &str, params: &Value) -> Result<Value> {
        let (result, before, after) = self.dry_run_docs(id, params)?;
        let c = changes(&before, &after);
        let count = c.changed.len() + c.created.len() + c.deleted.len();
        let truncated = [&c.changed, &c.created, &c.deleted].iter().any(|l| l.len() > MAX_LISTED);
        let cap = |mut v: Vec<NodeId>| {
            v.truncate(MAX_LISTED);
            v
        };
        Ok(serde_json::json!({
            "dryRun": true,
            "wouldChange": cap(c.changed),
            "created": cap(c.created),
            "deleted": cap(c.deleted),
            "count": count,
            "truncated": truncated,
            "result": result,
        }))
    }

    /// Run a batch, report what it did as a `doc.diff` of the document before and after, and put
    /// everything back (see [`Session::dry_run`]). A failed step or assertion is the batch's error.
    pub(crate) fn preview_batch(&mut self, label: &str, steps: &[crate::BatchStep], keep_going: bool) -> Result<Value> {
        let last_command = self.last_command.clone();
        let clipboard = self.clipboard.clone();
        let ui_requests = self.ui_requests.len();
        let mut report = None;
        let outcome = self.grouped::<()>("Preview", |s| {
            let before = s.ds()?.doc.clone();
            let results = if keep_going { s.batch_continue(label, steps)? } else { s.batch(label, steps)? };
            report = Some((results, before, s.ds()?.doc.clone()));
            Err(EngineError::Other("preview".into()))
        });
        self.last_command = last_command;
        self.clipboard = clipboard;
        self.ui_requests.truncate(ui_requests);
        let Some((results, before, after)) = report else {
            return Err(outcome.err().unwrap_or_else(|| EngineError::Other("the preview did not finish".into())));
        };
        let diff = cmd::docdiff::diff_documents("command.batch", &before, &after, true, 50, "preview", Value::Null, after.name.clone())?;
        Ok(serde_json::json!({"preview": true, "rolledBack": true, "results": results, "diff": diff}))
    }
}
