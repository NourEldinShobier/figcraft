//! Token export beyond the plain variable files of `variable.exportTokens`: shared styles as DTCG
//! composite tokens (typography, shadow, gradient, border) with `com.figcraft` `$extensions`, a
//! Style Dictionary scaffold, a DTCG resolver document for modes, and native output for Swift,
//! Kotlin/Compose and Android XML.

use std::collections::{BTreeMap, BTreeSet};

use figcraft_color::{Color, GradientKind, PaintKind};
use figcraft_doc::text::{LetterSpacing, LineHeight, TextDecoration};
use figcraft_doc::{Collection, EffectKind, Style, StyleKind, Variable, VariableValue};
use serde_json::{Map, Value, json};

use super::tokens::{Ctx, TokFile, clean_segment, color_value, insert, unique_paths};
use super::variables_io::{dart_ident, dart_number, dart_unique};
use crate::{Result, bad};

const EXT: &str = "com.figcraft";

fn r5(x: f64) -> f64 {
    if x.is_finite() { (x * 1e5).round() / 1e5 } else { 0.0 }
}

fn dim(x: f64) -> Value {
    json!({"value": r5(x), "unit": "px"})
}

/// shortcut: a name-based weight guess for fonts without a `wght` variation; read the face's real
/// weight once the text crate exposes it.
fn weight_of(style: &str) -> u16 {
    let s = style.to_ascii_lowercase().replace(['-', '_', ' '], "");
    for (needle, weight) in [
        ("extralight", 200),
        ("ultralight", 200),
        ("extrabold", 800),
        ("ultrabold", 800),
        ("semibold", 600),
        ("demibold", 600),
        ("thin", 100),
        ("light", 300),
        ("medium", 500),
        ("bold", 700),
        ("black", 900),
        ("heavy", 900),
    ] {
        if s.contains(needle) {
            return weight;
        }
    }
    400
}

// ---------------------------------------------------------------------------------------------
// Styles as composite tokens

fn typography(cx: &Ctx, st: &Style, ts: &figcraft_doc::TextStyle) -> Value {
    let bound = |k: &str| st.bound.get(k).and_then(|id| cx.ref_any(id)).map(Value::String);
    let size = ts.font_size;
    let mut v = Map::new();
    v.insert("fontFamily".into(), bound("fontFamily").unwrap_or_else(|| json!(ts.font_family)));
    v.insert("fontSize".into(), bound("fontSize").unwrap_or_else(|| dim(size)));
    let weight = ts.variations.get("wght").map_or_else(|| f64::from(weight_of(&ts.font_style)), |w| *w);
    v.insert("fontWeight".into(), json!(r5(weight)));
    let spacing = match ts.letter_spacing {
        LetterSpacing::Pixels(p) => p,
        LetterSpacing::Percent(p) => p / 100.0 * size,
    };
    v.insert("letterSpacing".into(), bound("letterSpacing").unwrap_or_else(|| dim(spacing)));
    // DTCG line heights are multipliers of the font size; `auto` has none, so it is left out.
    let line = match ts.line_height {
        LineHeight::Pixels(p) if size > 0.0 => Some(p / size),
        LineHeight::Percent(p) => Some(p / 100.0),
        _ => None,
    };
    if let Some(b) = bound("lineHeight") {
        v.insert("lineHeight".into(), b);
    } else if let Some(l) = line {
        v.insert("lineHeight".into(), json!(r5(l)));
    }
    Value::Object(v)
}

fn text_extensions(ts: &figcraft_doc::TextStyle) -> Map<String, Value> {
    let mut e = Map::new();
    e.insert("fontStyle".into(), json!(ts.font_style));
    if ts.decoration != TextDecoration::None {
        e.insert("textDecoration".into(), serde_json::to_value(ts.decoration).unwrap_or(Value::Null));
    }
    if ts.case != figcraft_doc::TextCase::Original {
        e.insert("textCase".into(), serde_json::to_value(ts.case).unwrap_or(Value::Null));
    }
    if matches!(ts.line_height, LineHeight::Auto) {
        e.insert("lineHeight".into(), json!("auto"));
    }
    if !ts.variations.is_empty() {
        e.insert("variations".into(), json!(ts.variations));
    }
    e
}

