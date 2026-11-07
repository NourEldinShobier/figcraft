//! The cascade for `import.html`: which declarations apply to an element (user-agent defaults,
//! style sheets by specificity, the `style` attribute, `!important`), custom properties and
//! `var()`, shorthand expansion into the longhands the importer reads, and CSS value parsers.

use std::collections::BTreeMap;
use std::rc::Rc;

use figcraft_color::Color;

use super::Warn;
use super::css::{Decl, Index, parse_decls, split_top};
use super::dom::Dom;

pub type Props = BTreeMap<String, String>;
/// Custom properties (`--name`): inherited, so shared between an element and its children.
pub type Vars = Rc<BTreeMap<String, String>>;

/// Longhands the importer reads. Others are reported (unless they only affect interaction or
/// rendering details that have no counterpart in a static design).
const KNOWN: &[&str] = &[
    "display",
    "flex-direction",
    "flex-wrap",
    "flex-grow",
    "flex-basis",
    "justify-content",
    "align-items",
    "row-gap",
    "column-gap",
    "width",
    "height",
    "min-width",
    "max-width",
    "min-height",
    "max-height",
    "box-sizing",
    "background-color",
    "background-image",
    "color",
    "font-family",
    "font-size",
    "font-weight",
    "font-style",
    "line-height",
    "text-align",
    "text-transform",
    "text-decoration",
    "letter-spacing",
    "white-space",
    "opacity",
    "box-shadow",
    "overflow",
    "visibility",
    "position",
];
const SIDES: [&str; 4] = ["top", "right", "bottom", "left"];
const CORNERS: [&str; 4] = ["top-left", "top-right", "bottom-right", "bottom-left"];

fn is_known(name: &str) -> bool {
    KNOWN.contains(&name)
        || name.starts_with("margin-") && name.len() > 7
        || name.starts_with("padding-")
        || name.starts_with("border-")
            && (name.ends_with("-width") || name.ends_with("-style") || name.ends_with("-color") || name.ends_with("-radius"))
}

/// Properties that change nothing in a static design: skipped without a warning.
fn is_silent(name: &str) -> bool {
    const PREFIXES: &[&str] =
        &["transition", "animation", "outline", "list-style", "scroll", "-webkit-", "-moz-", "-ms-", "font-variant", "font-feature"];
    const NAMES: &[&str] = &[
        "cursor",
        "will-change",
        "user-select",
        "pointer-events",
        "appearance",
        "tab-size",
        "z-index",
        "flex-shrink",
        "vertical-align",
        "text-overflow",
        "word-break",
        "overflow-wrap",
        "word-wrap",
        "content",
        "resize",
        "caret-color",
        "accent-color",
        "touch-action",
        "isolation",
        "top",
        "left",
        "right",
        "bottom",
        "clear",
        "table-layout",
        "border-collapse",
        "border-spacing",
        "color-scheme",
        "font-display",
        "hyphens",
        "text-rendering",
        "font-smooth",
        "orphans",
        "widows",
        "text-size-adjust",
        "print-color-adjust",
        "quotes",
        "speak",
        "empty-cells",
        "background-repeat",
        "background-position",
        "background-size",
        "background-attachment",
        "background-origin",
        "background-clip",
        "text-underline-offset",
        "text-decoration-thickness",
        "text-decoration-color",
        "text-decoration-style",
        "text-decoration-line",
        "text-wrap",
        "contain",
        "content-visibility",
        "mask",
        "inset",
    ];
    NAMES.contains(&name) || PREFIXES.iter().any(|p| name.starts_with(p))
}

