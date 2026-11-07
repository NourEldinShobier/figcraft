//! Backgrounds, borders, corner radii and shadows for `import.html`.

use figcraft_color::Color;
use serde_json::{Map, Value, json};

use super::Warn;
use super::css::split_top;
use super::style::{Len, Props, len};

/// A colour value; `currentcolor` is `current`.
pub fn color(v: &str, current: Color) -> Option<Color> {
    if v.eq_ignore_ascii_case("currentcolor") { Some(current) } else { Color::parse_css(v) }
}

/// Fills of an element: its background colour, then its gradient on top.
pub fn fills(p: &Props, current: Color, size: (Option<f64>, Option<f64>), warn: &mut Warn) -> Vec<Value> {
    let mut out = Vec::new();
    if let Some(bg) = p.get("background-color") {
        match color(bg, current) {
            Some(c) if c.a > 0.0 => out.push(json!(c.to_css_hex())),
            Some(_) => {}
            None => warn.add(format!("background-color `{bg}` is not supported (ignored)")),
        }
    }
    let image = p.get("background-image").map(String::as_str).filter(|i| *i != "none");
    if let Some(img) = image {
        match img.strip_prefix("linear-gradient(").and_then(|r| r.strip_suffix(')')).and_then(|args| linear_gradient(args, current, size)) {
            Some(g) => out.push(g),
            None if img.starts_with("url(") => warn.add("background-image: url(...) is not supported (ignored)"),
            None => {
                warn.add(format!("background-image `{}` is not supported (only simple linear-gradient)", img.chars().take(40).collect::<String>()))
            }
        }
    }
    out
}

fn angle_of(first: &str) -> Option<f64> {
    if let Some(dir) = first.strip_prefix("to ") {
        let mut words = dir.split(' ');
        return Some(match (words.next()?, words.next()) {
            ("top", None) => 0.0,
            ("right", None) => 90.0,
            ("bottom", None) => 180.0,
            ("left", None) => 270.0,
            ("top", Some("right")) | ("right", Some("top")) => 45.0,
            ("bottom", Some("right")) | ("right", Some("bottom")) => 135.0,
            ("bottom", Some("left")) | ("left", Some("bottom")) => 225.0,
            ("top", Some("left")) | ("left", Some("top")) => 315.0,
            _ => return None,
        });
    }
    let (n, unit) = first.split_at(first.find(|c: char| c.is_ascii_alphabetic())?);
    let n: f64 = n.parse().ok()?;
    match unit {
        "deg" => Some(n),
        "turn" => Some(n * 360.0),
        "rad" => Some(n.to_degrees()),
        "grad" => Some(n * 0.9),
        _ => None,
    }
}

/// A linear gradient's stops: each a colour and optional percentage position, spread evenly where none is given.
fn stops(args: &[&str], current: Color) -> Option<Vec<(Color, f64)>> {
    let mut parsed: Vec<(Color, Option<f64>)> = Vec::new();
    for a in args {
        let parts: Vec<&str> = split_top(a.trim(), ' ').into_iter().filter(|s| !s.is_empty()).collect();
        let c = color(parts.first()?, current)?;
        let pos = parts.get(1).and_then(|p| p.strip_suffix('%')).and_then(|p| p.parse::<f64>().ok()).map(|p| (p / 100.0).clamp(0.0, 1.0));
        parsed.push((c, pos));
    }
    if parsed.len() < 2 {
        return None;
    }
    let last = parsed.len() - 1;
    if let Some(f) = parsed.first_mut().filter(|f| f.1.is_none()) {
        f.1 = Some(0.0);
    }
    if let Some(l) = parsed.last_mut().filter(|l| l.1.is_none()) {
        l.1 = Some(1.0);
    }
    let mut out: Vec<(Color, f64)> = Vec::new();
    let mut i = 0;
    while i <= last {
        let Some((c, Some(p))) = parsed.get(i).copied() else {
            // A run of unplaced stops between two placed ones.
            let from = i.checked_sub(1)?;
            let to = (i..=last).find(|j| parsed.get(*j).is_some_and(|s| s.1.is_some()))?;
            let (a, b) = (parsed.get(from)?.1?, parsed.get(to)?.1?);
            for (k, j) in (from + 1..to).enumerate() {
                let t = (k + 1) as f64 / (to - from) as f64;
                out.push((parsed.get(j)?.0, a + (b - a) * t));
            }
            i = to;
            continue;
        };
        out.push((c, p.max(out.last().map_or(0.0, |l| l.1))));
        i += 1;
    }
    Some(out)
}

