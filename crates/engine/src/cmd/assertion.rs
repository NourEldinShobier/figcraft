//! `batch.assert`: a check of the document that fails when it is false. In a `command.batch` it is
//! written `{"assert": {...}}` in place of `command`/`params`, and a failed assertion rolls the
//! whole batch back like any failed step. Each assertion reuses the command that reports the
//! problem: `inspect.overflow`, `lint.layout` (overlap), `a11y.checkContrast`, `node.find`.

use serde_json::{Map, Value, json};

use super::CommandSpec;
use crate::{Result, Session, bad, spec};

const CMD: &str = "batch.assert";
/// Offenders named in a failure message.
const SHOWN: usize = 3;
const KINDS: [&str; 6] = ["noOverflow", "noOverlap", "contrast", "count", "exists", "none"];

pub fn specs() -> Vec<CommandSpec> {
    vec![spec!(
        "batch.assert",
        "Assert document state",
        "{noOverflow? ({root?, page?}), noOverlap? ({root?, page?}), contrast? ({root?}), count? ({where, min?, max?, equals?}), exists? ({where}), none? ({where})}  checks the document as it is now and fails with the reason when an assertion is false; several keys must all hold → {ok: true, checked: [names]}. noOverflow: nothing overflows (inspect.overflow); noOverlap: no overlapping siblings (lint.layout overlap); contrast: no text fails WCAG AA (a11y.checkContrast; `root` limits it to a subtree); count: the layers `where` (a node.find filter) finds number min..max or equals; exists: at least one; none: zero. In a command.batch, write the step as {\"assert\": {...}}",
        false,
        assert
    )]
}

fn assert(s: &mut Session, p: &Value) -> Result<Value> {
    let o = p.as_object().filter(|o| !o.is_empty()).ok_or_else(|| bad(CMD, format!("give at least one assertion: {}", KINDS.join(", "))))?;
    if let Some(k) = o.keys().find(|k| !KINDS.contains(&k.as_str())) {
        return Err(bad(CMD, format!("unknown assertion `{k}`; use {}", KINDS.join(", "))));
    }
    let mut checked = Vec::new();
    for kind in KINDS {
        let Some(arg) = o.get(kind) else { continue };
        // `true` stands for no parameters.
        let args = match arg {
            Value::Bool(true) => Map::new(),
            Value::Object(a) => a.clone(),
            _ => return Err(bad(CMD, format!("`{kind}` must be an object (or true when it takes no parameters)"))),
        };
        if let Some(why) = check(s, kind, &args)? {
            return Err(bad(CMD, format!("assertion `{kind}` failed: {why}")));
        }
        checked.push(kind);
    }
    Ok(json!({"ok": true, "checked": checked}))
}

fn only(kind: &str, a: &Map<String, Value>, allowed: &[&str]) -> Result<()> {
    match a.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(k) => Err(bad(CMD, format!("`{kind}` takes {}, not `{k}`", allowed.join(", ")))),
        None => Ok(()),
    }
}

fn pick(a: &Map<String, Value>, keys: &[&str]) -> Value {
    Value::Object(keys.iter().filter_map(|k| a.get(*k).map(|v| ((*k).to_string(), v.clone()))).collect())
}

/// The first few `key` values of `items`, for a failure message.
fn names(items: &[Value], key: &str) -> String {
    let text = |i: &Value| match i.get(key) {
        Some(Value::String(t)) => t.clone(),
        Some(v) => v.to_string(),
        None => i.to_string(),
    };
    items.iter().take(SHOWN).map(text).collect::<Vec<_>>().join("; ")
}

fn list<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key).and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default()
}

/// `None` when the assertion holds, else why it does not.
fn check(s: &mut Session, kind: &str, a: &Map<String, Value>) -> Result<Option<String>> {
    let total_of = |v: &Value| v.get("total").and_then(Value::as_u64).unwrap_or(0);
    Ok(match kind {
        "noOverflow" => {
            only(kind, a, &["root", "page"])?;
            let mut p = pick(a, &["root", "page"]);
            p["limit"] = json!(SHOWN);
            let r = s.execute("inspect.overflow", &p)?;
            (total_of(&r) > 0).then(|| format!("{} overflow problem(s), e.g. {}", total_of(&r), names(list(&r, "items"), "detail")))
        }
        "noOverlap" => {
            only(kind, a, &["root", "page"])?;
            let mut p = pick(a, &["root", "page"]);
            p["checks"] = json!(["overlap"]);
            p["limit"] = json!(SHOWN);
            let r = s.execute("lint.layout", &p)?;
            (total_of(&r) > 0).then(|| format!("{} overlap(s), e.g. {}", total_of(&r), names(list(&r, "findings"), "message")))
        }
        "contrast" => {
            only(kind, a, &["root"])?;
            let p = match a.get("root") {
                Some(root) => json!({"ids": [root]}),
                None => json!({}),
            };
            let r = s.execute("a11y.checkContrast", &p)?;
            let fails = list(&r, "failures");
            (!fails.is_empty()).then(|| format!("{} text layer(s) below AA contrast, e.g. {}", fails.len(), names(fails, "name")))
        }
        "count" | "exists" | "none" => {
            let bounds: &[&str] = if kind == "count" { &["min", "max", "equals"] } else { &[] };
            only(kind, a, &[&["where"], bounds].concat())?;
            let filter = a.get("where").filter(|w| w.is_object()).ok_or_else(|| bad(CMD, format!("`{kind}` needs `where`, a node.find filter")))?;
            let bound = |k: &str| -> Result<Option<u64>> {
                a.get(k).map(|v| v.as_u64().ok_or_else(|| bad(CMD, format!("`{k}` must be a whole number, 0 or more")))).transpose()
            };
            let (min, max, equals) = match kind {
                "exists" => (Some(1), None, None),
                "none" => (None, None, Some(0)),
                _ => (bound("min")?, bound("max")?, bound("equals")?),
            };
            if kind == "count" && min.is_none() && max.is_none() && equals.is_none() {
                return Err(bad(CMD, "`count` needs `min`, `max` or `equals`"));
            }
            let mut f = filter.clone();
            if let Some(o) = f.as_object_mut() {
                o.entry("limit").or_insert(json!(0));
            }
            let total = total_of(&s.execute("node.find", &f)?);
            let want = match (equals, min, max) {
                (Some(e), _, _) => format!("exactly {e}"),
                (None, Some(lo), Some(hi)) => format!("{lo} to {hi}"),
                (None, Some(lo), None) => format!("at least {lo}"),
                (None, None, Some(hi)) => format!("at most {hi}"),
                (None, None, None) => String::new(),
            };
            let ok = equals.is_none_or(|e| total == e) && min.is_none_or(|m| total >= m) && max.is_none_or(|m| total <= m);
            (!ok).then(|| format!("{total} layer(s) match, expected {want}"))
        }
        _ => None,
    })
}
