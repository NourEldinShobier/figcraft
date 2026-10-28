//! Design-token export and import from variables: the Design Tokens Format Module (DTCG, 2025.10),
//! CSS custom properties per mode, a Tailwind theme extension, and DTCG back into variables.
//!
//! DTCG form: one tokens file per collection and mode (a file is a complete standalone token tree;
//! the format module leaves modes to a separate resolver document, so this keeps each file valid
//! on its own). Variable names split on `/` into groups; a variable that is also a group gets
//! `$root`. Colours are `{colorSpace: "srgb", components, alpha?, hex}`, lengths
//! `{value, unit: "px"}`, other numbers plain; strings and booleans use the `string` and `boolean`
//! types, which the format does not define (tools that only know the standard types ignore them).
//! An alias is `{group.token}`; one into another collection is `{Collection.group.token}`.

use std::collections::{BTreeMap, HashMap, HashSet};

use figcraft_color::Color;
use figcraft_doc::*;
use serde_json::{Map, Value, json};

use super::CommandSpec;
use crate::library::local_collections;
use crate::params::*;
use crate::token_roles::{NumberRole, Roles};
use crate::{Result, Session, bad, spec};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        spec!(
            "variable.exportTokens",
            "Export design tokens",
            "{collection? [id or name; default every collection of this file, not library copies], format? (dtcg (default): Design Tokens Format Module files, one per collection and mode | css: custom properties, the default mode on :root and each other mode on [data-theme=\"<mode>\"] | tailwind: a `theme.extend` object whose values are var(--name) plus the css that defines them | json: the local variables JSON), modes? [mode names or ids; default all], path? (a folder for dtcg files, a file for the other formats)}  → dtcg: {files: [{name, collection, mode, tokens}]} (+ paths when written) | css, tailwind, json: {text} (tailwind also {theme})",
            false,
            export_tokens
        ),
        spec!(
            "variable.importTokens",
            "Import design tokens",
            "{tokens (a DTCG tokens file: object, JSON text or a path) | files [{tokens, mode?}] (one file per mode), collection? (name; default \"Tokens\"; an existing collection of that name is merged into), mode? (name of the mode a single `tokens` is; default the collection's first)}  colour (srgb components or hex), dimension (px, rem), number, fontWeight, fontFamily, string and boolean tokens become variables, `{group.token}` aliases are kept; other types are skipped with a warning  → {collections, variables, updated, warnings}",
            true,
            import_tokens
        ),
    ]
}

const MAX_TOKENS: usize = 20_000;
const MAX_FILE_BYTES: u64 = 32 << 20;

fn clean_segment(s: &str) -> String {
    let t: String = s.trim().chars().map(|c| if matches!(c, '.' | '{' | '}') || c.is_control() { '-' } else { c }).collect();
    let t = t.trim_start_matches('$').to_string();
    if t.is_empty() { "_".into() } else { t }
}

/// Each variable of `c` as its token path: names split on `/`, unique, `$root` where a path is also a group.
fn paths_for(doc: &Document, c: &Collection) -> HashMap<VariableId, Vec<String>> {
    let vars: Vec<&Variable> = doc.variables.variables.iter().filter(|v| v.collection == c.id).collect();
    let mut seen: HashSet<Vec<String>> = HashSet::new();
    let mut paths: Vec<Vec<String>> = Vec::with_capacity(vars.len());
    for v in &vars {
        let mut p: Vec<String> = v.name.split('/').map(clean_segment).collect();
        while !seen.insert(p.clone()) {
            if let Some(last) = p.last_mut() {
                *last = format!("{last}-2");
            }
        }
        paths.push(p);
    }
    let mut groups: HashSet<Vec<String>> = HashSet::new();
    for p in &paths {
        for n in 1..p.len() {
            groups.insert(p[..n].to_vec());
        }
    }
    for p in &mut paths {
        if groups.contains(p) {
            p.push("$root".into());
        }
    }
    vars.iter().map(|v| v.id.clone()).zip(paths).collect()
}

