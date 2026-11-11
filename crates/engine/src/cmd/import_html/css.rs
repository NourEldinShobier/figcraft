//! A hand-written CSS parser for `import.html`: style sheets of rules with simple selectors (tag,
//! `.class`, `#id`, `:root`, `*`, compounds of those and the descendant combinator), declaration
//! lists, and the matching of selectors against the parsed document. Everything else is skipped
//! with a warning.

use std::collections::HashMap;

use super::Warn;
use super::dom::Dom;
use super::style::{Len, len};

/// Most rules kept from all style sheets.
const MAX_RULES: usize = 10_000;

pub struct Decl {
    pub name: String,
    pub value: String,
    pub important: bool,
}

/// A structural pseudo-class: the element's place among its element siblings (1-based).
#[derive(Clone, Copy)]
enum Pseudo {
    First,
    Last,
    Nth(usize),
}

#[derive(Default)]
struct Compound {
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
    pseudos: Vec<Pseudo>,
}

pub struct Selector {
    /// Leftmost first; each part is an ancestor of the next.
    parts: Vec<Compound>,
    /// (ids, classes, tags) packed in one number so that it sorts.
    pub specificity: u32,
}

pub struct Rule {
    selector: Selector,
    pub decls: Vec<Decl>,
    pub order: usize,
}

/// `s` without `/* ... */` comments.
fn strip_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find("/*") {
        out.push_str(rest.get(..at).unwrap_or(""));
        rest = rest.get(at + 2..).unwrap_or("");
        rest = rest.find("*/").and_then(|e| rest.get(e + 2..)).unwrap_or("");
    }
    out.push_str(rest);
    out
}

/// Split `s` at `sep` characters outside parentheses, brackets, braces and quotes.
pub fn split_top(s: &str, sep: char) -> Vec<&str> {
    let (mut out, mut depth, mut quote, mut start) = (Vec::new(), 0i32, None::<char>, 0usize);
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '(' | '[' | '{') => depth += 1,
            (None, ')' | ']' | '}') => depth = (depth - 1).max(0),
            (None, c) if c == sep && depth == 0 => {
                out.push(s.get(start..i).unwrap_or(""));
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(s.get(start..).unwrap_or(""));
    out
}

/// Declarations of a block body (`a: b; c: d !important`).
pub fn parse_decls(body: &str) -> Vec<Decl> {
    let body = strip_comments(body);
    split_top(&body, ';')
        .into_iter()
        .filter(|d| !d.contains('{'))
        .filter_map(|d| {
            let (name, value) = d.split_once(':')?;
            let name = name.trim().to_ascii_lowercase();
            let mut value = value.trim();
            let important = value.to_ascii_lowercase().ends_with("!important");
            if important {
                value = value.get(..value.len() - "!important".len()).unwrap_or("").trim_end();
            }
            (!name.is_empty() && !value.is_empty() && !name.contains(char::is_whitespace)).then(|| Decl { name, value: value.to_string(), important })
        })
        .collect()
}

/// The inside of the `{ ... }` block that starts `s`, and what follows it.
fn block(s: &str) -> (&str, &str) {
    let (mut depth, mut quote) = (0i32, None::<char>);
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '{') => depth += 1,
            (None, '}') => {
                depth -= 1;
                if depth == 0 {
                    return (s.get(1..i).unwrap_or(""), s.get(i + 1..).unwrap_or(""));
                }
            }
            _ => {}
        }
    }
    (s.get(1..).unwrap_or(""), "")
}

/// After the at-rule that starts `s`: past its `;` or its block.
fn skip_at_rule(s: &str) -> &str {
    let semi = split_top(s, ';').first().map_or(s.len(), |p| p.len());
    match s.find('{') {
        Some(open) if open < semi => block(s.get(open..).unwrap_or("")).1,
        _ => s.get(semi + 1..).unwrap_or(""),
    }
}

/// Deepest `@media` nesting evaluated.
const MAX_MEDIA_DEPTH: usize = 3;

/// Whether a media query holds at viewport width `vw`: `None` when it uses anything but
/// `all`/`screen` and `(min-width: n)` / `(max-width: n)` joined by `and`.
fn media_matches(query: &str, vw: f64) -> Option<bool> {
    let q = query.trim().to_ascii_lowercase();
    let mut holds = true;
    for part in q.split(" and ") {
        match part.trim() {
            "all" | "screen" | "only screen" => {}
            part => {
                let (feature, value) = part.strip_prefix('(')?.strip_suffix(')')?.split_once(':')?;
                let Some(Len::Px(px)) = len(value.trim(), 16.0, vw) else { return None };
                match feature.trim() {
                    "max-width" => holds &= vw <= px,
                    "min-width" => holds &= vw >= px,
                    _ => return None,
                }
            }
        }
    }
    Some(holds)
}