/// Default styles of HTML elements, as a style sheet's declarations.
fn ua_css(tag: &str) -> &'static str {
    match tag {
        "body" => "display:block;margin:8px",
        "html" | "div" | "section" | "article" | "aside" | "nav" | "header" | "footer" | "main" | "figure" | "figcaption" | "form" | "fieldset"
        | "details" | "summary" | "li" | "dd" | "dt" | "address" | "hgroup" | "menu" | "tbody" | "thead" | "tfoot" | "caption" | "center" => {
            "display:block"
        }
        "p" | "dl" => "display:block;margin-top:1em;margin-bottom:1em",
        "pre" => "display:block;margin-top:1em;margin-bottom:1em;font-family:monospace;white-space:pre",
        "blockquote" => "display:block;margin:1em 40px",
        "ul" | "ol" => "display:block;margin-top:1em;margin-bottom:1em;padding-left:40px",
        "table" => "display:block",
        "hr" => "display:block;margin-top:.5em;margin-bottom:.5em",
        "tr" => "display:flex",
        "td" => "display:block;flex-grow:1",
        "th" => "display:block;flex-grow:1;font-weight:bold;text-align:center",
        "h1" => "display:block;font-size:2em;font-weight:bold;margin-top:.67em;margin-bottom:.67em",
        "h2" => "display:block;font-size:1.5em;font-weight:bold;margin-top:.83em;margin-bottom:.83em",
        "h3" => "display:block;font-size:1.17em;font-weight:bold;margin-top:1em;margin-bottom:1em",
        "h4" => "display:block;font-weight:bold;margin-top:1.33em;margin-bottom:1.33em",
        "h5" => "display:block;font-size:.83em;font-weight:bold;margin-top:1.67em;margin-bottom:1.67em",
        "h6" => "display:block;font-size:.67em;font-weight:bold;margin-top:2.33em;margin-bottom:2.33em",
        "b" | "strong" => "font-weight:bold",
        "i" | "em" | "cite" | "dfn" | "var" => "font-style:italic",
        "u" | "ins" => "text-decoration:underline",
        "s" | "del" | "strike" => "text-decoration:line-through",
        "a" => "color:#0000EE;text-decoration:underline",
        "small" | "sub" | "sup" => "font-size:.83em",
        "code" | "kbd" | "samp" | "tt" => "font-family:monospace",
        "button" => "display:inline-block;text-align:center",
        _ => "",
    }
}

/// The cascaded longhands of element `el` and the custom properties in force there.
pub fn compute(dom: &Dom, el: usize, index: &Index, parent_vars: &Vars, warn: &mut Warn) -> (Props, Vars) {
    let tag = dom.tag(el).unwrap_or("");
    let ua = parse_decls(ua_css(tag));
    let inline = parse_decls(dom.attr(el, "style").unwrap_or(""));
    let rules = index.matching(dom, el);
    // (important, origin, specificity, order, declaration)
    let mut all: Vec<(u8, u8, u32, usize, &Decl)> = Vec::new();
    all.extend(ua.iter().enumerate().map(|(i, d)| (u8::from(d.important), 0, 0, i, d)));
    for r in &rules {
        all.extend(r.decls.iter().map(|d| (u8::from(d.important), 1, r.specificity(), r.order, d)));
    }
    all.extend(inline.iter().enumerate().map(|(i, d)| (u8::from(d.important), 2, 0, i, d)));
    all.sort_by_key(|(imp, origin, spec, order, _)| (*imp, *origin, *spec, *order));

    let own: Vec<&Decl> = all.iter().map(|a| a.4).filter(|d| d.name.starts_with("--")).collect();
    let vars: Vars = if own.is_empty() {
        parent_vars.clone()
    } else {
        let mut m = (**parent_vars).clone();
        for d in own {
            m.insert(d.name.clone(), d.value.clone());
        }
        Rc::new(m)
    };
    let mut props = Props::new();
    for (_, _, _, _, d) in all.iter().filter(|a| !a.4.name.starts_with("--")) {
        match resolve_vars(&d.value, &vars) {
            Some(v) => set_decl(&mut props, &d.name, &collapse_ws(&v)),
            None => warn.add(format!("`{}: {}` uses a custom property that is not defined (ignored)", d.name, d.value)),
        }
    }
    for k in props.keys().filter(|k| !is_known(k)) {
        warn.add(format!("property `{k}` is not supported (ignored)"));
    }
    (props, vars)
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `value` with every `var(--x, fallback)` replaced; `None` when one has neither a value nor a fallback.
pub fn resolve_vars(value: &str, vars: &BTreeMap<String, String>) -> Option<String> {
    let mut v = value.to_string();
    for _ in 0..8 {
        let Some(at) = v.find("var(") else { return Some(v) };
        let inner_start = at + 4;
        let (mut depth, mut end) = (1i32, None);
        for (i, c) in v.get(inner_start..)?.char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(inner_start + i);
                        break;
                    }
                }
                _ => {}
            }
        }
        let end = end?;
        let inner = v.get(inner_start..end)?;
        let parts = split_top(inner, ',');
        let name = parts.first()?.trim();
        let fallback = inner.split_once(',').map(|(_, f)| f.trim().to_string());
        let replacement = vars.get(name).cloned().or(fallback)?;
        v = format!("{}{}{}", v.get(..at)?, replacement, v.get(end + 1..)?);
    }
    (!v.contains("var(")).then_some(v)
}

