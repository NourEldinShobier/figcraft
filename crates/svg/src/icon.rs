//! The icon preset: tidies an exported SVG for use as an icon. Per-path matrix transforms are baked
//! into the path data (uniform scales keep the stroke width right), groups without their own look
//! are unwrapped, a clip that is just the view box is dropped, one chosen colour becomes
//! `currentColor`, and numbers keep at most 3 decimals. It works on the exporter's own output
//! (absolute `M L Q C Z` path data, double-quoted attributes); anything it does not understand is
//! left as it is.

use std::fmt::Write;

use crate::num;

struct El {
    name: String,
    attrs: Vec<(String, String)>,
    kids: Vec<Node>,
    /// Written as `<name/>` when it has no children.
    empty: bool,
}

enum Node {
    El(El),
    Text(String),
}

impl El {
    fn attr(&self, key: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
    fn remove(&mut self, key: &str) {
        self.attrs.retain(|(k, _)| k != key);
    }
    fn set(&mut self, key: &str, value: String) {
        match self.attrs.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = value,
            None => self.attrs.push((key.to_string(), value)),
        }
    }
}

/// The tree of `svg` (an XML declaration and comments dropped), or None when it is not well formed.
fn parse(svg: &str) -> Option<Vec<Node>> {
    let mut stack: Vec<El> = vec![El { name: String::new(), attrs: vec![], kids: vec![], empty: false }];
    let mut rest = svg;
    while !rest.is_empty() {
        if !rest.starts_with('<') {
            let end = rest.find('<').unwrap_or(rest.len());
            let (text, tail) = rest.split_at(end);
            if !text.trim().is_empty() {
                stack.last_mut()?.kids.push(Node::Text(text.to_string()));
            }
            rest = tail;
        } else if let Some(r) = rest.strip_prefix("<?") {
            rest = &r[r.find("?>")? + 2..];
        } else if let Some(r) = rest.strip_prefix("<!--") {
            rest = &r[r.find("-->")? + 3..];
        } else if let Some(r) = rest.strip_prefix("</") {
            let end = r.find('>')?;
            let done = stack.pop()?;
            if done.name != r[..end].trim() || stack.is_empty() {
                return None;
            }
            stack.last_mut()?.kids.push(Node::El(done));
            rest = &r[end + 1..];
        } else {
            let r = &rest[1..];
            let name_end = r.find(|c: char| c.is_whitespace() || c == '/' || c == '>')?;
            let mut el = El { name: r[..name_end].to_string(), attrs: vec![], kids: vec![], empty: false };
            let mut r = &r[name_end..];
            loop {
                r = r.trim_start();
                if let Some(t) = r.strip_prefix("/>") {
                    el.empty = true;
                    stack.last_mut()?.kids.push(Node::El(el));
                    rest = t;
                    break;
                }
                if let Some(t) = r.strip_prefix('>') {
                    stack.push(el);
                    rest = t;
                    break;
                }
                let eq = r.find('=')?;
                let key = r[..eq].trim().to_string();
                let v = r[eq + 1..].trim_start().strip_prefix('"')?;
                let close = v.find('"')?;
                el.attrs.push((key, v[..close].to_string()));
                r = &v[close + 1..];
            }
        }
    }
    let root = stack.pop()?;
    stack.is_empty().then_some(root.kids)
}

fn write_node(out: &mut String, n: &Node) {
    match n {
        Node::Text(t) => out.push_str(t),
        Node::El(e) => {
            let _ = write!(out, "<{}", e.name);
            for (k, v) in &e.attrs {
                let _ = write!(out, " {k}=\"{v}\"");
            }
            if e.kids.is_empty() && e.empty {
                out.push_str("/>");
            } else {
                out.push('>');
                e.kids.iter().for_each(|k| write_node(out, k));
                let _ = write!(out, "</{}>", e.name);
            }
        }
    }
}

type Matrix = [f64; 6];

fn numbers(s: &str) -> Option<Vec<f64>> {
    s.split(|c: char| c.is_whitespace() || c == ',').filter(|t| !t.is_empty()).map(|t| t.parse::<f64>().ok().filter(|v| v.is_finite())).collect()
}

fn parse_matrix(s: &str) -> Option<Matrix> {
    let inner = s.trim().strip_prefix("matrix(")?.strip_suffix(')')?;
    numbers(inner)?.try_into().ok()
}

fn apply(m: &Matrix, x: f64, y: f64) -> (f64, f64) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