/// Append the rules of the style sheet `src` to `rules`; `@media` blocks that hold at width `vw` apply.
pub fn parse_sheet(src: &str, rules: &mut Vec<Rule>, warn: &mut Warn, vw: f64) {
    sheet(src, rules, warn, vw, 0);
}

fn sheet(src: &str, rules: &mut Vec<Rule>, warn: &mut Warn, vw: f64, depth: usize) {
    let src = strip_comments(src);
    let mut rest = src.as_str();
    loop {
        rest = rest.trim_start();
        if rest.is_empty() || rules.len() >= MAX_RULES {
            if rules.len() >= MAX_RULES {
                warn.add(format!("more than {MAX_RULES} CSS rules: the rest is ignored"));
            }
            return;
        }
        if let Some(at) = rest.strip_prefix('@') {
            let name: String = at.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
            if name == "media"
                && depth < MAX_MEDIA_DEPTH
                && let Some(open) = rest.find('{')
            {
                let query = rest.get("@media".len()..open).unwrap_or("");
                let (body, after) = block(rest.get(open..).unwrap_or(""));
                match media_matches(query, vw) {
                    Some(true) => sheet(body, rules, warn, vw, depth + 1),
                    Some(false) => {}
                    None => warn.add(format!("media query `{}` is not supported (skipped; only min-width and max-width work)", query.trim())),
                }
                rest = after;
                continue;
            }
            if name != "charset" {
                warn.add(format!("at-rule @{name} is not supported (skipped)"));
            }
            rest = skip_at_rule(rest);
            continue;
        }
        let Some(open) = rest.find('{') else { return };
        let prelude = rest.get(..open).unwrap_or("");
        let (body, after) = block(rest.get(open..).unwrap_or(""));
        rest = after;
        let decls_of = || parse_decls(body);
        for text in split_top(prelude, ',') {
            match parse_selector(text) {
                Some(selector) => {
                    let order = rules.len();
                    rules.push(Rule { selector, decls: decls_of(), order });
                }
                None => warn.add(format!(
                    "selector `{}` is not supported (rule skipped; only tag, .class, #id, descendant, :first-child, :last-child and :nth-child(n) selectors work)",
                    text.trim()
                )),
            }
        }
    }
}

fn ident(s: &str) -> Option<(&str, &str)> {
    let end = s.find(|c: char| !(c.is_alphanumeric() || matches!(c, '-' | '_') || !c.is_ascii())).unwrap_or(s.len());
    (end > 0).then(|| s.split_at(end))
}

fn parse_compound(text: &str) -> Option<Compound> {
    let mut c = Compound::default();
    let mut rest = text;
    if let Some(r) = rest.strip_prefix('*') {
        rest = r;
    } else if let Some(r) = rest.strip_prefix(":root") {
        c.tag = Some("html".into());
        rest = r;
    } else if let Some((tag, r)) = ident(rest) {
        c.tag = Some(tag.to_ascii_lowercase());
        rest = r;
    }
    while !rest.is_empty() {
        let (kind, r) = (rest.chars().next()?, rest.get(1..)?);
        let (name, mut r) = ident(r)?;
        match kind {
            '.' => c.classes.push(name.to_string()),
            '#' if c.id.is_none() => c.id = Some(name.to_string()),
            ':' => c.pseudos.push(match name {
                "first-child" => Pseudo::First,
                "last-child" => Pseudo::Last,
                "nth-child" => {
                    // A plain number only: `(3)`, not `(2n+1)` or `(odd)`.
                    let close = r.strip_prefix('(')?.find(')')?;
                    let digits = r.get(1..close + 1)?.trim();
                    let n = digits.chars().all(|c| c.is_ascii_digit()).then(|| digits.parse::<usize>().ok()).flatten().filter(|n| *n >= 1)?;
                    r = r.get(close + 2..)?;
                    Pseudo::Nth(n)
                }
                _ => return None,
            }),
            _ => return None,
        }
        rest = r;
    }
    Some(c)
}

/// (position, count) of `el` among the element children of its parent.
fn sibling_place(dom: &Dom, el: usize) -> (usize, usize) {
    let Some(parent) = dom.parent(el) else { return (1, 1) };
    let siblings = || dom.children(parent).iter().filter(|c| dom.tag(**c).is_some());
    (siblings().position(|c| *c == el).map_or(1, |i| i + 1), siblings().count())
}

fn parse_selector(text: &str) -> Option<Selector> {
    let parts: Vec<Compound> = text.split_whitespace().map(parse_compound).collect::<Option<_>>()?;
    if parts.is_empty() {
        return None;
    }
    let count = |f: fn(&Compound) -> usize| parts.iter().map(f).sum::<usize>() as u32;
    let specificity = count(|c| usize::from(c.id.is_some())) * 10_000
        + count(|c| c.classes.len() + c.pseudos.len()) * 100
        + count(|c| usize::from(c.tag.is_some()));
    Some(Selector { parts, specificity })
}

