//! `doc.status`: what each open document and its file on disk look like (the MCP `diagnostics` tool).

use serde_json::{Value, json};

use super::CommandSpec;
use crate::spec;

pub fn specs() -> Vec<CommandSpec> {
    vec![spec!(
        "doc.status",
        "Document status",
        "{}  → [{index, id, name, path?, dirty, changedOnDisk (the file changed on disk since this tab read or saved it), lastSavedMs (the file's modification time, Unix milliseconds; null without a file), active}]: read-only; `path` as in doc.list, only for the user's own scripts, not AI agents or plugins",
        false,
        |s, _| {
            // Where the files are on disk is the user's: an agent gets their names (#1117).
            let paths = s.agent.is_none();
            Ok(Value::Array(
                s.docs
                    .iter()
                    .enumerate()
                    .map(|(i, d)| {
                        let saved = d
                            .path
                            .as_deref()
                            .and_then(|p| crate::file_access::metadata(p).ok())
                            .and_then(|m| m.modified().ok())
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .and_then(|t| u64::try_from(t.as_millis()).ok());
                        let mut e = json!({
                            "index": i,
                            "id": d.uid,
                            "name": d.doc.name,
                            "dirty": d.is_dirty(),
                            "changedOnDisk": d.changed_on_disk(),
                            "lastSavedMs": saved,
                            "active": i == s.active,
                        });
                        if paths {
                            e["path"] = json!(d.path);
                        }
                        e
                    })
                    .collect(),
            ))
        }
    )]
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::Session;

    #[test]
    fn lists_every_open_file_with_its_state_and_hides_paths_from_agents() {
        let mut s = Session::new();
        let list = s.execute("doc.status", &json!({})).unwrap();
        let first = &list[0];
        assert_eq!(
            (first["index"].clone(), first["dirty"].clone(), first["changedOnDisk"].clone(), first["lastSavedMs"].clone(), first["active"].clone()),
            (json!(0), json!(false), json!(false), json!(null), json!(true))
        );
        assert_eq!(first["path"], json!(null), "the user's own call gets the (empty) path");
        s.execute("file.new", &json!({"name": "Second"})).unwrap();
        let as_agent = s.execute_as(Some("Claude"), "doc.status", &json!({})).unwrap();
        let rows = as_agent.as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[1]["name"].clone(), rows[1]["active"].clone(), rows[0]["active"].clone()), (json!("Second"), json!(true), json!(false)));
        assert!(rows.iter().all(|r| r.get("path").is_none()), "{as_agent}");
    }
}