fn linear_gradient(args: &str, current: Color, size: (Option<f64>, Option<f64>)) -> Option<Value> {
    let parts = split_top(args, ',');
    let (angle, rest) = match parts.first().and_then(|f| angle_of(f.trim())) {
        Some(a) => (a, parts.get(1..)?),
        None => (180.0, parts.as_slice()),
    };
    let stops = stops(rest, current)?;
    let (w, h) = (size.0.unwrap_or(100.0).max(1.0), size.1.unwrap_or(100.0).max(1.0));
    let r = angle.to_radians();
    let (dx, dy) = (r.sin(), -r.cos());
    let half = (w * dx).abs() / 2.0 + (h * dy).abs() / 2.0;
    // `+ 0.0` turns -0.0 into 0.0.
    let round = |v: f64| (v * 1e4).round() / 1e4 + 0.0;
    let at = |sign: f64| json!([round(0.5 + sign * dx * half / w), round(0.5 + sign * dy * half / h)]);
    let stops: Vec<Value> = stops.iter().map(|(c, p)| json!({"position": p, "color": c.to_css_hex()})).collect();
    Some(json!({"type": "linearGradient", "stops": stops, "from": at(-1.0), "to": at(1.0)}))
}

/// A border: width per side (top, right, bottom, left; 0: none), one colour and the dash style.
pub struct Border {
    pub widths: [f64; 4],
    color: Color,
    dash: Option<&'static str>,
}

pub fn border(p: &Props, current: Color, em: f64, vw: f64, warn: &mut Warn) -> Option<Border> {
    let mut widths = [0.0; 4];
    let (mut found, mut dash) = (None::<Color>, None);
    let mut mixed = false;
    for (i, side) in ["top", "right", "bottom", "left"].iter().enumerate() {
        let style = p.get(&format!("border-{side}-style")).map_or("none", String::as_str);
        if matches!(style, "none" | "hidden") {
            continue;
        }
        let w = match p.get(&format!("border-{side}-width")).map_or("medium", String::as_str) {
            "thin" => 1.0,
            "medium" => 3.0,
            "thick" => 5.0,
            v => match len(v, em, vw) {
                Some(Len::Px(n)) if n >= 0.0 => n,
                _ => {
                    warn.add(format!("border width `{v}` is not supported (ignored)"));
                    0.0
                }
            },
        };
        if let Some(slot) = widths.get_mut(i) {
            *slot = w;
        }
        let c = p.get(&format!("border-{side}-color")).and_then(|c| color(c, current)).unwrap_or(current);
        mixed |= w > 0.0 && found.is_some_and(|f| f != c);
        if w > 0.0 {
            found.get_or_insert(c);
            dash = dash.or(match style {
                "dashed" => Some("dashed"),
                "dotted" => Some("dotted"),
                _ => None,
            });
        }
    }
    if mixed {
        warn.add("borders with a different colour on each side use the first colour");
    }
    found.map(|color| Border { widths, color, dash })
}

impl Border {
    pub fn props(&self, out: &mut Map<String, Value>) {
        let [t, r, b, l] = self.widths;
        out.insert("stroke".into(), json!(self.color.to_css_hex()));
        out.insert("strokeAlign".into(), json!("inside"));
        out.insert("strokesIncluded".into(), json!(true));
        if t == r && r == b && b == l {
            out.insert("strokeWeight".into(), json!(t));
        } else {
            for (k, w) in [("strokeTopWeight", t), ("strokeRightWeight", r), ("strokeBottomWeight", b), ("strokeLeftWeight", l)] {
                out.insert(k.into(), json!(w));
            }
        }
        let w = t.max(r).max(b).max(l);
        match self.dash {
            Some("dashed") => out.insert("dashes".into(), json!([w * 3.0, w * 2.0])),
            Some(_) => out.insert("dashes".into(), json!([w, w])),
            None => None,
        };
    }
}

/// `cornerRadius` from the four corner values (top-left, top-right, bottom-right, bottom-left).
pub fn radius(p: &Props, em: f64, vw: f64, size: (Option<f64>, Option<f64>), warn: &mut Warn) -> Option<Value> {
    let mut radii = [0.0f64; 4];
    for (i, corner) in ["top-left", "top-right", "bottom-right", "bottom-left"].iter().enumerate() {
        let Some(v) = p.get(&format!("border-{corner}-radius")) else { continue };
        let r = match len(v, em, vw) {
            Some(Len::Px(n)) => n,
            // Half the shorter side is a full round (a pill, a circle); unknown sizes get a big radius.
            Some(Len::Pct(pc)) => match size {
                (Some(w), Some(h)) => w.min(h) * pc.min(50.0) / 100.0,
                _ => 9999.0 * pc.min(50.0) / 50.0,
            },
            _ => {
                warn.add(format!("border-radius `{v}` is not supported (ignored)"));
                0.0
            }
        };
        if let Some(slot) = radii.get_mut(i) {
            *slot = r.clamp(0.0, 9999.0);
        }
    }
    let [a, b, c, d] = radii;
    if radii.iter().all(|r| *r == 0.0) {
        None
    } else if a == b && b == c && c == d {
        Some(json!(a))
    } else {
        Some(json!(radii))
    }
}