fn shadows(cx: &Ctx, st: &Style, effects: &[figcraft_doc::Effect]) -> Option<(Value, Map<String, Value>)> {
    let bound = |i: usize, k: &str| st.bound.get(&format!("effects/{i}/{k}")).and_then(|id| cx.ref_any(id)).map(Value::String);
    let mut list = Vec::new();
    let mut ext = Map::new();
    let mut other = Vec::new();
    for (i, e) in effects.iter().enumerate() {
        if !e.visible {
            continue;
        }
        let (s, inset) = match &e.kind {
            EffectKind::DropShadow(s) => (s, false),
            EffectKind::InnerShadow(s) => (s, true),
            EffectKind::LayerBlur(b) => {
                other.push(json!({"type": "layerBlur", "radius": r5(b.radius)}));
                continue;
            }
            EffectKind::BackgroundBlur(b) => {
                other.push(json!({"type": "backgroundBlur", "radius": r5(b.radius)}));
                continue;
            }
            _ => continue,
        };
        let mut o = Map::new();
        o.insert("color".into(), bound(i, "color").unwrap_or_else(|| color_value(s.color)));
        o.insert("offsetX".into(), dim(s.offset.0));
        o.insert("offsetY".into(), dim(s.offset.1));
        o.insert("blur".into(), bound(i, "radius").unwrap_or_else(|| dim(s.radius)));
        o.insert("spread".into(), bound(i, "spread").unwrap_or_else(|| dim(s.spread)));
        if inset {
            o.insert("inset".into(), json!(true));
        }
        list.push(Value::Object(o));
    }
    if !other.is_empty() {
        ext.insert("blurs".into(), json!(other));
    }
    match list.len() {
        0 => None,
        1 => list.pop().map(|v| (v, ext)),
        _ => Some((Value::Array(list), ext)),
    }
}

/// The first visible paint of a paint style as `($type, $value, extensions)`.
fn paint_token(cx: &Ctx, st: &Style, paints: &[figcraft_color::Paint], border: bool) -> Option<(&'static str, Value, Map<String, Value>)> {
    let (i, p) = paints.iter().enumerate().find(|(_, p)| p.visible)?;
    let mut ext = Map::new();
    if paints.iter().filter(|p| p.visible).count() > 1 {
        ext.insert("layers".into(), json!(paints.iter().filter(|p| p.visible).count()));
    }
    match &p.kind {
        PaintKind::Solid { .. } => {
            let col = st.bound.get(&format!("fills/{i}/color")).and_then(|id| cx.ref_any(id)).map(Value::String);
            let col = col.unwrap_or_else(|| color_value(p.solid_color().unwrap_or(Color::new(0.0, 0.0, 0.0, 1.0))));
            if border {
                // A paint style has no width: 1px is a placeholder the extension flags.
                ext.insert("widthAssumed".into(), json!(true));
                Some(("border", json!({"color": col, "width": dim(1.0), "style": "solid"}), ext))
            } else {
                Some(("color", col, ext))
            }
        }
        PaintKind::Gradient(g) => {
            let stops: Vec<Value> = g
                .stops
                .iter()
                .map(|s| json!({"color": color_value(s.color.with_alpha(s.color.a * p.opacity)), "position": r5(f64::from(s.position))}))
                .collect();
            let kind = match g.kind {
                GradientKind::Linear => "linear",
                GradientKind::Radial => "radial",
                GradientKind::Angular => "angular",
                GradientKind::Diamond => "diamond",
            };
            ext.insert("kind".into(), json!(kind));
            ext.insert("from".into(), json!([r5(g.from.0), r5(g.from.1)]));
            ext.insert("to".into(), json!([r5(g.to.0), r5(g.to.1)]));
            ext.insert("width".into(), json!([r5(g.width.0), r5(g.width.1)]));
            Some(("gradient", Value::Array(stops), ext))
        }
        _ => None,
    }
}

/// Every shared style as a DTCG token under `text`, `effect` and `paint` groups; styles with no
/// DTCG shape (grids, image paints, blur-only effects) come back as notes.
pub(super) fn styles(cx: &Ctx) -> (Map<String, Value>, Vec<String>) {
    let mut skipped = Vec::new();
    let mut made: Vec<(String, String, Value)> = Vec::new(); // (group, style name, token)
    for st in &cx.doc.styles {
        let (group, got) = match &st.kind {
            StyleKind::Text { style } => ("text", Some(("typography", typography(cx, st, style), text_extensions(style)))),
            StyleKind::Effect { effects } => ("effect", shadows(cx, st, effects).map(|(v, e)| ("shadow", v, e))),
            StyleKind::Paint { paints } => {
                let first = st.name.split('/').next().unwrap_or("").trim().to_ascii_lowercase();
                let border = matches!(first.as_str(), "border" | "borders" | "stroke" | "strokes");
                ("paint", paint_token(cx, st, paints, border))
            }
            StyleKind::Grid { .. } => ("grid", None),
        };
        let Some((ty, value, ext)) = got else {
            skipped.push(format!("{} style `{}`: no DTCG type for it", group, st.name));
            continue;
        };
        let mut t = Map::new();
        t.insert("$type".into(), json!(ty));
        t.insert("$value".into(), value);
        if !st.description.is_empty() {
            t.insert("$description".into(), json!(st.description));
        }
        if !ext.is_empty() {
            t.insert("$extensions".into(), json!({ EXT: ext }));
        }
        made.push((group.to_string(), st.name.clone(), Value::Object(t)));
    }
    let mut tree = Map::new();
    for group in ["text", "effect", "paint"] {
        let items: Vec<&(String, String, Value)> = made.iter().filter(|m| m.0 == group).collect();
        let names: Vec<&str> = items.iter().map(|m| m.1.as_str()).collect();
        for (m, path) in items.iter().zip(unique_paths(&names)) {
            let mut full = vec![group.to_string()];
            full.extend(path);
            insert(&mut tree, &full, m.2.clone());
        }
    }
    (tree, skipped)
}