/// Path data of absolute `M L Q C Z` commands as (command, numbers); None for anything else.
fn parse_path(d: &str) -> Option<Vec<(char, Vec<f64>)>> {
    let mut out: Vec<(char, Vec<f64>)> = Vec::new();
    let mut start = None;
    for (i, c) in d.char_indices().chain(std::iter::once((d.len(), 'Z'))) {
        if c.is_ascii_alphabetic() {
            if let Some((s, cmd)) = start.take() {
                let cmd: char = cmd;
                out.push((cmd, numbers(&d[s..i])?));
            }
            if i == d.len() {
                break;
            }
            if !matches!(c, 'M' | 'L' | 'Q' | 'C' | 'Z') {
                return None;
            }
            start = Some((i + 1, c));
            if c == 'Z' {
                out.push(('Z', vec![]));
                start = None;
            }
        }
    }
    let arity = |c| match c {
        'M' | 'L' => 2,
        'Q' => 4,
        'C' => 6,
        _ => 0,
    };
    out.iter().all(|(c, v)| v.len() == arity(*c)).then_some(out)
}

fn path_string(cmds: &[(char, Vec<f64>)]) -> String {
    let mut s = String::new();
    for (c, v) in cmds {
        s.push(*c);
        s.push_str(&v.iter().map(|n| num(*n)).collect::<Vec<_>>().join(" "));
    }
    s
}

/// Bakes `transform` into the path data of a `<path>`; false (nothing changed) when it can't be done exactly.
fn bake(e: &mut El) -> bool {
    let Some(m) = e.attr("transform").and_then(parse_matrix) else { return false };
    let Some(d) = e.attr("d").and_then(parse_path) else { return false };
    let (sx, sy) = ((m[0] * m[0] + m[1] * m[1]).sqrt(), (m[2] * m[2] + m[3] * m[3]).sqrt());
    let uniform = (sx - sy).abs() <= 1e-6 * sx.max(1.0) && (m[0] * m[3] - m[1] * m[2]) > 0.0 && (m[0] * m[2] + m[1] * m[3]).abs() <= 1e-6 * sx * sy;
    let has_stroke_width = e.attr("stroke-width").is_some();
    if !uniform && (has_stroke_width || e.attr("stroke-dasharray").is_some()) {
        return false;
    }
    if !uniform && e.attr("fill").is_some_and(|f| f.starts_with("url(")) {
        return false;
    }
    let moved: Vec<(char, Vec<f64>)> = d
        .into_iter()
        .map(|(c, v)| {
            let v = v.chunks(2).flat_map(|p| p.first().zip(p.get(1)).map(|(x, y)| apply(&m, *x, *y)).map(|(x, y)| [x, y]).unwrap_or_default()).collect();
            (c, v)
        })
        .collect();
    e.set("d", path_string(&moved));
    e.remove("transform");
    if uniform {
        if let Some(w) = e.attr("stroke-width").and_then(|w| w.parse::<f64>().ok()) {
            e.set("stroke-width", num(w * sx));
        }
        if let Some(a) = e.attr("stroke-dasharray").map(|a| numbers(a).unwrap_or_default()).filter(|a| !a.is_empty()) {
            e.set("stroke-dasharray", a.iter().map(|n| num(n * sx)).collect::<Vec<_>>().join(" "));
        }
    }
    true
}

fn bake_all(n: &mut Node) {
    if let Node::El(e) = n {
        if e.name == "path" {
            bake(e);
        }
        e.kids.iter_mut().for_each(bake_all);
    }
}

/// Is `e` (a `<clipPath>`) the rectangle `0 0 w h`?
fn clip_is_view(e: &El, w: f64, h: f64) -> bool {
    let [Node::El(p)] = e.kids.as_slice() else { return false };
    if p.name != "path" || p.attr("transform").is_some() {
        return false;
    }
    let Some(cmds) = p.attr("d").and_then(parse_path) else { return false };
    let pts: Vec<(f64, f64)> = cmds.iter().filter(|(c, _)| matches!(c, 'M' | 'L')).filter_map(|(_, v)| v.first().zip(v.get(1)).map(|(x, y)| (*x, *y))).collect();
    cmds.iter().all(|(c, _)| matches!(c, 'M' | 'L' | 'Z'))
        && pts.len() == 4
        && pts.iter().all(|(x, y)| [0.0, w].iter().any(|v| (x - v).abs() < 0.01) && [0.0, h].iter().any(|v| (y - v).abs() < 0.01))
}