fn compound_matches(dom: &Dom, el: usize, c: &Compound) -> bool {
    if c.tag.as_deref().is_some_and(|t| dom.tag(el) != Some(t)) {
        return false;
    }
    if c.id.as_deref().is_some_and(|id| dom.attr(el, "id") != Some(id)) {
        return false;
    }
    let class_attr = dom.attr(el, "class").unwrap_or("");
    if !c.classes.iter().all(|want| class_attr.split_whitespace().any(|have| have == want)) {
        return false;
    }
    if c.pseudos.is_empty() {
        return true;
    }
    let (pos, count) = sibling_place(dom, el);
    c.pseudos.iter().all(|p| match p {
        Pseudo::First => pos == 1,
        Pseudo::Last => pos == count,
        Pseudo::Nth(n) => pos == *n,
    })
}

fn selector_matches(dom: &Dom, el: usize, sel: &Selector) -> bool {
    let mut parts = sel.parts.iter().rev();
    let Some(last) = parts.next() else { return false };
    if !compound_matches(dom, el, last) {
        return false;
    }
    let mut at = el;
    for part in parts {
        let mut up = dom.parent(at);
        loop {
            match up {
                None => return false,
                Some(a) if compound_matches(dom, a, part) => {
                    at = a;
                    break;
                }
                Some(a) => up = dom.parent(a),
            }
        }
    }
    true
}

/// The rules, looked up by the id, class or tag their right-most compound names.
pub struct Index {
    rules: Vec<Rule>,
    by_id: HashMap<String, Vec<usize>>,
    by_class: HashMap<String, Vec<usize>>,
    by_tag: HashMap<String, Vec<usize>>,
    any: Vec<usize>,
}

impl Index {
    pub fn new(rules: Vec<Rule>) -> Self {
        let mut ix = Index { rules: Vec::new(), by_id: HashMap::new(), by_class: HashMap::new(), by_tag: HashMap::new(), any: Vec::new() };
        for (i, r) in rules.iter().enumerate() {
            let Some(last) = r.selector.parts.last() else { continue };
            if let Some(id) = &last.id {
                ix.by_id.entry(id.clone()).or_default().push(i);
            } else if let Some(class) = last.classes.first() {
                ix.by_class.entry(class.clone()).or_default().push(i);
            } else if let Some(tag) = &last.tag {
                ix.by_tag.entry(tag.clone()).or_default().push(i);
            } else {
                ix.any.push(i);
            }
        }
        ix.rules = rules;
        ix
    }

    /// The rules that match `el`, in no particular order.
    pub fn matching(&self, dom: &Dom, el: usize) -> Vec<&Rule> {
        let mut cands: Vec<usize> = self.any.clone();
        if let Some(l) = dom.attr(el, "id").and_then(|id| self.by_id.get(id)) {
            cands.extend(l);
        }
        for class in dom.attr(el, "class").unwrap_or("").split_whitespace() {
            if let Some(l) = self.by_class.get(class) {
                cands.extend(l);
            }
        }
        if let Some(l) = dom.tag(el).and_then(|t| self.by_tag.get(t)) {
            cands.extend(l);
        }
        cands.iter().filter_map(|i| self.rules.get(*i)).filter(|r| selector_matches(dom, el, &r.selector)).collect()
    }
}

impl Rule {
    pub fn specificity(&self) -> u32 {
        self.selector.specificity
    }
}

#[cfg(test)]
mod tests {
    use super::super::dom::parse;
    use super::*;

    fn sheet(src: &str) -> (Index, Vec<String>) {
        sheet_at(src, 1440.0)
    }

    fn sheet_at(src: &str, vw: f64) -> (Index, Vec<String>) {
        let (mut rules, mut warn) = (Vec::new(), Warn::default());
        parse_sheet(src, &mut rules, &mut warn, vw);
        (Index::new(rules), warn.list)
    }

    fn matched(ix: &Index, dom: &Dom, el: usize) -> Vec<String> {
        let mut got: Vec<_> = ix.matching(dom, el).iter().map(|r| r.decls[0].value.clone()).collect();
        got.sort();
        got
    }