// ---------------------------------------------------------------------------------------------
// Style Dictionary and resolver

fn slug(s: &str) -> String {
    clean_segment(s).chars().map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_') { c } else { '_' }).collect()
}

/// A Style Dictionary (v4) scaffold: `style-dictionary.<collection>.<mode>.config.json` per token
/// file, building css variables (`:root` for the default mode, `[data-theme="<mode>"]` for the
/// others) from `tokens/<file>`. The token files use the strings Style Dictionary reads (hex
/// colours, `16px`). Run: `npx style-dictionary build --config <config>`.
pub(super) fn style_dictionary(files: &[TokFile], styles: &Map<String, Value>) -> Vec<Value> {
    let mut out = Vec::new();
    for f in files {
        let config = json!({
            "source": [format!("tokens/{}", f.name)],
            "platforms": {"css": {
                "transformGroup": "css",
                "buildPath": "build/css/",
                "files": [{
                    "destination": format!("{}.{}.css", slug(&f.collection), slug(&f.mode)),
                    "format": "css/variables",
                    "options": {"selector": if f.default { ":root".to_string() } else { format!("[data-theme=\"{}\"]", f.mode.replace(['"', '\\'], "")) }},
                }],
            }},
        });
        out.push(json!({"name": format!("style-dictionary.{}.{}.config.json", slug(&f.collection), slug(&f.mode)),
                        "text": serde_json::to_string_pretty(&config).unwrap_or_default()}));
        out.push(json!({"name": format!("tokens/{}", f.name), "collection": f.collection, "mode": f.mode, "tokens": f.tokens}));
    }
    if !styles.is_empty() {
        let config = json!({
            "source": ["tokens/Styles.tokens.json"],
            "platforms": {"json": {"transformGroup": "js", "buildPath": "build/json/",
                                   "files": [{"destination": "styles.json", "format": "json/nested"}]}},
        });
        out.push(json!({"name": "style-dictionary.styles.config.json", "text": serde_json::to_string_pretty(&config).unwrap_or_default()}));
        out.push(json!({"name": "tokens/Styles.tokens.json", "tokens": styles}));
    }
    out
}

/// A DTCG resolver document (2025.10 draft: `sets`, `modifiers`, `resolutionOrder`) over the token
/// files: a collection with several modes is a modifier whose contexts are its modes, a
/// single-mode collection a set, and styles a set.
pub(super) fn resolver(files: &[TokFile], has_styles: bool) -> Value {
    let mut sets = Map::new();
    let mut modifiers = Map::new();
    let mut order = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for f in files {
        if seen.contains(&f.collection.as_str()) {
            continue;
        }
        seen.push(&f.collection);
        let mine: Vec<&TokFile> = files.iter().filter(|x| x.collection == f.collection).collect();
        if mine.len() == 1 {
            sets.insert(f.collection.clone(), json!({"sources": [{"$ref": f.name}]}));
            order.push(json!({"$ref": format!("#/sets/{}", f.collection)}));
        } else {
            let contexts: Map<String, Value> = mine.iter().map(|x| (x.mode.clone(), json!([{"$ref": x.name}]))).collect();
            let default = mine.iter().find(|x| x.default).map_or(f.mode.clone(), |x| x.mode.clone());
            modifiers.insert(f.collection.clone(), json!({"contexts": contexts, "default": default}));
            order.push(json!({"$ref": format!("#/modifiers/{}", f.collection)}));
        }
    }
    if has_styles {
        sets.insert("Styles".into(), json!({"sources": [{"$ref": "Styles.tokens.json"}]}));
        order.push(json!({"$ref": "#/sets/Styles"}));
    }
    json!({"version": "2025.10", "name": "FigCraft tokens", "sets": sets, "modifiers": modifiers, "resolutionOrder": order})
}

// ---------------------------------------------------------------------------------------------
// Native output

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Lang {
    Swift,
    Kotlin,
}

enum Val {
    Color(Color),
    Length(f64),
    Plain(f64),
    Text(String),
    Flag(bool),
}