fn clip_ref(e: &El) -> Option<String> {
    e.attr("clip-path")?.strip_prefix("url(#")?.strip_suffix(')').map(str::to_string)
}

fn collect_view_clips(n: &Node, w: f64, h: f64, out: &mut Vec<String>) {
    if let Node::El(e) = n {
        if e.name == "clipPath"
            && clip_is_view(e, w, h)
            && let Some(id) = e.attr("id")
        {
            out.push(id.to_string());
        }
        e.kids.iter().for_each(|k| collect_view_clips(k, w, h, out));
    }
}

fn drop_clips(n: &mut Node, ids: &[String]) {
    if let Node::El(e) = n {
        if clip_ref(e).is_some_and(|id| ids.contains(&id)) {
            e.remove("clip-path");
        }
        e.kids.iter_mut().for_each(|k| drop_clips(k, ids));
    }
}

/// Unwraps groups with no attributes, and merges a lone child's group attributes in when the
/// child has none of the same.
fn flatten(kids: Vec<Node>) -> Vec<Node> {
    let mut out = Vec::with_capacity(kids.len());
    for k in kids {
        let mut e = match k {
            Node::El(e) => e,
            text => {
                out.push(text);
                continue;
            }
        };
        e.kids = flatten(std::mem::take(&mut e.kids));
        if e.name != "g" {
            out.push(Node::El(e));
            continue;
        }
        if e.attrs.is_empty() {
            out.extend(e.kids);
            continue;
        }
        let mergeable = |c: &El| c.name != "g" && e.attrs.iter().all(|(k, _)| k != "id" && k != "opacity" && c.attr(k).is_none());
        if let [Node::El(c)] = e.kids.as_mut_slice()
            && mergeable(c)
        {
            let attrs = std::mem::take(&mut e.attrs);
            c.attrs.extend(attrs);
            out.extend(e.kids);
            continue;
        }
        out.push(Node::El(e));
    }
    out
}

fn used_ids(n: &Node, out: &mut String) {
    if let Node::El(e) = n {
        for (k, v) in &e.attrs {
            if k != "id" {
                out.push_str(v);
                out.push(' ');
            }
        }
        e.kids.iter().for_each(|k| used_ids(k, out));
    }
}

/// Removes `<clipPath>` definitions nothing refers to, then an empty `<defs>`.
fn prune_defs(kids: &mut Vec<Node>, used: &str) {
    for k in kids.iter_mut() {
        if let Node::El(e) = k
            && e.name == "defs"
        {
            e.kids.retain(|d| !matches!(d, Node::El(c) if c.name == "clipPath" && c.attr("id").is_some_and(|id| !used.contains(&format!("url(#{id})")))));
        }
    }
    kids.retain(|k| !matches!(k, Node::El(e) if e.name == "defs" && e.kids.is_empty()));
}

fn colors(n: &Node, out: &mut Vec<String>) {
    if let Node::El(e) = n {
        for key in ["fill", "stroke"] {
            if let Some(v) = e.attr(key).filter(|v| v.starts_with('#')) {
                let v = v.to_ascii_lowercase();
                if !out.contains(&v) {
                    out.push(v);
                }
            }
        }
        e.kids.iter().for_each(|k| colors(k, out));
    }
}

fn recolor(n: &mut Node, color: &str) {
    if let Node::El(e) = n {
        for key in ["fill", "stroke"] {
            if e.attr(key).is_some_and(|v| v.eq_ignore_ascii_case(color)) {
                e.set(key, "currentColor".into());
            }
        }
        e.kids.iter_mut().for_each(|k| recolor(k, color));
    }
}

fn round_attrs(n: &mut Node) {
    if let Node::El(e) = n {
        for key in ["stroke-width", "stroke-miterlimit", "opacity", "fill-opacity", "stroke-opacity"] {
            if let Some(v) = e.attr(key).and_then(|v| v.parse::<f64>().ok()) {
                e.set(key, num(v));
            }
        }
        e.kids.iter_mut().for_each(round_attrs);
    }
}