fn insert(tree: &mut Map<String, Value>, path: &[String], token: Value) {
    match path {
        [] => {}
        [leaf] => {
            tree.insert(leaf.clone(), token);
        }
        [head, rest @ ..] => {
            if let Value::Object(m) = tree.entry(head.clone()).or_insert_with(|| Value::Object(Map::new())) {
                insert(m, rest, token);
            }
        }
    }
}

fn round5(x: f32) -> f64 {
    (f64::from(x) * 1e5).round() / 1e5
}

fn color_value(c: Color) -> Value {
    let mut o = Map::new();
    o.insert("colorSpace".into(), json!("srgb"));
    o.insert("components".into(), json!([round5(c.r), round5(c.g), round5(c.b)]));
    if c.a < 1.0 {
        o.insert("alpha".into(), json!(round5(c.a)));
    }
    o.insert("hex".into(), json!(format!("#{}", c.to_hex())));
    Value::Object(o)
}

fn type_name(v: &Variable, roles: &Roles) -> &'static str {
    match v.kind {
        VariableType::Color => "color",
        VariableType::Number if roles.role(v) == NumberRole::Unitless => "number",
        VariableType::Number => "dimension",
        VariableType::String => "string",
        VariableType::Boolean => "boolean",
    }
}

struct Ctx<'a> {
    doc: &'a Document,
    roles: Roles,
    paths: HashMap<CollectionId, HashMap<VariableId, Vec<String>>>,
}

impl Ctx<'_> {
    fn reference(&self, from: &Collection, target: &Variable) -> Option<String> {
        let path = self.paths.get(&target.collection)?.get(&target.id)?.join(".");
        if target.collection == from.id {
            return Some(format!("{{{path}}}"));
        }
        let other = self.doc.variables.collection(&target.collection)?;
        Some(format!("{{{}.{path}}}", clean_segment(&other.name)))
    }

    fn token(&self, c: &Collection, v: &Variable, mode: &Mode) -> Option<Value> {
        let val = v.values.get(&mode.id).or_else(|| c.default_mode().and_then(|d| v.values.get(d)))?;
        let mut t = Map::new();
        let ty = match val {
            VariableValue::Alias(a) => {
                let target = self.doc.variables.get(a)?;
                t.insert("$value".into(), json!(self.reference(c, target)?));
                type_name(target, &self.roles)
            }
            VariableValue::Color(col) => {
                t.insert("$value".into(), color_value(*col));
                "color"
            }
            VariableValue::Number(x) if type_name(v, &self.roles) == "number" => {
                t.insert("$value".into(), json!(x));
                "number"
            }
            VariableValue::Number(x) => {
                t.insert("$value".into(), json!({"value": x, "unit": "px"}));
                "dimension"
            }
            VariableValue::String(s) => {
                t.insert("$value".into(), json!(s));
                "string"
            }
            VariableValue::Boolean(b) => {
                t.insert("$value".into(), json!(b));
                "boolean"
            }
        };
        t.insert("$type".into(), json!(ty));
        if !v.description.is_empty() {
            t.insert("$description".into(), json!(v.description));
        }
        Some(Value::Object(t))
    }
}

fn file_name(c: &Collection, m: &Mode) -> String {
    let safe = |s: &str| -> String { s.chars().map(|ch| if ch.is_alphanumeric() || matches!(ch, '-' | '_' | ' ') { ch } else { '_' }).collect() };
    format!("{}.{}.tokens.json", safe(&c.name), safe(&m.name))
}

fn mode_wanted(only: &Option<Vec<String>>, m: &Mode) -> bool {
    only.as_ref().is_none_or(|o| o.iter().any(|x| *x == m.id || *x == m.name))
}