fn put(props: &mut Props, name: &str, value: &str) {
    props.insert(name.to_string(), value.to_string());
}

/// Store a declaration, expanding shorthands into longhands.
fn set_decl(props: &mut Props, name: &str, value: &str) {
    if matches!(value, "inherit" | "initial" | "unset" | "revert") {
        props.remove(name);
        return;
    }
    match name {
        "margin" | "padding" => box4(props, value, |side| format!("{name}-{side}")),
        "border-width" => box4(props, value, |side| format!("border-{side}-width")),
        "border-style" => box4(props, value, |side| format!("border-{side}-style")),
        "border-color" => box4(props, value, |side| format!("border-{side}-color")),
        "border" => SIDES.iter().for_each(|s| border_side(props, s, value)),
        "border-top" | "border-right" | "border-bottom" | "border-left" => border_side(props, name.trim_start_matches("border-"), value),
        "border-radius" => radius(props, value),
        "background" => background(props, value),
        "background-image" => put(props, name, split_top(value, ',').first().copied().unwrap_or("none").trim()),
        "flex" => flex(props, value),
        "flex-flow" => {
            for t in value.split(' ') {
                put(props, if t.starts_with("wrap") || t == "nowrap" { "flex-wrap" } else { "flex-direction" }, t);
            }
        }
        "gap" | "grid-gap" => {
            let mut it = value.split(' ');
            let row = it.next().unwrap_or("0");
            put(props, "row-gap", row);
            put(props, "column-gap", it.next().unwrap_or(row));
        }
        "font" => font(props, value),
        "overflow-x" | "overflow-y" => {
            if matches!(value, "hidden" | "clip") {
                put(props, "overflow", "hidden");
            }
        }
        "text-decoration-line" => put(props, "text-decoration", value),
        n if is_silent(n) => {}
        n => put(props, n, value),
    }
}

fn box4(props: &mut Props, value: &str, key: impl Fn(&str) -> String) {
    let v: Vec<&str> = split_top(value, ' ').into_iter().filter(|s| !s.is_empty()).collect();
    let (t, r, b, l) = match v.as_slice() {
        [a] => (*a, *a, *a, *a),
        [a, b] => (*a, *b, *a, *b),
        [a, b, c] => (*a, *b, *c, *b),
        [a, b, c, d] => (*a, *b, *c, *d),
        _ => return,
    };
    for (side, val) in SIDES.iter().zip([t, r, b, l]) {
        put(props, &key(side), val);
    }
}

fn border_side(props: &mut Props, side: &str, value: &str) {
    let (mut width, mut style, mut color) = ("medium", "none", "currentcolor");
    for t in split_top(value, ' ').into_iter().filter(|t| !t.is_empty()) {
        match t {
            "none" | "hidden" | "solid" | "dashed" | "dotted" | "double" | "groove" | "ridge" | "inset" | "outset" => style = t,
            "thin" | "medium" | "thick" => width = t,
            t if t.starts_with(|c: char| c.is_ascii_digit() || c == '.') && Color::parse_css(t).is_none() => width = t,
            t => color = t,
        }
    }
    put(props, &format!("border-{side}-width"), width);
    put(props, &format!("border-{side}-style"), style);
    put(props, &format!("border-{side}-color"), color);
}