const SWIFT_EXTENSION_TYPES: [&str; 7] = ["Color", "CGFloat", "String", "Bool", "Double", "Int", "Float"];
const SWIFT_EXTRA: [&str; 12] = ["func", "let", "protocol", "struct", "internal", "self", "Self", "Type", "Any", "init", "subscript", "extension"];
const KOTLIN_EXTRA: [&str; 12] = ["fun", "val", "var", "when", "object", "is", "in", "typealias", "interface", "package", "as", "this"];

fn ident(name: &str, pascal: bool, lang: Lang) -> String {
    let mut out = dart_ident(name, pascal);
    let extra: &[&str] = if lang == Lang::Swift { &SWIFT_EXTRA } else { &KOTLIN_EXTRA };
    if extra.contains(&out.as_str()) && !out.ends_with('_') {
        out.push('_');
    }
    out
}

fn is_ident(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The (scope, member) a variable's code syntax for `platform` names: `Color.brand` is scope
/// `Color`, `brand` member, a bare `brand` has no scope. Anything else (calls, deeper paths) is
/// not a declaration this can write, so the variable gets a generated name.
fn syntax_name(v: &Variable, platform: &str) -> Option<(String, String)> {
    let s = v.code_syntax.get(platform)?.trim();
    match s.split_once('.') {
        Some((a, b)) if is_ident(a) && is_ident(b) => Some((a.to_string(), b.to_string())),
        None if is_ident(s) => Some((String::new(), s.to_string())),
        _ => None,
    }
}

/// An Android resource name: lower snake case, never empty or starting with a digit.
fn res_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('_') && !out.is_empty() {
            out.push('_');
        }
    }
    let mut out = out.trim_end_matches('_').to_string();
    if out.is_empty() || out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert_str(0, "v_");
    }
    out
}

fn res_syntax(v: &Variable, kind: &str) -> Option<String> {
    let s = v.code_syntax.get("ANDROID")?.trim();
    let n = s.strip_prefix(&format!("@{kind}/")).or_else(|| s.strip_prefix(&format!("R.{kind}.")))?;
    (is_ident(n)).then(|| n.to_string())
}

fn swift_color(c: Color) -> String {
    let r = |x: f32| format!("{}", r5(f64::from(x)));
    format!("Color(.sRGB, red: {}, green: {}, blue: {}, opacity: {})", r(c.r), r(c.g), r(c.b), r(c.a))
}

fn android_color(c: Color) -> String {
    let [r, g, b, a] = c.to_rgba8();
    format!("#{a:02X}{r:02X}{g:02X}{b:02X}")
}

fn value_in(cx: &Ctx, v: &Variable, c: &Collection, mode: &str) -> Option<Val> {
    let modes = BTreeMap::from([(c.id.clone(), mode.to_string())]);
    Some(match cx.doc.variables.resolve(&v.id, &modes)? {
        VariableValue::Color(col) => Val::Color(col),
        VariableValue::Number(x) if x.is_finite() => {
            if cx.is_unitless(v) {
                Val::Plain(x)
            } else {
                Val::Length(x)
            }
        }
        VariableValue::String(t) => Val::Text(t),
        VariableValue::Boolean(b) => Val::Flag(b),
        _ => return None,
    })
}

type Scope = (String, Vec<String>); // (scope header name, member lines)

fn scope_lines<'a>(scopes: &'a mut Vec<Scope>, name: &str) -> &'a mut Vec<String> {
    let i = match scopes.iter().position(|s| s.0 == name) {
        Some(i) => i,
        None => {
            scopes.push((name.to_string(), Vec::new()));
            scopes.len() - 1
        }
    };
    &mut scopes[i].1
}

fn member_line(lang: Lang, name: &str, v: &Val) -> String {
    match (lang, v) {
        (Lang::Swift, Val::Color(c)) => format!("static let {name}: Color = {}", swift_color(*c)),
        (Lang::Swift, Val::Length(x) | Val::Plain(x)) => format!("static let {name}: CGFloat = {}", dart_number(*x)),
        (Lang::Swift, Val::Text(t)) => format!("static let {name}: String = \"{}\"", swift_string(t)),
        (Lang::Swift, Val::Flag(b)) => format!("static let {name}: Bool = {b}"),
        (_, Val::Color(c)) => format!("val {name} = {}", crate::codegen::kotlin_color(*c)),
        (_, Val::Length(x)) => format!("val {name} = {}.dp", dart_number(*x)),
        (_, Val::Plain(x)) => format!("const val {name} = {}f", dart_number(*x)),
        (_, Val::Text(t)) => format!("const val {name} = \"{}\"", swift_string(t).replace('$', "\\$")),
        (_, Val::Flag(b)) => format!("const val {name} = {b}"),
    }
}