fn export_tokens(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "variable.exportTokens";
    let srgb = s.doc()?.in_srgb();
    let doc: &Document = &srgb;
    let colls: Vec<&Collection> = match string(p, "collection") {
        Some(k) => vec![doc.variables.collection(&k).ok_or_else(|| bad(cmd, format!("no collection {k}")))?],
        None => local_collections(doc).collect(),
    };
    let only: Option<Vec<String>> = match get(p, "modes") {
        None | Some(Value::Null) => None,
        Some(Value::Array(a)) => Some(a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()),
        Some(_) => return Err(bad(cmd, "modes: [mode names or ids]")),
    };
    let format = string(p, "format").unwrap_or_else(|| "dtcg".into());
    let write_to = |text: &str| -> Result<Value> {
        match string(p, "path") {
            Some(path) => {
                super::file::write(&path, text.as_bytes())?;
                Ok(json!({"path": path}))
            }
            None => Ok(json!({"text": text})),
        }
    };
    match format.as_str() {
        "dtcg" => {
            let cx =
                Ctx { doc, roles: Roles::new(doc), paths: doc.variables.collections.iter().map(|c| (c.id.clone(), paths_for(doc, c))).collect() };
            let mut files = Vec::new();
            for c in &colls {
                for m in c.modes.iter().filter(|m| mode_wanted(&only, m)) {
                    let mut tree = Map::new();
                    for v in doc.variables.variables.iter().filter(|v| v.collection == c.id) {
                        if let (Some(tok), Some(path)) = (cx.token(c, v, m), cx.paths.get(&c.id).and_then(|h| h.get(&v.id))) {
                            insert(&mut tree, path, tok);
                        }
                    }
                    files.push(json!({"name": file_name(c, m), "collection": c.name, "mode": m.name, "tokens": Value::Object(tree)}));
                }
            }
            if files.is_empty() {
                return Err(bad(cmd, "nothing to export: no collection with a matching mode"));
            }
            let mut out = json!({"format": "dtcg", "files": files});
            if let Some(dir) = string(p, "path") {
                let dir = dir.trim_end_matches(['/', '\\']).to_string();
                let mut written = Vec::new();
                for f in &files {
                    let name = f["name"].as_str().unwrap_or("tokens.json");
                    let text = serde_json::to_string_pretty(&f["tokens"]).map_err(|e| crate::EngineError::Other(e.to_string()))?;
                    let target = format!("{dir}/{name}");
                    super::file::write(&target, text.as_bytes())?;
                    written.push(target);
                }
                out["paths"] = json!(written);
            }
            Ok(out)
        }
        "css" => {
            let text = super::variables_io::css_with(doc, &colls, false, Some("theme"), only.as_deref());
            write_to(&text)
        }
        "tailwind" => {
            let theme = crate::tailwind::Theme::new(doc);
            let mut ext: BTreeMap<&str, Map<String, Value>> = BTreeMap::new();
            for c in &colls {
                for v in doc.variables.variables.iter().filter(|v| v.collection == c.id) {
                    let name = theme.name(v);
                    for (prefix, section) in [
                        ("--color-", "colors"),
                        ("--radius-", "borderRadius"),
                        ("--text-", "fontSize"),
                        ("--spacing-", "spacing"),
                        ("--font-", "fontFamily"),
                    ] {
                        if let Some(key) = name.strip_prefix(prefix) {
                            ext.entry(section).or_default().insert(key.to_string(), json!(format!("var({name})")));
                            break;
                        }
                    }
                }
            }
            let css = super::variables_io::css_with(doc, &colls, false, Some("theme"), only.as_deref());
            let mut out = write_to(&css)?;
            out["theme"] = json!({"extend": ext});
            out["format"] = json!("tailwind");
            Ok(out)
        }
        "json" => {
            let text = serde_json::to_string_pretty(&figcraft_format::rest::export_variables(&doc.variables, &colls))
                .map_err(|e| crate::EngineError::Other(e.to_string()))?;
            write_to(&text)
        }
        _ => Err(bad(cmd, "format: dtcg|css|tailwind|json")),
    }
}

// ---------------------------------------------------------------------------------------------
// Import