/// Effects from `box-shadow`: drop shadows, and inner ones for `inset`.
pub fn shadows(v: &str, current: Color, em: f64, vw: f64, warn: &mut Warn) -> Vec<Value> {
    if v == "none" {
        return Vec::new();
    }
    let mut out = Vec::new();
    for layer in split_top(v, ',').into_iter().take(8) {
        match one_shadow(layer, current, em, vw) {
            Some(e) => out.push(e),
            None => warn.add(format!("box-shadow `{}` is not supported (ignored)", layer.trim())),
        }
    }
    out
}

fn one_shadow(layer: &str, current: Color, em: f64, vw: f64) -> Option<Value> {
    let (mut inset, mut nums, mut col) = (false, Vec::new(), None);
    for t in split_top(layer.trim(), ' ').into_iter().filter(|t| !t.is_empty()) {
        if t == "inset" {
            inset = true;
        } else if let Some(Len::Px(n)) = len(t, em, vw) {
            nums.push(n);
        } else {
            col = Some(color(t, current)?);
        }
    }
    let [x, y, rest @ ..] = nums.as_slice() else { return None };
    let blur = rest.first().copied().unwrap_or(0.0).max(0.0);
    let spread = rest.get(1).copied().unwrap_or(0.0);
    let c = col.unwrap_or(current);
    Some(json!({
        "type": if inset { "innerShadow" } else { "dropShadow" },
        "color": c.to_css_hex(),
        "offset": [x, y],
        "radius": blur,
        "spread": spread,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn black() -> Color {
        Color::new(0.0, 0.0, 0.0, 1.0)
    }

    #[test]
    fn gradients_get_stops_and_a_direction() {
        let mut w = Warn::default();
        let mut p = Props::new();
        p.insert("background-image".into(), "linear-gradient(to right, #ff0000, #00ff00 50%, rgba(0,0,255,0.5))".into());
        let f = fills(&p, black(), (Some(200.0), Some(100.0)), &mut w);
        assert!(w.list.is_empty(), "{:?}", w.list);
        assert_eq!(f[0]["from"], json!([0.0, 0.5]));
        assert_eq!(f[0]["to"], json!([1.0, 0.5]));
        assert_eq!(f[0]["stops"][1], json!({"position": 0.5, "color": "#00FF00"}));
        assert_eq!(f[0]["stops"][2]["position"], json!(1.0));
        let mut q = Props::new();
        q.insert("background-image".into(), "linear-gradient(red, blue)".into());
        let g = fills(&q, black(), (Some(10.0), Some(10.0)), &mut w);
        assert_eq!(g[0]["from"], json!([0.5, 0.0]));
    }

    #[test]
    fn unplaced_stops_spread_between_placed_ones() {
        let s = stops(&["red", "lime", "blue", "white"], black()).unwrap();
        let pos: Vec<f64> = s.iter().map(|s| (s.1 * 100.0).round()).collect();
        assert_eq!(pos, [0.0, 33.0, 67.0, 100.0]);
    }

    #[test]
    fn unsupported_backgrounds_warn() {
        let mut w = Warn::default();
        let mut p = Props::new();
        p.insert("background-image".into(), "url(a.png)".into());
        assert!(fills(&p, black(), (None, None), &mut w).is_empty());
        p.insert("background-image".into(), "radial-gradient(red, blue)".into());
        let _ = fills(&p, black(), (None, None), &mut w);
        assert_eq!(w.list.len(), 2, "{:?}", w.list);
    }

    #[test]
    fn borders_radii_and_shadows() {
        let mut w = Warn::default();
        let mut p = Props::new();
        for side in ["top", "right", "bottom", "left"] {
            p.insert(format!("border-{side}-style"), if side == "left" { "none" } else { "solid" }.into());
            p.insert(format!("border-{side}-width"), "2px".into());
            p.insert(format!("border-{side}-color"), "#123456".into());
        }
        let b = border(&p, black(), 16.0, 1000.0, &mut w).unwrap();
        assert_eq!(b.widths, [2.0, 2.0, 2.0, 0.0]);
        let mut m = Map::new();
        b.props(&mut m);
        assert_eq!((m["stroke"].clone(), m["strokeLeftWeight"].clone(), m["strokeTopWeight"].clone()), (json!("#123456"), json!(0.0), json!(2.0)));
        let mut r = Props::new();
        r.insert("border-top-left-radius".into(), "50%".into());
        assert_eq!(radius(&r, 16.0, 1000.0, (Some(40.0), Some(20.0)), &mut w), Some(json!([10.0, 0.0, 0.0, 0.0])));
        let s = shadows("0 4px 8px rgba(0,0,0,.25), inset 1px 1px 0 2px #fff", black(), 16.0, 1000.0, &mut w);
        assert_eq!(s[0], json!({"type": "dropShadow", "color": "#00000040", "offset": [0.0, 4.0], "radius": 8.0, "spread": 0.0}));
        assert_eq!(s[1]["type"], json!("innerShadow"));
        assert_eq!(s[1]["spread"], json!(2.0));
        assert!(shadows("bogus", black(), 16.0, 1000.0, &mut w).is_empty());
    }
}