/// `text` inside a Swift/Kotlin double-quoted string.
fn swift_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// Swift or Kotlin source. The default mode of each collection is declared under the names the
/// variables' code syntax gives (`Color.brand` becomes `extension Color { static let brand }`,
/// Kotlin an `object Color`), else in a type named after the collection; every other mode gets a
/// type of its own named after collection and mode. Aliases and colour calculations are resolved.
pub(super) fn native_source(cx: &Ctx, colls: &[&Collection], only: &Option<Vec<String>>, lang: Lang) -> String {
    let platform = if lang == Lang::Swift { "iOS" } else { "ANDROID" };
    let mut scopes: Vec<Scope> = Vec::new();
    let mut taken: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut types = BTreeSet::new();
    for c in colls {
        let base = ident(&c.name, true, lang);
        for m in c.modes.iter().filter(|m| super::tokens::mode_wanted(only, m)) {
            let is_default = c.default_mode() == Some(&m.id);
            let own = dart_unique(if is_default { base.clone() } else { format!("{base}{}", ident(&m.name, true, lang)) }, &mut types);
            for v in cx.doc.variables.variables.iter().filter(|v| v.collection == c.id) {
                let Some(val) = value_in(cx, v, c, &m.id) else { continue };
                let (scope, member) = match syntax_name(v, platform).filter(|_| is_default) {
                    Some(sm) => sm,
                    None => (own.clone(), ident(&v.name, false, lang)),
                };
                let member = dart_unique(member, taken.entry(scope.clone()).or_default());
                scope_lines(&mut scopes, &scope).push(member_line(lang, &member, &val));
            }
        }
    }
    let mut out = if lang == Lang::Swift {
        String::from("// Variables exported from FigCraft.\n\nimport SwiftUI\n")
    } else {
        String::from("// Variables exported from FigCraft.\n\nimport androidx.compose.ui.graphics.Color\nimport androidx.compose.ui.unit.dp\n")
    };
    for (name, lines) in &scopes {
        if name.is_empty() {
            out.push('\n');
            for l in lines {
                // Swift has no top-level `static`.
                out.push_str(&if lang == Lang::Swift { l.replacen("static let", "let", 1) } else { l.clone() });
                out.push('\n');
            }
            continue;
        }
        let head = match lang {
            Lang::Swift if SWIFT_EXTENSION_TYPES.contains(&name.as_str()) => format!("extension {name}"),
            Lang::Swift => format!("enum {name}"),
            _ => format!("object {name}"),
        };
        out.push_str(&format!("\n{head} {{\n"));
        for l in lines {
            out.push_str(&format!("    {l}\n"));
        }
        out.push_str("}\n");
    }
    out
}

/// `values/colors.xml`, `values/dimens.xml` for each collection's default mode and
/// `values-night/colors.xml` from modes named Dark. Strings, booleans and other modes have no
/// resource file here and come back in the notes.
pub(super) fn android_files(cx: &Ctx, colls: &[&Collection], only: &Option<Vec<String>>) -> (Vec<Value>, Vec<String>) {
    let (mut colors, mut dimens, mut night) = (Vec::new(), Vec::new(), Vec::new());
    let mut notes = Vec::new();
    let mut taken: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for c in colls {
        for v in cx.doc.variables.variables.iter().filter(|v| v.collection == c.id) {
            let default = c.default_mode().and_then(|m| value_in(cx, v, c, m));
            let kind = match default {
                Some(Val::Color(_)) => "color",
                Some(Val::Length(_) | Val::Plain(_)) => "dimen",
                Some(_) => {
                    notes.push(format!("`{}`: strings and booleans have no values/*.xml file here", v.name));
                    continue;
                }
                None => continue,
            };
            let base = res_syntax(v, kind).unwrap_or_else(|| res_name(&v.name));
            let name = dart_unique(base, taken.entry(kind).or_default());
            for m in c.modes.iter().filter(|m| super::tokens::mode_wanted(only, m)) {
                let Some(val) = value_in(cx, v, c, &m.id) else { continue };
                let is_default = c.default_mode() == Some(&m.id);
                let dark = !is_default && m.name.trim().eq_ignore_ascii_case("dark");
                if !is_default && !dark {
                    if !notes.iter().any(|n| n.contains(&format!("mode `{}`", m.name))) {
                        notes.push(format!("mode `{}` of `{}`: only the default mode and a mode named Dark have a values folder", m.name, c.name));
                    }
                    continue;
                }
                let line = match val {
                    Val::Color(col) => format!("    <color name=\"{name}\">{}</color>", android_color(col)),
                    Val::Length(x) => format!("    <dimen name=\"{name}\">{}dp</dimen>", dart_number(x)),
                    Val::Plain(x) => format!("    <item name=\"{name}\" format=\"float\" type=\"dimen\">{}</item>", dart_number(x)),
                    _ => continue,
                };
                match (kind, dark) {
                    ("color", false) => colors.push(line),
                    ("color", true) => night.push(line),
                    (_, false) => dimens.push(line),
                    _ => {}
                }
            }
        }
    }
    let file = |name: &str, lines: &[String]| json!({"name": name, "text": format!("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<resources>\n{}\n</resources>\n", lines.join("\n"))});
    let mut files = Vec::new();
    for (name, lines) in [("values/colors.xml", &colors), ("values/dimens.xml", &dimens), ("values-night/colors.xml", &night)] {
        if !lines.is_empty() {
            files.push(file(name, lines));
        }
    }
    (files, notes)
}