/// `svg` as an icon. `current_color`: `Some("auto")` turns the colour into `currentColor` when
/// the icon has exactly one, `Some("#rrggbb")` turns that colour, `None` keeps colours.
pub fn icon_preset(svg: &str, current_color: Option<&str>) -> String {
    let Some(mut tree) = parse(svg) else { return svg.to_string() };
    let [Node::El(root)] = tree.as_mut_slice() else { return svg.to_string() };
    if root.name != "svg" {
        return svg.to_string();
    }
    let view = root.attr("viewBox").and_then(numbers).filter(|v| v.len() == 4);
    root.kids.iter_mut().for_each(bake_all);
    if let Some([_, _, w, h]) = view.as_deref() {
        let mut clips = Vec::new();
        root.kids.iter().for_each(|k| collect_view_clips(k, *w, *h, &mut clips));
        root.kids.iter_mut().for_each(|k| drop_clips(k, &clips));
    }
    root.kids = flatten(std::mem::take(&mut root.kids));
    let mut used = String::new();
    root.kids.iter().for_each(|k| used_ids(k, &mut used));
    prune_defs(&mut root.kids, &used);
    root.kids.iter_mut().for_each(round_attrs);
    let wanted = match current_color {
        Some("auto") => {
            let mut found = Vec::new();
            root.kids.iter().for_each(|k| colors(k, &mut found));
            (found.len() == 1).then(|| found.remove(0))
        }
        Some(c) => Some(c.to_ascii_lowercase()),
        None => None,
    };
    if let Some(c) = wanted {
        root.kids.iter_mut().for_each(|k| recolor(k, &c));
    }
    let mut out = String::new();
    tree.iter().for_each(|n| write_node(&mut out, n));
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::icon_preset;

    #[test]
    fn transforms_are_baked_groups_unwrapped_and_the_view_clip_dropped() {
        let svg = "<svg width=\"24\" height=\"24\" viewBox=\"0 0 24 24\" fill=\"none\" xmlns=\"http://www.w3.org/2000/svg\">\
<g><g clip-path=\"url(#c1)\"><path transform=\"matrix(1 0 0 1 3 5)\" d=\"M0 0L10 0L10 7.0000000001L0 7Z\" stroke=\"#111111\" stroke-width=\"2.0000001\" stroke-linecap=\"round\"/></g></g>\
<defs><clipPath id=\"c1\"><path d=\"M0 0L24 0L24 24L0 24Z\"/></clipPath></defs></svg>\n";
        assert_eq!(
            icon_preset(svg, Some("auto")),
            "<svg width=\"24\" height=\"24\" viewBox=\"0 0 24 24\" fill=\"none\" xmlns=\"http://www.w3.org/2000/svg\">\
<path d=\"M3 5L13 5L13 12L3 12Z\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\"/></svg>\n"
        );
    }

    #[test]
    fn uniform_scale_scales_the_stroke_and_two_colours_stay() {
        let svg = "<svg viewBox=\"0 0 24 24\"><path transform=\"matrix(2 0 0 2 1 1)\" d=\"M1 1L2 2\" stroke=\"#000000\" stroke-width=\"1.5\"/><path d=\"M0 0\" fill=\"#ff0000\"/></svg>";
        assert_eq!(
            icon_preset(svg, Some("auto")),
            "<svg viewBox=\"0 0 24 24\"><path d=\"M3 3L5 5\" stroke=\"#000000\" stroke-width=\"3\"/><path d=\"M0 0\" fill=\"#ff0000\"/></svg>\n"
        );
        assert_eq!(
            icon_preset(svg, Some("#FF0000")),
            "<svg viewBox=\"0 0 24 24\"><path d=\"M3 3L5 5\" stroke=\"#000000\" stroke-width=\"3\"/><path d=\"M0 0\" fill=\"currentColor\"/></svg>\n"
        );
    }

    #[test]
    fn non_uniform_stroked_transforms_and_junk_are_left_alone() {
        let kept = "<svg viewBox=\"0 0 24 24\"><path transform=\"matrix(2 0 0 1 0 0)\" d=\"M1 1L2 2\" stroke-width=\"1\"/></svg>\n";
        assert_eq!(icon_preset(kept, None), kept);
        assert_eq!(icon_preset("not xml <", None), "not xml <");
    }

    #[test]
    fn a_group_with_one_attribute_and_one_child_merges_into_the_child() {
        let svg = "<svg viewBox=\"0 0 8 8\"><g stroke-linejoin=\"round\"><path d=\"M0 0L1 1\"/></g><g opacity=\"0.5\"><path d=\"M0 0\"/></g></svg>";
        assert_eq!(
            icon_preset(svg, None),
            "<svg viewBox=\"0 0 8 8\"><path d=\"M0 0L1 1\" stroke-linejoin=\"round\"/><g opacity=\"0.5\"><path d=\"M0 0\"/></g></svg>\n"
        );
    }
}