struct Tok {
    path: String,
    ty: Option<String>,
    value: Value,
    description: String,
}

fn walk(obj: &Map<String, Value>, path: &mut Vec<String>, inherited: Option<&str>, out: &mut Vec<Tok>, warnings: &mut Vec<String>, depth: usize) {
    if depth > 32 || out.len() >= MAX_TOKENS {
        return;
    }
    let ty = obj.get("$type").and_then(Value::as_str).or(inherited);
    if let Some(value) = obj.get("$value") {
        out.push(Tok {
            path: path.join("/"),
            ty: ty.map(str::to_string),
            value: value.clone(),
            description: obj.get("$description").and_then(Value::as_str).unwrap_or_default().to_string(),
        });
        return;
    }
    for (k, v) in obj {
        let Value::Object(child) = v else {
            if !k.starts_with('$') {
                warnings.push(format!("`{}`: not a token or group", path.iter().chain([k]).cloned().collect::<Vec<_>>().join(".")));
            }
            continue;
        };
        if k == "$root" {
            walk(child, path, ty, out, warnings, depth + 1);
        } else if !k.starts_with('$') {
            path.push(k.clone());
            walk(child, path, ty, out, warnings, depth + 1);
            path.pop();
        }
    }
}

enum Parsed {
    Alias(String),
    Val(VariableType, Value),
}

fn number_of(v: &Value) -> Option<f64> {
    v.as_f64().filter(|x| x.is_finite())
}

fn parse_color(v: &Value) -> Option<Color> {
    match v {
        Value::String(s) => Color::parse_css(s),
        Value::Object(o) => {
            let alpha = o.get("alpha").and_then(number_of).unwrap_or(1.0) as f32;
            let space = o.get("colorSpace").and_then(Value::as_str).unwrap_or("srgb");
            if space == "srgb"
                && let Some(Value::Array(c)) = o.get("components")
                && c.len() == 3
            {
                let n = |i: usize| c.get(i).and_then(number_of).map(|x| x as f32);
                return Some(Color::new(n(0)?, n(1)?, n(2)?, alpha));
            }
            // Other colour spaces: the sRGB hex the file carries.
            o.get("hex").and_then(Value::as_str).and_then(Color::from_hex).map(|c| c.with_alpha(alpha))
        }
        _ => None,
    }
}

fn parse_dimension(v: &Value) -> Option<f64> {
    match v {
        Value::Number(_) => number_of(v),
        Value::Object(o) => {
            let x = o.get("value").and_then(number_of)?;
            match o.get("unit").and_then(Value::as_str).unwrap_or("px") {
                "px" => Some(x),
                "rem" => Some(x * 16.0),
                _ => None,
            }
        }
        Value::String(s) => {
            let t = s.trim();
            t.strip_suffix("px")
                .map(|n| n.trim().parse().ok())
                .unwrap_or_else(|| t.strip_suffix("rem").and_then(|n| n.trim().parse::<f64>().ok()).map(|x| x * 16.0))
        }
        _ => None,
    }
}