pub(super) fn require_files(cmd: &str, files: &[Value]) -> Result<()> {
    if files.is_empty() { Err(bad(cmd, "nothing to export: no variable with a colour or number value")) } else { Ok(()) }
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
        mk(&mut s, "space/md", "number", json!({"Light": 16, "Dark": 12}));
        mk(&mut s, "label", "string", json!({"Light": "Hi", "Dark": "Hallo"}));
        s
    }

    fn files(r: &Value) -> Vec<String> {
        r["files"].as_array().unwrap().iter().map(|f| f["name"].as_str().unwrap().to_string()).collect()
    }

    fn with_styles() -> Session {
        let mut s = themed();
        ex(
            &mut s,
            "variable.update",
            json!({"variable": "space/md", "codeSyntax": {"iOS": "Spacing.md", "ANDROID": "@dimen/space_md"}, "scopes": ["GAP"]}),
        );
        ex(
            &mut s,
            "style.create",
            json!({"type": "text", "name": "Heading/H1", "value": {"fontFamily": "Inter", "fontSize": 32, "fontStyle": "Bold"}}),
        );
        let r = ex(&mut s, "shape.rectangle", json!({"x": 0, "y": 0, "width": 40, "height": 40}));
        ex(&mut s, "effect.add", json!({"ids": [r["id"]], "type": "dropShadow"}));
        ex(&mut s, "style.create", json!({"type": "effect", "name": "Elevation/Low", "from": r["id"]}));
        ex(&mut s, "style.create", json!({"type": "paint", "name": "Brand/Blue", "value": "#3366FF"}));
        ex(&mut s, "style.create", json!({"type": "paint", "name": "Border/Default", "value": "#112233"}));
        ex(
            &mut s,
            "style.create",
            json!({"type": "paint", "name": "Sky", "value": [{"type": "gradient", "kind": "linear",
                    "stops": [{"position": 0, "color": "#FF0000"}, {"position": 1, "color": "#0000FF"}], "from": [0, 0.5], "to": [1, 0.5]}]}),
        );
        s
    }

    #[test]
    fn styles_become_composite_tokens_with_extensions() {
        let mut s = with_styles();
        let r = ex(&mut s, "variable.exportTokens", json!({"format": "dtcg-styles"}));
        let t = &r["files"][0]["tokens"];
        let h1 = &t["text"]["Heading"]["H1"];
        assert_eq!(h1["$type"], json!("typography"));
        assert_eq!(h1["$value"]["fontFamily"], json!("Inter"));
        assert_eq!(h1["$value"]["fontSize"], json!({"value": 32.0, "unit": "px"}));
        assert_eq!(h1["$value"]["fontWeight"], json!(700.0));
        assert_eq!(h1["$extensions"]["com.figcraft"]["fontStyle"], json!("Bold"));
        let shadow = &t["effect"]["Elevation"]["Low"];
        assert_eq!(shadow["$type"], json!("shadow"));
        assert_eq!(shadow["$value"]["offsetY"], json!({"value": 4.0, "unit": "px"}));
        assert_eq!(shadow["$value"]["blur"], json!({"value": 4.0, "unit": "px"}));
        assert_eq!(t["paint"]["Brand"]["Blue"]["$type"], json!("color"));
        assert_eq!(t["paint"]["Brand"]["Blue"]["$value"]["hex"], json!("#3366FF"));
        let border = &t["paint"]["Border"]["Default"];
        assert_eq!((border["$type"].clone(), border["$value"]["style"].clone()), (json!("border"), json!("solid")));
        assert_eq!(border["$extensions"]["com.figcraft"]["widthAssumed"], json!(true));
        let sky = &t["paint"]["Sky"];
        assert_eq!(sky["$type"], json!("gradient"));
        assert_eq!(sky["$value"][1]["position"], json!(1.0));
        assert_eq!(sky["$value"][1]["color"]["hex"], json!("#0000FF"));
        assert_eq!(sky["$extensions"]["com.figcraft"]["kind"], json!("linear"));
        // No styles: an error, not an empty file.
        assert!(Session::new().execute("variable.exportTokens", &json!({"format": "dtcg-styles"})).is_err());
    }

    #[test]
    fn a_bound_style_field_is_an_alias() {
        let mut s = with_styles();
        ex(&mut s, "variable.create", json!({"collection": "Theme", "name": "size/h1", "type": "number", "values": {"Light": 40, "Dark": 40}}));
        let id = ex(&mut s, "style.list", json!({"type": "text"}))[0]["id"].clone();
        ex(&mut s, "style.bind", json!({"style": id, "field": "fontSize", "variable": "size/h1"}));
        let r = ex(&mut s, "variable.exportTokens", json!({"format": "dtcg-styles"}));
        assert_eq!(r["files"][0]["tokens"]["text"]["Heading"]["H1"]["$value"]["fontSize"], json!("{Theme.size.h1}"));
    }

    #[test]
    fn variable_extensions_round_trip_through_import() {
        let mut a = with_styles();
        let first = ex(&mut a, "variable.exportTokens", json!({"format": "dtcg"}));
        let md = &first["files"][0]["tokens"]["space"]["md"];
        assert_eq!(md["$extensions"]["com.figcraft"], json!({"scopes": ["GAP"], "codeSyntax": {"ANDROID": "@dimen/space_md", "iOS": "Spacing.md"}}));
        assert!(first["files"][0]["tokens"]["brand"]["primary"].get("$extensions").is_none());
        let fs: Vec<Value> = first["files"].as_array().unwrap().iter().map(|f| json!({"tokens": f["tokens"], "mode": f["mode"]})).collect();
        let mut b = Session::new();
        ex(&mut b, "variable.importTokens", json!({"files": fs, "collection": "Theme"}));
        assert_eq!(first["files"], ex(&mut b, "variable.exportTokens", json!({}))["files"]);
    }

    #[test]
    fn style_dictionary_has_a_config_and_string_tokens_per_mode() {
        let mut s = with_styles();
        let r = ex(&mut s, "variable.exportTokens", json!({"format": "style-dictionary"}));
        assert_eq!(
            files(&r),
            [
                "style-dictionary.Theme.Light.config.json",
                "tokens/Theme.Light.tokens.json",
                "style-dictionary.Theme.Dark.config.json",
                "tokens/Theme.Dark.tokens.json",
                "style-dictionary.styles.config.json",
                "tokens/Styles.tokens.json"
            ]
        );
        let list = r["files"].as_array().unwrap();
        let light: Value = serde_json::from_str(list[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(light["source"], json!(["tokens/Theme.Light.tokens.json"]));
        assert_eq!(light["platforms"]["css"]["files"][0]["options"]["selector"], json!(":root"));
        assert_eq!(light["platforms"]["css"]["files"][0]["format"], json!("css/variables"));
        let dark: Value = serde_json::from_str(list[2]["text"].as_str().unwrap()).unwrap();
        assert_eq!(dark["platforms"]["css"]["files"][0]["options"]["selector"], json!("[data-theme=\"Dark\"]"));
        assert_eq!(list[1]["tokens"]["brand"]["primary"]["$value"], json!("#0000FF"));
        assert_eq!(list[1]["tokens"]["space"]["md"]["$value"], json!("16px"));
        // The scaffold's token files read back as the same variables.
        let fs: Vec<Value> = [1, 3].iter().map(|i| json!({"tokens": list[*i]["tokens"], "mode": list[*i]["mode"]})).collect();
        let mut b = Session::new();
        let imp = ex(&mut b, "variable.importTokens", json!({"files": fs, "collection": "Theme"}));
        assert_eq!(imp["variables"], json!(3), "{imp}");
        let back = ex(&mut b, "variable.exportTokens", json!({"format": "dtcg"}));
        let orig = ex(&mut s, "variable.exportTokens", json!({"format": "dtcg"}));
        assert_eq!(back["files"][0]["tokens"]["brand"], orig["files"][0]["tokens"]["brand"]);
        assert_eq!(back["files"][1]["tokens"]["space"]["md"]["$value"], orig["files"][1]["tokens"]["space"]["md"]["$value"]);
    }

    #[test]
    fn resolver_lists_modes_as_contexts() {
        let mut s = with_styles();
        let r = ex(&mut s, "variable.exportTokens", json!({"format": "resolver"}));
        assert_eq!(files(&r), ["tokens.resolver.json", "Theme.Light.tokens.json", "Theme.Dark.tokens.json", "Styles.tokens.json"]);
        let doc: Value = serde_json::from_str(r["files"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(
            doc,
            json!({"version": "2025.10", "name": "FigCraft tokens",
                   "sets": {"Styles": {"sources": [{"$ref": "Styles.tokens.json"}]}},
                   "modifiers": {"Theme": {"contexts": {"Light": [{"$ref": "Theme.Light.tokens.json"}], "Dark": [{"$ref": "Theme.Dark.tokens.json"}]}, "default": "Light"}},
                   "resolutionOrder": [{"$ref": "#/modifiers/Theme"}, {"$ref": "#/sets/Styles"}]})
        );
    }

    #[test]
    fn swift_honours_ios_code_syntax_and_names_other_modes() {
        let mut s = with_styles();
        let text = ex(&mut s, "variable.exportTokens", json!({"format": "swift"}))["text"].as_str().unwrap().to_string();
        assert_eq!(
            text,
            "// Variables exported from FigCraft.\n\nimport SwiftUI\n\nenum Theme {\n    static let brandPrimary: Color = Color(.sRGB, red: 0, green: 0, blue: 1, opacity: 1)\n    static let label: String = \"Hi\"\n}\n\nenum Spacing {\n    static let md: CGFloat = 16\n}\n\nenum ThemeDark {\n    static let brandPrimary: Color = Color(.sRGB, red: 1, green: 1, blue: 1, opacity: 1)\n    static let spaceMd: CGFloat = 12\n    static let label: String = \"Hallo\"\n}\n"
        );
        let text = ex(&mut s, "variable.exportTokens", json!({"format": "swift", "modes": ["Light"]}))["text"].as_str().unwrap().to_string();
        assert!(!text.contains("ThemeDark"));
    }

    #[test]
    fn kotlin_writes_colors_dp_and_consts() {
        let mut s = with_styles();
        let text = ex(&mut s, "variable.exportTokens", json!({"format": "kotlin", "modes": ["Light"]}))["text"].as_str().unwrap().to_string();
        assert_eq!(
            text,
            "// Variables exported from FigCraft.\n\nimport androidx.compose.ui.graphics.Color\nimport androidx.compose.ui.unit.dp\n\nobject Theme {\n    val brandPrimary = Color(0xFF0000FF)\n    val spaceMd = 16.dp\n    const val label = \"Hi\"\n}\n"
        );
        // A bare `Tokens.name` Android syntax names the declaration.
        ex(&mut s, "variable.update", json!({"variable": "space/md", "codeSyntax": {"ANDROID": "Tokens.gap"}}));
        let text = ex(&mut s, "variable.exportTokens", json!({"format": "kotlin", "modes": ["Light"]}))["text"].as_str().unwrap().to_string();
        assert!(text.contains("object Tokens {\n    val gap = 16.dp\n}"), "{text}");
    }

    #[test]
    fn android_xml_has_colors_dimens_and_night() {
        let mut s = with_styles();
        let r = ex(&mut s, "variable.exportTokens", json!({"format": "android"}));
        assert_eq!(files(&r), ["values/colors.xml", "values/dimens.xml", "values-night/colors.xml"]);
        let list = r["files"].as_array().unwrap();
        let head = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<resources>\n";
        assert_eq!(list[0]["text"], json!(format!("{head}    <color name=\"brand_primary\">#FF0000FF</color>\n</resources>\n")));
        assert_eq!(list[1]["text"], json!(format!("{head}    <dimen name=\"space_md\">16dp</dimen>\n</resources>\n")));
        assert_eq!(list[2]["text"], json!(format!("{head}    <color name=\"brand_primary\">#FFFFFFFF</color>\n</resources>\n")));
        assert_eq!(r["skipped"].as_array().unwrap().len(), 1, "{r}");
    }

    #[test]
    fn tailwind_text_defines_the_names_the_theme_reads() {
        let mut s = themed();
        let tw = ex(&mut s, "variable.exportTokens", json!({"format": "tailwind"}));
        let text = tw["text"].as_str().unwrap();
        assert!(text.contains("@theme static"), "{text}");
        let colors = tw["theme"]["extend"]["colors"].as_object().unwrap();
        assert!(!colors.is_empty());
        for v in colors.values() {
            let name = v.as_str().unwrap().trim_start_matches("var(").trim_end_matches(')');
            assert!(text.contains(&format!("  {name}: ")), "{name} is not defined in\n{text}");
        }
    }

    #[test]
    fn junk_and_writing_files() {
        let mut s = with_styles();
        for f in ["swift", "kotlin", "android", "resolver", "style-dictionary", "dtcg-styles"] {
            assert!(s.execute("variable.exportTokens", &json!({"format": f, "modes": ["Nope"], "collection": "Nope"})).is_err(), "{f}");
            assert!(s.execute("variable.exportTokens", &json!({"format": f, "modes": 5})).is_err(), "{f}");
        }
        let dir = std::env::temp_dir().join(format!("figcraft-tokens-{}", std::process::id()));
        let r = ex(&mut s, "variable.exportTokens", json!({"format": "android", "path": dir.to_string_lossy()}));
        assert_eq!(r["paths"].as_array().unwrap().len(), 3);
        assert!(std::fs::read_to_string(dir.join("values-night/colors.xml")).unwrap().contains("#FFFFFFFF"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