    #[test]
    fn structural_pseudo_classes_match_by_place_among_element_siblings() {
        let dom = parse("<ul><li>a</li> text <li>b</li><li>c</li></ul>");
        let (ix, warns) = sheet("li:first-child{a:1} li:last-child{a:2} li:nth-child(2){a:3} :nth-child(3){a:4} li:nth-child(4){a:5}");
        assert!(warns.is_empty(), "{warns:?}");
        let lis: Vec<usize> = dom.children(dom.find("ul").unwrap()).iter().copied().filter(|c| dom.tag(*c) == Some("li")).collect();
        assert_eq!(matched(&ix, &dom, lis[0]), ["1"]);
        assert_eq!(matched(&ix, &dom, lis[1]), ["3"]);
        assert_eq!(matched(&ix, &dom, lis[2]), ["2", "4"]);
        let (_, warns) = sheet("li:nth-child(2n+1){a:1} li:nth-child(odd){a:1} li:nth-child(){a:1} li:hover{a:1} li:nth-child(0){a:1}");
        assert_eq!(warns.len(), 5, "{warns:?}");
        assert_eq!(sheet("li:first-child.x{a:1}").0.rules[0].specificity(), 201);
    }

    #[test]
    fn media_queries_apply_by_viewport_width() {
        let css = "p{a:base} @media (max-width: 800px) { p{a:narrow} } @media screen and (min-width: 1000px) and (max-width: 1500px) { p{a:wide} } @media (min-width: 100em) { p{a:em} }";
        let dom = parse("<p>x</p>");
        let p = dom.find("p").unwrap();
        let (ix, warns) = sheet_at(css, 700.0);
        assert!(warns.is_empty(), "{warns:?}");
        assert_eq!(matched(&ix, &dom, p), ["base", "narrow"]);
        assert_eq!(matched(&sheet_at(css, 1440.0).0, &dom, p), ["base", "wide"]);
        assert_eq!(matched(&sheet_at(css, 1600.0).0, &dom, p), ["base", "em"]);
        // The boundary counts as inside, and a nested query applies too.
        assert_eq!(matched(&sheet_at("@media (max-width: 800px) { @media (min-width: 800px) { p{a:both} } }", 800.0).0, &dom, p), ["both"]);
        let (ix, warns) =
            sheet_at("@media print { p{a:1} } @media (orientation: portrait) { p{a:1} } @media (max-width: 1px) { p{a:1} } @media { p{a:1} }", 100.0);
        assert!(ix.rules.is_empty());
        assert_eq!(warns.len(), 3, "{warns:?}");
        assert!(warns.iter().all(|w| w.contains("media query")));
    }

    #[test]
    fn selectors_match_by_tag_class_id_and_descendants() {
        let dom = parse("<div id=a class='card big'><section><p class=t>x</p></section></div><p class=t>y</p>");
        let (ix, warns) = sheet("p{a:1} .t{a:2} div .t{a:3} #a p{a:4} .card.big section p{a:5} .nope p{a:6} *{a:7}");
        assert!(warns.is_empty(), "{warns:?}");
        let inner = dom.find("p").unwrap();
        let mut got: Vec<_> = ix.matching(&dom, inner).iter().map(|r| r.decls[0].value.clone()).collect();
        got.sort();
        assert_eq!(got, ["1", "2", "3", "4", "5", "7"]);
        let outer = dom.children(0)[1];
        let mut got: Vec<_> = ix.matching(&dom, outer).iter().map(|r| r.decls[0].value.clone()).collect();
        got.sort();
        assert_eq!(got, ["1", "2", "7"]);
    }

    #[test]
    fn specificity_orders_ids_over_classes_over_tags() {
        let (ix, _) = sheet("#i{a:1} .c.d{a:1} div p{a:1}");
        let spec: Vec<u32> = ix.rules.iter().map(Rule::specificity).collect();
        assert_eq!(spec, [10_000, 200, 2]);
    }

    #[test]
    fn unsupported_rules_warn_and_the_rest_still_applies() {
        let (ix, warns) = sheet(
            "/* c */ @media print { p { color: red } } a:hover { x: y } ul > li { x: y } @import url(x.css); h1, h2 { margin: 0 } @charset 'utf-8';",
        );
        assert_eq!(ix.rules.len(), 2, "h1 and h2");
        assert_eq!(warns.len(), 4, "{warns:?}");
        assert!(warns[0].contains("media query") && warns[1].contains("a:hover") && warns[2].contains("ul > li") && warns[3].contains("@import"));
    }

    #[test]
    fn declarations_keep_semicolons_inside_urls_and_flag_important() {
        let d = parse_decls("background: url(data:image/png;base64,AAA) no-repeat; COLOR : red !important; ; bogus; width:1px");
        let got: Vec<_> = d.iter().map(|d| (d.name.as_str(), d.value.as_str(), d.important)).collect();
        assert_eq!(got, [("background", "url(data:image/png;base64,AAA) no-repeat", false), ("color", "red", true), ("width", "1px", false)]);
    }

    #[test]
    fn junk_sheets_do_not_panic() {
        for junk in ["{", "}", "a{", "a{b", "@", "@media{", "a{b:c;;;}}}", "\u{0}{}", "a b c d e f g{x:y}", ".{}", "#{}", "*.{}"] {
            let _ = sheet(junk);
        }
        let _ = parse_decls("a:b:c;:;;a;");
    }
}
