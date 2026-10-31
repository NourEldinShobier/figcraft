//! Plain-text design source (`docs/source-format.md`): `file.exportSource` and `file.importSource`.

use std::path::Path;

use figcraft_doc::Document;
use figcraft_format::source::{Imported, attach_image, export_source, import_source};
use serde_json::{Value, json};

use super::load::{import_summary, read_capped, write};
use crate::cmd::CommandSpec;
use crate::params::string;
use crate::{DocState, EngineError, Result, Session, bad, file_access, spec};

/// Longest source text read.
const MAX_SOURCE: u64 = 512 << 20;

/// The text file inside a source folder.
pub const SOURCE_FILE: &str = "design.jsonl";

fn other(e: impl std::fmt::Display) -> EngineError {
    EngineError::Other(e.to_string())
}

/// A path with no extension (or an existing folder) names a source folder, else one text file.
fn is_folder(path: &str) -> bool {
    file_access::is_dir(path) || Path::new(path).extension().is_none()
}

fn join(dir: &str, parts: &[&str]) -> String {
    parts.iter().fold(Path::new(dir).to_path_buf(), |p, s| p.join(s)).to_string_lossy().into_owned()
}

/// The source of `doc`, written to `out`: one text file, or a folder holding `design.jsonl` and the
/// document's images under `images/<hash>`. Returns the number of lines.
pub fn write_source(doc: &Document, out: &str) -> Result<usize> {
    let text = export_source(doc).map_err(other)?;
    if is_folder(out) {
        file_access::create_dir_all(out)?;
        write(&join(out, &[SOURCE_FILE]), text.as_bytes())?;
        if !doc.images.is_empty() {
            file_access::create_dir_all(join(out, &["images"]))?;
            for (hash, img) in &doc.images {
                write(&join(out, &["images", hash]), &img.bytes)?;
            }
        }
    } else {
        write(out, text.as_bytes())?;
    }
    Ok(text.lines().count())
}

/// Source read from `path` (a text file, or a folder as [`write_source`] makes) or from `text`
/// alone. Images the source lists are loaded from `images/` next to the file; the others are named
/// in the returned warnings.
pub fn read_source(path: Option<&str>, text: Option<&str>) -> Result<(Document, Vec<String>)> {
    let (text, dir) = match (text, path) {
        (Some(t), _) => (t.to_string(), None),
        (None, Some(p)) => {
            let (file, dir) = if is_folder(p) {
                (join(p, &[SOURCE_FILE]), Some(p.to_string()))
            } else {
                (p.to_string(), Path::new(p).parent().map(|d| d.to_string_lossy().into_owned()))
            };
            let bytes = read_capped(&file, MAX_SOURCE)?;
            (String::from_utf8(bytes).map_err(|_| other("source is not UTF-8 text"))?, dir)
        }
        (None, None) => return Err(bad("file.importSource", "give `source` (the text) or `path`")),
    };
    let Imported { mut doc, mut issues, images } = import_source(&text).map_err(other)?;
    let mut missing = 0usize;
    for img in images {
        // The hash becomes a file name: only hex digits.
        let loaded = (img.hash.len() <= 64 && img.hash.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| dir.as_deref().and_then(|d| read_capped(&join(d, &["images", &img.hash]), 256 << 20).ok()))
            .flatten()
            .and_then(|bytes| attach_image(&mut doc, &img.hash, bytes).ok());
        if loaded.is_none() {
            missing += 1;
        }
    }
    if missing > 0 {
        issues.push(format!("{missing} image(s) the source lists were not found next to it (images/<hash>); layers using them show nothing"));
    }
    Ok((doc, issues))
}

fn export_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let doc = s.doc()?;
    match string(p, "path") {
        Some(path) => {
            let lines = write_source(doc, &path)?;
            Ok(json!({"path": path, "lines": lines, "images": doc.images.len()}))
        }
        None => {
            let source = export_source(doc).map_err(other)?;
            Ok(json!({"lines": source.lines().count(), "source": source, "images": doc.images.keys().collect::<Vec<_>>()}))
        }
    }
}

fn import_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let (doc, issues) = read_source(string(p, "path").as_deref(), string(p, "source").as_deref())?;
    let summary = import_summary(&doc);
    s.docs.push(DocState::new(doc, None).unsaved());
    s.active = s.docs.len() - 1;
    Ok(json!({"index": s.active, "summary": summary, "warnings": issues}))
}


pub fn specs() -> Vec<CommandSpec> {
    vec![
        spec!(
            "file.exportSource",
            "Export plain-text source",
            "{path? (a file ending in an extension: the text; no extension or an existing folder: design.jsonl plus images/<hash>; without a path the text comes back as `source`)}  the document as canonical JSON Lines, one object per line with sorted keys and no volatile fields: saving twice gives identical bytes and a changed fill changes one line (docs/source-format.md). Returns {lines, images, source? | path}",
            false,
            export_cmd
        ),
        spec!(
            "file.importSource",
            "Import plain-text source",
            "{source (the text) | path (a text file, or a folder with design.jsonl and images/)}  rebuilds the document from file.exportSource's text as a new unsaved tab; images load from images/<hash> next to the file (missing ones are named in `warnings`). Returns {index, summary, warnings}",
            false,
            import_cmd
        ),
    ]
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::Session;

    #[test]
    fn source_exports_imports_into_a_new_tab_and_exports_the_same_text() {
        let mut s = Session::new();
        s.execute("node.create", &json!({"type": "frame", "key": "card", "width": 80, "height": 40, "props": {"fills": ["#FF0000"]}})).unwrap();
        let a = s.execute("file.exportSource", &json!({})).unwrap();
        let text = a["source"].as_str().unwrap().to_string();
        assert_eq!(s.execute("file.exportSource", &json!({})).unwrap()["source"], json!(text));
        let r = s.execute("file.importSource", &json!({"source": text})).unwrap();
        assert_eq!(r["index"], json!(1));
        assert_eq!(r["summary"]["layers"], json!(1));
        assert_eq!(s.execute("file.exportSource", &json!({})).unwrap()["source"], json!(text));
        assert_eq!(s.execute("node.find", &json!({"key": "card"})).unwrap()["total"], json!(1));
    }

    #[test]
    fn a_folder_holds_the_text_and_a_file_path_with_an_extension_holds_only_the_text() {
        let mut s = Session::new();
        s.execute("node.create", &json!({"type": "rectangle", "name": "R"})).unwrap();
        let base = std::env::temp_dir().join(format!("figcraft-source-{}", std::process::id()));
        let dir = base.join("design").to_string_lossy().into_owned();
        let file = base.join("one.jsonl").to_string_lossy().into_owned();
        s.execute("file.exportSource", &json!({"path": dir})).unwrap();
        s.execute("file.exportSource", &json!({"path": file})).unwrap();
        assert_eq!(std::fs::read_to_string(base.join("design").join("design.jsonl")).unwrap(), std::fs::read_to_string(&file).unwrap());
        assert_eq!(s.execute("file.importSource", &json!({"path": dir})).unwrap()["summary"]["layers"], json!(1));
        let _ = std::fs::remove_dir_all(&base);
    }
}
