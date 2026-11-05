//! Named lists of layers: an agent saves the layers a search found (`list.save`) and later acts on
//! them with the generic `list` parameter (`{"command": "node.set", "params": {"list": "buttons",
//! "props": {...}}}`). A list belongs to one file tab, lives in memory only (it is neither saved in
//! the file nor undoable) and goes when the tab closes; layers deleted since drop out of it when
//! it is read.

use std::collections::BTreeSet;

use figcraft_doc::NodeId;
use serde_json::{Value, json};

use super::CommandSpec;
use crate::doc_state::DocState;
use crate::params::{boolean, req_targets, string};
use crate::{Result, Session, bad, spec};

/// Most lists one tab keeps.
const MAX_LISTS: usize = 1000;
/// Longest list name, in characters.
const MAX_NAME: usize = 200;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        spec!(
            "list.save",
            "Save layer list",
            "{name, ids?, add? (bool: add to the list instead of replacing it)}  keeps the layers under `name` for this file's tab (not saved in the file, gone when the tab closes): `ids`, the layers a `where` filter finds, or the selection; `list: name` on any command then acts on them → {name, count}",
            false,
            list_save
        ),
        spec!("list.get", "Get layer list", "{name} → {name, ids}  (the layers of the list that still exist)", false, list_get),
        spec!("list.delete", "Delete layer list", "{name} → {name, deleted}", false, list_delete),
        spec!("list.all", "All layer lists", "{} → {lists: [{name, count}]}", false, list_all),
    ]
}

/// The error for a list that isn't there, naming the ones that are.
pub(crate) fn unknown_list(ds: &DocState, name: &str) -> String {
    const SHOWN: usize = 20;
    let known: Vec<&str> = ds.id_lists.keys().map(String::as_str).take(SHOWN).collect();
    if known.is_empty() {
        return format!("no list named \"{name}\" (none saved yet: list.save makes one)");
    }
    let more = ds.id_lists.len().saturating_sub(known.len());
    let more = if more > 0 { format!(", {more} more") } else { String::new() };
    format!("no list named \"{name}\" (known lists: {}{more})", known.join(", "))
}

fn name_of(cmd: &str, p: &Value) -> Result<String> {
    let name = string(p, "name").map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
    let name = name.ok_or_else(|| bad(cmd, "`name` must be a non-empty string"))?;
    if name.chars().count() > MAX_NAME {
        return Err(bad(cmd, format!("`name` is longer than {MAX_NAME} characters")));
    }
    Ok(name)
}

fn list_save(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "list.save";
    let name = name_of(CMD, p)?;
    let add = boolean(p, "add").unwrap_or(false);
    let ids = req_targets(CMD, s, p)?;
    let ds = s.ds_mut()?;
    if !ds.id_lists.contains_key(&name) && ds.id_lists.len() >= MAX_LISTS {
        return Err(bad(CMD, format!("a file keeps at most {MAX_LISTS} lists: delete one with list.delete")));
    }
    let list = ds.id_lists.entry(name.clone()).or_default();
    if !add {
        list.clear();
    }
    let mut seen: BTreeSet<NodeId> = list.iter().copied().collect();
    list.extend(ids.into_iter().filter(|i| seen.insert(*i)));
    Ok(json!({"name": name, "count": list.len()}))
}

fn list_get(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "list.get";
    let name = name_of(CMD, p)?;
    let ds = s.ds()?;
    let ids = ds.id_lists.get(&name).ok_or_else(|| bad(CMD, unknown_list(ds, &name)))?;
    let live: Vec<NodeId> = ids.iter().copied().filter(|i| ds.doc.contains(*i)).collect();
    Ok(json!({"name": name, "ids": live}))
}

fn list_delete(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "list.delete";
    let name = name_of(CMD, p)?;
    let ds = s.ds_mut()?;
    if ds.id_lists.remove(&name).is_none() {
        return Err(bad(CMD, unknown_list(ds, &name)));
    }
    Ok(json!({"name": name, "deleted": true}))
}

fn list_all(s: &mut Session, _p: &Value) -> Result<Value> {
    let ds = s.ds()?;
    let lists: Vec<Value> =
        ds.id_lists.iter().map(|(name, ids)| json!({"name": name, "count": ids.iter().filter(|i| ds.doc.contains(**i)).count()})).collect();
    Ok(json!({"lists": lists}))
}