fn parse_token(t: &Tok) -> std::result::Result<Parsed, String> {
    if let Value::String(s) = &t.value
        && let Some(inner) = s.trim().strip_prefix('{').and_then(|r| r.strip_suffix('}'))
    {
        let inner = inner.strip_suffix(".$root").unwrap_or(inner);
        return Ok(Parsed::Alias(inner.split('.').collect::<Vec<_>>().join("/")));
    }
    let bad_val = || format!("`{}`: not a valid {} value", t.path, t.ty.as_deref().unwrap_or("token"));
    let ty = t.ty.as_deref().unwrap_or(match &t.value {
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::Object(o) if o.contains_key("components") || o.contains_key("hex") => "color",
        Value::Object(o) if o.contains_key("unit") => "dimension",
        Value::String(s) if s.starts_with('#') && Color::from_hex(s).is_some() => "color",
        Value::String(_) => "string",
        _ => "",
    });
    Ok(match ty {
        "color" => {
            let c = parse_color(&t.value).ok_or_else(bad_val)?;
            Parsed::Val(VariableType::Color, json!({"r": c.r, "g": c.g, "b": c.b, "a": c.a}))
        }
        "dimension" => Parsed::Val(VariableType::Number, json!(parse_dimension(&t.value).ok_or_else(bad_val)?)),
        "number" | "fontWeight" => Parsed::Val(VariableType::Number, json!(number_of(&t.value).ok_or_else(bad_val)?)),
        "fontFamily" => match &t.value {
            Value::String(s) => Parsed::Val(VariableType::String, json!(s)),
            Value::Array(a) => Parsed::Val(VariableType::String, json!(a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "))),
            _ => return Err(bad_val()),
        },
        "string" => match &t.value {
            Value::String(s) => Parsed::Val(VariableType::String, json!(s)),
            _ => return Err(bad_val()),
        },
        "boolean" => match &t.value {
            Value::Bool(b) => Parsed::Val(VariableType::Boolean, json!(b)),
            _ => return Err(bad_val()),
        },
        other => return Err(format!("`{}`: type `{other}` isn't imported", t.path)),
    })
}

fn tokens_value(cmd: &str, v: &Value) -> Result<Map<String, Value>> {
    let parsed = match v {
        Value::Object(_) => v.clone(),
        Value::String(s) if s.trim_start().starts_with('{') => serde_json::from_str(s).map_err(|e| bad(cmd, format!("tokens: {e}")))?,
        Value::String(path) => {
            let bytes = super::file::read_capped(path, MAX_FILE_BYTES)?;
            serde_json::from_slice(&bytes).map_err(|e| bad(cmd, format!("tokens: {e}")))?
        }
        _ => return Err(bad(cmd, "tokens: a DTCG file as an object, JSON text or a path")),
    };
    match parsed {
        Value::Object(m) => Ok(m),
        _ => Err(bad(cmd, "tokens: a DTCG file is a JSON object")),
    }
}

fn import_tokens(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "variable.importTokens";
    // (mode name, tokens) per file.
    let mut inputs: Vec<(Option<String>, Map<String, Value>)> = Vec::new();
    match (get(p, "files"), get(p, "tokens")) {
        (Some(Value::Array(a)), _) => {
            for f in a.iter().take(64) {
                let t = get(f, "tokens").ok_or_else(|| bad(cmd, "files: [{tokens, mode?}]"))?;
                inputs.push((string(f, "mode"), tokens_value(cmd, t)?));
            }
        }
        (Some(Value::Null) | None, Some(t)) if !t.is_null() => inputs.push((string(p, "mode"), tokens_value(cmd, t)?)),
        _ => return Err(bad(cmd, "`tokens` (a DTCG file) or `files` ([{tokens, mode?}]) required")),
    }
    let cname = string(p, "collection").unwrap_or_else(|| "Tokens".into());
    let first_mode = s
        .doc()?
        .variables
        .collections
        .iter()
        .find(|c| c.name == cname && c.extends.is_none())
        .and_then(|c| c.modes.first().map(|m| m.name.clone()))
        .unwrap_or_else(|| "Mode 1".into());

    let mut warnings: Vec<String> = Vec::new();
    // Modes in order of first appearance, and per mode its tokens.
    let mut modes: Vec<String> = Vec::new();
    let mut per_mode: Vec<Vec<Tok>> = Vec::new();
    for (mode, tree) in &inputs {
        let mode = mode.clone().unwrap_or_else(|| first_mode.clone());
        let mut toks = Vec::new();
        walk(tree, &mut Vec::new(), None, &mut toks, &mut warnings, 0);
        match modes.iter().position(|m| *m == mode) {
            Some(i) => per_mode[i].extend(toks),
            None => {
                modes.push(mode);
                per_mode.push(toks);
            }
        }
    }
    // Parse every token; the variable's type comes from its first non-alias value.
    let mut order: Vec<String> = Vec::new();
    let mut kinds: BTreeMap<String, VariableType> = BTreeMap::new();
    let mut descriptions: BTreeMap<String, String> = BTreeMap::new();
    let mut parsed: Vec<Vec<(String, Parsed)>> = Vec::new();
    for toks in &per_mode {
        let mut list = Vec::new();
        for t in toks {
            match parse_token(t) {
                Ok(pt) => {
                    if !order.contains(&t.path) {
                        order.push(t.path.clone());
                    }
                    if let Parsed::Val(k, _) = &pt {
                        match kinds.get(&t.path) {
                            Some(prev) if prev != k => {
                                warnings.push(format!("`{}` is a different type in another mode: skipped here", t.path));
                                continue;
                            }
                            _ => {
                                kinds.insert(t.path.clone(), *k);
                            }
                        }
                    }
                    if !t.description.is_empty() {
                        descriptions.insert(t.path.clone(), t.description.clone());
                    }
                    list.push((t.path.clone(), pt));
                }
                Err(w) => warnings.push(w),
            }
        }
        parsed.push(list);
    }
    // Alias-only variables take the type of what they point at.
    let alias_of: BTreeMap<&str, &str> =
        parsed.iter().flatten().filter_map(|(path, pt)| if let Parsed::Alias(a) = pt { Some((path.as_str(), a.as_str())) } else { None }).collect();
    for path in &order {
        if kinds.contains_key(path) {
            continue;
        }
        let mut cur = path.as_str();
        for _ in 0..32 {
            match alias_of.get(cur) {
                Some(next) => cur = next,
                None => break,
            }
            if let Some(k) = kinds.get(cur) {
                kinds.insert(path.clone(), *k);
                break;
            }
        }
    }
    let ids: BTreeMap<&str, String> =
        order.iter().filter(|p| kinds.contains_key(*p)).enumerate().map(|(i, p)| (p.as_str(), format!("t{i}"))).collect();
    let mode_ids: Vec<String> = (0..modes.len()).map(|i| format!("m{i}")).collect();
    let mut values: BTreeMap<&str, Map<String, Value>> = BTreeMap::new();
    for (mi, list) in parsed.iter().enumerate() {
        for (path, pt) in list {
            let Some(mode_id) = mode_ids.get(mi) else { continue };
            if !ids.contains_key(path.as_str()) {
                warnings.push(format!("`{path}`: its alias target has no value: skipped"));
                continue;
            }
            let v = match pt {
                Parsed::Val(_, v) => v.clone(),
                Parsed::Alias(target) => match ids.get(target.as_str()) {
                    Some(id) => json!({"type": "VARIABLE_ALIAS", "id": id}),
                    None => {
                        warnings.push(format!("`{path}`: alias target `{}` isn't in these tokens: skipped", target.replace('/', ".")));
                        continue;
                    }
                },
            };
            values.entry(path.as_str()).or_default().insert(mode_id.clone(), v);
        }
    }
    if ids.is_empty() {
        return Err(bad(cmd, "no tokens found (expected tokens with `$value`)"));
    }
    let var_ids: Vec<&String> = ids.values().collect();
    let mut vars = Map::new();
    for (path, id) in &ids {
        let kind = match kinds.get(*path) {
            Some(VariableType::Color) => "COLOR",
            Some(VariableType::Number) => "FLOAT",
            Some(VariableType::String) => "STRING",
            _ => "BOOLEAN",
        };
        vars.insert(
            id.clone(),
            json!({"id": id, "name": path, "key": id, "variableCollectionId": "c0", "resolvedType": kind,
                   "valuesByMode": values.get(path).cloned().unwrap_or_default(), "remote": false,
                   "description": descriptions.get(*path).cloned().unwrap_or_default(), "hiddenFromPublishing": false,
                   "scopes": ["ALL_SCOPES"], "codeSyntax": {}}),
        );
    }
    let mode_list: Vec<Value> = mode_ids.iter().zip(&modes).map(|(id, name)| json!({"modeId": id, "name": name})).collect();
    let rest = json!({"meta": {
        "variableCollections": {"c0": {"id": "c0", "name": cname, "key": "c0", "modes": mode_list, "defaultModeId": mode_ids.first(),
                                        "remote": false, "hiddenFromPublishing": false, "variableIds": var_ids}},
        "variables": vars}});
    let mut out = super::variables_io::import(s, &json!({"variables": rest}))?;
    if let Some(Value::Array(w)) = out.get_mut("warnings") {
        w.extend(warnings.into_iter().map(Value::String));
    } else {
        out["warnings"] = json!(warnings);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::Session;

    fn ex(s: &mut Session, cmd: &str, p: Value) -> Value {
        match s.execute(cmd, &p) {
            Ok(v) => v,
            Err(e) => panic!("{cmd} {p}: {e}"),
        }
    }

    fn themed() -> Session {
        let mut s = Session::new();
        ex(&mut s, "variable.createCollection", json!({"name": "Theme", "modes": ["Light", "Dark"]}));
        let mk = |s: &mut Session, name: &str, ty: &str, values: Value| {
            ex(s, "variable.create", json!({"collection": "Theme", "name": name, "type": ty, "values": values}));
        };
        mk(&mut s, "brand/primary", "color", json!({"Light": "#0000FF", "Dark": "#FFFFFF"}));
        mk(&mut s, "brand/primary/hover", "color", json!({"Light": "#0000CC80", "Dark": "#EEEEEE"}));
        mk(&mut s, "space/md", "number", json!({"Light": 16, "Dark": 12}));
        mk(&mut s, "label", "string", json!({"Light": "Hi", "Dark": "Hallo"}));
        mk(&mut s, "dense", "boolean", json!({"Light": false, "Dark": true}));
        mk(&mut s, "link", "color", json!({"Light": "#000000", "Dark": "#000000"}));
        ex(&mut s, "variable.set", json!({"variable": "link", "mode": "Light", "value": {"alias": "brand/primary"}}));
        ex(&mut s, "variable.set", json!({"variable": "link", "mode": "Dark", "value": {"alias": "brand/primary"}}));
        s
    }

    #[test]
    fn dtcg_files_per_mode_have_typed_values_roots_and_aliases() {
        let mut s = themed();
        let r = ex(&mut s, "variable.exportTokens", json!({"format": "dtcg"}));
        let files = r["files"].as_array().unwrap();
        assert_eq!(files.iter().map(|f| f["name"].as_str().unwrap()).collect::<Vec<_>>(), ["Theme.Light.tokens.json", "Theme.Dark.tokens.json"]);
        let light = &files[0]["tokens"];
        assert_eq!(
            light["brand"]["primary"]["$root"],
            json!({"$value": {"colorSpace": "srgb", "components": [0.0, 0.0, 1.0], "hex": "#0000FF"}, "$type": "color"})
        );
        let hover = &light["brand"]["primary"]["hover"];
        assert_eq!((hover["$value"]["hex"].clone(), hover["$value"]["alpha"].clone()), (json!("#0000CC"), json!(0.50196)));
        assert_eq!(light["space"]["md"], json!({"$value": {"value": 16.0, "unit": "px"}, "$type": "dimension"}));
        assert_eq!(light["label"], json!({"$value": "Hi", "$type": "string"}));
        assert_eq!(light["dense"], json!({"$value": false, "$type": "boolean"}));
        assert_eq!(light["link"], json!({"$value": "{brand.primary.$root}", "$type": "color"}));
        assert_eq!(files[1]["tokens"]["brand"]["primary"]["$root"]["$value"]["hex"], json!("#FFFFFF"));
        // One mode only.
        let r = ex(&mut s, "variable.exportTokens", json!({"modes": ["Dark"]}));
        assert_eq!(r["files"].as_array().unwrap().len(), 1);
        assert!(s.execute("variable.exportTokens", &json!({"modes": ["Nope"]})).is_err());
        assert!(s.execute("variable.exportTokens", &json!({"format": "yaml"})).is_err());
    }

    #[test]
    fn dtcg_round_trips_through_import() {
        let mut a = themed();
        let first = ex(&mut a, "variable.exportTokens", json!({"format": "dtcg"}));
        let files: Vec<Value> = first["files"].as_array().unwrap().iter().map(|f| json!({"tokens": f["tokens"], "mode": f["mode"]})).collect();
        let mut b = Session::new();
        let r = ex(&mut b, "variable.importTokens", json!({"files": files, "collection": "Theme"}));
        assert_eq!((r["collections"].clone(), r["variables"].clone(), r["warnings"].clone()), (json!(1), json!(6), json!([])));
        let again = ex(&mut b, "variable.exportTokens", json!({"format": "dtcg"}));
        assert_eq!(first["files"], again["files"]);
        // Importing the same files again updates in place.
        let r = ex(&mut b, "variable.importTokens", json!({"files": files, "collection": "Theme"}));
        assert_eq!((r["variables"].clone(), r["updated"].clone()), (json!(0), json!(6)));
    }

    #[test]
    fn import_reads_older_shapes_and_warns_on_the_rest() {
        let mut s = Session::new();
        let tokens = json!({
            "color": {"$type": "color", "ink": {"$value": "#112233"}, "ref": {"$value": "{color.ink}"}},
            "size": {"s": {"$type": "dimension", "$value": "2rem"}},
            "font": {"body": {"$type": "fontFamily", "$value": ["Inter", "sans-serif"]}},
            "motion": {"fast": {"$type": "duration", "$value": {"value": 100, "unit": "ms"}}},
            "ghost": {"$type": "color", "$value": "{nowhere.at.all}"},
        });
        let r = ex(&mut s, "variable.importTokens", json!({"tokens": tokens}));
        assert_eq!(r["variables"], json!(4));
        assert_eq!(r["warnings"].as_array().unwrap().len(), 2, "{r}");
        let out = ex(&mut s, "variable.exportTokens", json!({}));
        let t = &out["files"][0]["tokens"];
        assert_eq!(t["color"]["ink"]["$value"]["hex"], json!("#112233"));
        assert_eq!(t["color"]["ref"]["$value"], json!("{color.ink}"));
        assert_eq!(t["size"]["s"]["$value"], json!({"value": 32.0, "unit": "px"}));
        assert_eq!(t["font"]["body"]["$value"], json!("Inter, sans-serif"));
        assert!(s.execute("variable.importTokens", &json!({"tokens": {}})).is_err());
        assert!(s.execute("variable.importTokens", &json!({"tokens": 5})).is_err());
    }

    #[test]
    fn css_and_tailwind_use_data_theme_and_var_references() {
        let mut s = themed();
        let css = ex(&mut s, "variable.exportTokens", json!({"format": "css"}))["text"].as_str().unwrap().to_string();
        assert!(css.contains(":root {") && css.contains("[data-theme=\"Dark\"] {"), "{css}");
        assert!(css.contains("#0000FF") && css.contains("#FFFFFF"), "{css}");
        let dark_only = ex(&mut s, "variable.exportTokens", json!({"format": "css", "modes": ["Dark"]}))["text"].as_str().unwrap().to_string();
        assert!(dark_only.contains(":root {") && !dark_only.contains("data-theme") && !dark_only.contains("#0000FF"), "{dark_only}");
        let tw = ex(&mut s, "variable.exportTokens", json!({"format": "tailwind"}));
        let colors = &tw["theme"]["extend"]["colors"];
        assert!(colors.as_object().unwrap().values().all(|v| v.as_str().unwrap().starts_with("var(--color-")), "{tw}");
        assert!(tw["text"].as_str().unwrap().contains("[data-theme=\"Dark\"]"));
    }
}