/// `border-radius: a b c d / ...`: the horizontal radii only, one per corner.
fn radius(props: &mut Props, value: &str) {
    let horizontal = value.split('/').next().unwrap_or("");
    let v: Vec<&str> = horizontal.split(' ').filter(|s| !s.is_empty()).collect();
    let (tl, tr, br, bl) = match v.as_slice() {
        [a] => (*a, *a, *a, *a),
        [a, b] => (*a, *b, *a, *b),
        [a, b, c] => (*a, *b, *c, *b),
        [a, b, c, d] => (*a, *b, *c, *d),
        _ => return,
    };
    for (corner, val) in CORNERS.iter().zip([tl, tr, br, bl]) {
        put(props, &format!("border-{corner}-radius"), val);
    }
}

fn background(props: &mut Props, value: &str) {
    let layers = split_top(value, ',');
    let image = layers.iter().flat_map(|l| split_top(l, ' ')).find(|t| t.contains("gradient(") || t.starts_with("url(")).unwrap_or("none");
    let color = layers.last().into_iter().flat_map(|l| split_top(l, ' ')).find(|t| *t == "currentcolor" || Color::parse_css(t).is_some());
    put(props, "background-image", image);
    put(props, "background-color", color.unwrap_or("transparent"));
}

fn flex(props: &mut Props, value: &str) {
    let toks: Vec<&str> = value.split(' ').collect();
    match toks.as_slice() {
        ["none"] => put(props, "flex-grow", "0"),
        ["auto"] => put(props, "flex-grow", "1"),
        [grow, rest @ ..] if grow.parse::<f64>().is_ok() => {
            put(props, "flex-grow", grow);
            if let Some(basis) = rest.iter().find(|t| t.parse::<f64>().is_err()) {
                put(props, "flex-basis", basis);
            }
        }
        _ => {}
    }
}

