//! `library.setGuidelines` / `library.guidelines`: Markdown design rules stored in the file that
//! designers and agents both read (also surfaced as a `get_design_context` hint).

use serde_json::{Value, json};

use super::CommandSpec;
use crate::params::get;
use crate::{Result, Session, bad, spec};

/// Most bytes of guideline text.
pub const MAX_BYTES: usize = 20 * 1024;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        spec!(
            "library.setGuidelines",
            "Set design guidelines",
            "{text (Markdown, up to 20 KB; \"\" clears)}  stores the design rules with the file, where people and agents read them → {bytes}",
            true,
            set
        ),
        spec!("library.guidelines", "Design guidelines", "{}  → {text, bytes}: read-only", false, read),
    ]
}

fn set(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "library.setGuidelines";
    let Some(Value::String(text)) = get(p, "text") else {
        return Err(bad(CMD, "`text` must be a string"));
    };
    if text.len() > MAX_BYTES {
        return Err(bad(CMD, format!("`text` is {} bytes; the limit is {MAX_BYTES}", text.len())));
    }
    let bytes = text.len();
    s.doc_mut()?.guidelines = text.clone();
    Ok(json!({"bytes": bytes}))
}

fn read(s: &mut Session, _: &Value) -> Result<Value> {
    let g = &s.doc()?.guidelines;
    Ok(json!({"text": g, "bytes": g.len()}))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::Session;

    #[test]
    fn set_read_undo_and_limits() {
        let mut s = Session::new();
        assert_eq!(s.execute("library.guidelines", &json!({})).unwrap(), json!({"text": "", "bytes": 0}));
        assert_eq!(s.execute("library.setGuidelines", &json!({"text": "# Rules\nUse tokens."})).unwrap(), json!({"bytes": 19}));
        assert_eq!(s.execute("library.guidelines", &json!({})).unwrap()["text"], "# Rules\nUse tokens.");
        s.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(s.execute("library.guidelines", &json!({})).unwrap()["bytes"], 0);
        assert!(s.execute("library.setGuidelines", &json!({"text": "x".repeat(20 * 1024 + 1)})).is_err());
        assert!(s.execute("library.setGuidelines", &json!({"text": "x".repeat(20 * 1024)})).is_ok());
    }

    #[test]
    fn junk_parameters_are_errors() {
        let mut s = Session::new();
        for p in [json!({}), json!({"text": 5}), json!({"text": null}), json!({"text": ["a"]}), json!(7), json!(null)] {
            assert!(s.execute("library.setGuidelines", &p).is_err(), "{p}");
        }
    }

    #[test]
    fn appear_as_a_design_context_hint() {
        let mut s = Session::new();
        let f = s.execute("frame.create", &json!({"x": 0, "y": 0, "width": 10, "height": 10})).unwrap();
        s.execute("library.setGuidelines", &json!({"text": "Buttons are 44px tall."})).unwrap();
        let d = s.execute("inspect.designContext", &json!({"id": f["id"], "include": "hints"})).unwrap();
        assert!(d["hints"][0].as_str().unwrap().contains("Buttons are 44px tall."), "{d}");
    }

    #[test]
    fn survive_save_and_reopen_and_old_files_open() {
        let mut s = Session::new();
        s.execute("library.setGuidelines", &json!({"text": "Spacing: 8pt grid."})).unwrap();
        let bytes = figcraft_format::save_bytes(s.doc().unwrap(), None).unwrap();
        let back = figcraft_format::load_bytes(&bytes).unwrap().0;
        assert_eq!(back.guidelines, "Spacing: 8pt grid.");
        let plain = figcraft_format::save_bytes(Session::new().doc().unwrap(), None).unwrap();
        assert_eq!(figcraft_format::load_bytes(&plain).unwrap().0.guidelines, "");
    }
}