/// `font: [style] [weight] size[/line-height] family`.
fn font(props: &mut Props, value: &str) {
    let toks: Vec<&str> = split_top(value, ' ').into_iter().filter(|t| !t.is_empty()).collect();
    let Some(at) = toks.iter().position(|t| {
        t.starts_with(|c: char| c.is_ascii_digit() || c == '.') && (t.contains(|c: char| c.is_ascii_alphabetic() || c == '%') || t.contains('/'))
    }) else {
        return;
    };
    for t in toks.iter().take(at) {
        match *t {
            "italic" | "oblique" => put(props, "font-style", "italic"),
            "bold" | "bolder" | "lighter" => put(props, "font-weight", t),
            t if t.parse::<u16>().is_ok() => put(props, "font-weight", t),
            _ => {}
        }
    }
    let (size, lh) = toks.get(at).map_or(("", None), |t| t.split_once('/').map_or((*t, None), |(s, l)| (s, Some(l))));
    put(props, "font-size", size);
    if let Some(lh) = lh {
        put(props, "line-height", lh);
    }
    put(props, "font-family", &toks.get(at + 1..).unwrap_or(&[]).join(" "));
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Len {
    Px(f64),
    Pct(f64),
    Auto,
}

/// A CSS length: `12px`, `1.5em` (of `em`), `2rem`, `10pt`, `5vw` (of `vw` per 100), `50%`, `0`, `auto`.
pub fn len(v: &str, em: f64, vw: f64) -> Option<Len> {
    let v = v.trim();
    if v == "auto" {
        return Some(Len::Auto);
    }
    if v == "0" {
        return Some(Len::Px(0.0));
    }
    let split = v.find(|c: char| c.is_ascii_alphabetic() || c == '%').unwrap_or(v.len());
    let n: f64 = v.get(..split)?.parse().ok().filter(|n: &f64| n.is_finite())?;
    Some(match v.get(split..)? {
        "px" => Len::Px(n),
        "em" => Len::Px(n * em),
        "rem" => Len::Px(n * 16.0),
        "pt" => Len::Px(n * 4.0 / 3.0),
        "vw" => Len::Px(n * vw / 100.0),
        "%" => Len::Pct(n),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(css: &str) -> Props {
        let mut p = Props::new();
        for d in parse_decls(css) {
            set_decl(&mut p, &d.name, &collapse_ws(&d.value));
        }
        p
    }

    #[test]
    fn shorthands_expand_to_longhands() {
        let p = props(
            "margin: 1px 2px 3px; padding: 4px 5px; border: 2px dashed #f00; border-bottom: none; border-radius: 4px 8px / 2px; flex: 1 1 0; gap: 8px 16px",
        );
        let get = |k: &str| p.get(k).map(String::as_str);
        assert_eq!(
            (get("margin-top"), get("margin-right"), get("margin-bottom"), get("margin-left")),
            (Some("1px"), Some("2px"), Some("3px"), Some("2px"))
        );
        assert_eq!((get("padding-top"), get("padding-left")), (Some("4px"), Some("5px")));
        assert_eq!((get("border-left-width"), get("border-left-style"), get("border-left-color")), (Some("2px"), Some("dashed"), Some("#f00")));
        assert_eq!(get("border-bottom-style"), Some("none"));
        assert_eq!((get("border-top-left-radius"), get("border-top-right-radius")), (Some("4px"), Some("8px")));
        assert_eq!((get("flex-grow"), get("row-gap"), get("column-gap")), (Some("1"), Some("8px"), Some("16px")));
    }

    #[test]
    fn background_font_and_flex_shorthands() {
        let p = props("background: #fff url(x.png) no-repeat; font: italic 700 14px/1.5 'Open Sans', sans-serif");
        assert_eq!((p["background-color"].as_str(), p["background-image"].as_str()), ("#fff", "url(x.png)"));
        assert_eq!(
            (p["font-style"].as_str(), p["font-weight"].as_str(), p["font-size"].as_str(), p["line-height"].as_str()),
            ("italic", "700", "14px", "1.5")
        );
        assert_eq!(p["font-family"], "'Open Sans', sans-serif");
        let g = props("background: linear-gradient(to right, red, blue)");
        assert_eq!((g["background-image"].as_str(), g["background-color"].as_str()), ("linear-gradient(to right, red, blue)", "transparent"));
    }

    #[test]
    fn custom_properties_resolve_with_fallbacks() {
        let vars: BTreeMap<String, String> = [("--a".to_string(), "10px".to_string()), ("--b".to_string(), "var(--a)".to_string())].into();
        assert_eq!(resolve_vars("var(--a) var(--zzz, 3px) var(--b)", &vars).as_deref(), Some("10px 3px 10px"));
        assert_eq!(resolve_vars("var(--zzz)", &vars), None);
        assert_eq!(resolve_vars("var(--a", &vars), None);
        let looping: BTreeMap<String, String> = [("--x".to_string(), "var(--x)".to_string())].into();
        assert_eq!(resolve_vars("var(--x)", &looping), None);
    }

    #[test]
    fn lengths() {
        assert_eq!(len("12px", 16.0, 1000.0), Some(Len::Px(12.0)));
        assert_eq!(len("1.5em", 10.0, 1000.0), Some(Len::Px(15.0)));
        assert_eq!(len("50%", 16.0, 1000.0), Some(Len::Pct(50.0)));
        assert_eq!(len("10vw", 16.0, 1000.0), Some(Len::Px(100.0)));
        assert_eq!(len("0", 16.0, 1000.0), Some(Len::Px(0.0)));
        assert_eq!(len("calc(1px + 2px)", 16.0, 1000.0), None);
        assert_eq!(len("5foo", 16.0, 1000.0), None);
        assert_eq!(len("NaNpx", 16.0, 1000.0), None);
    }
}
