//! Accessibility maths from public specs: WCAG 2.x contrast ratio and success criteria 1.4.3 / 1.4.6,
//! the APCA lightness contrast (Lc) of the public SAPC-4g (0.98G-4g) formula, and colour-vision
//! deficiency simulation with the matrices of Machado, Oliveira and Fernandes (2009, severity 1.0).

use crate::Color;

fn lin(c: f32) -> f64 {
    let c = f64::from(c.clamp(0.0, 1.0));
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn unlin(c: f64) -> f32 {
    let c = c.clamp(0.0, 1.0);
    (if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }) as f32
}

/// WCAG relative luminance of an sRGB colour (alpha ignored).
pub fn relative_luminance(c: Color) -> f64 {
    0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
}

/// `fg` painted over `bg` (Porter-Duff over); the result has alpha `fg.a + bg.a * (1 - fg.a)`.
pub fn over(fg: Color, bg: Color) -> Color {
    let (fa, ba) = (fg.a, bg.a);
    let a = fa + ba * (1.0 - fa);
    if a <= 0.0 {
        return Color::TRANSPARENT;
    }
    let mix = |f: f32, b: f32| (f * fa + b * ba * (1.0 - fa)) / a;
    Color::new(mix(fg.r, bg.r), mix(fg.g, bg.g), mix(fg.b, bg.b), a)
}

/// WCAG 2.x contrast ratio, 1 to 21, of `fg` on `bg`; translucent colours are first composited
/// over white (`bg`) and over that background (`fg`).
pub fn wcag_ratio(fg: Color, bg: Color) -> f64 {
    let bg = over(bg, Color::WHITE);
    let fg = over(fg, bg);
    let (a, b) = (relative_luminance(fg) + 0.05, relative_luminance(bg) + 0.05);
    if a > b { a / b } else { b / a }
}

/// APCA Lc (about -108 to 106; negative is light text on a dark background) of `text` on `bg`
/// (SAPC 0.98G-4g constants).
pub fn apca_lc(text: Color, bg: Color) -> f64 {
    let bg = over(bg, Color::WHITE);
    let text = over(text, bg);
    let y = |c: Color| {
        let g = |v: f32| f64::from(v.clamp(0.0, 1.0)).powf(2.4);
        let y = 0.212_672_9 * g(c.r) + 0.715_152_2 * g(c.g) + 0.072_175 * g(c.b);
        if y < 0.022 { y + (0.022 - y).powf(1.414) } else { y }
    };
    let (yt, yb) = (y(text), y(bg));
    if (yb - yt).abs() < 0.0005 {
        return 0.0;
    }
    let lc = if yb > yt {
        let s = (yb.powf(0.56) - yt.powf(0.57)) * 1.14;
        if s < 0.1 { 0.0 } else { s - 0.027 }
    } else {
        let s = (yb.powf(0.65) - yt.powf(0.62)) * 1.14;
        if s > -0.1 { 0.0 } else { s + 0.027 }
    };
    lc * 100.0
}

/// WCAG large text: at least 24 px (18 pt), or 18.66 px (14 pt) and bold.
pub fn is_large_text(font_size_px: f64, bold: bool) -> bool {
    font_size_px >= 24.0 || (bold && font_size_px >= 18.66)
}

/// The ratio level 1.4.3 (AA) or 1.4.6 (AAA) asks of text.
pub fn wcag_required(large: bool, aaa: bool) -> f64 {
    match (aaa, large) {
        (false, false) => 4.5,
        (false, true) | (true, true) => 3.0 + if aaa { 1.5 } else { 0.0 },
        (true, false) => 7.0,
    }
}

/// The least |Lc| this crate calls readable: 75 for body text, 60 for large text (the public
/// APCA "bronze" guidance in simplified form).
pub fn apca_required(large: bool) -> f64 {
    if large { 60.0 } else { 75.0 }
}

/// `text` moved towards black or white (whichever needs the smaller move) until its WCAG ratio on
/// `bg` reaches `target`; None when neither end reaches it.
pub fn fix_text_colour(text: Color, bg: Color, target: f64) -> Option<Color> {
    let solid = over(text, over(bg, Color::WHITE)).with_alpha(1.0);
    // Judge the 8-bit colour that would be written, not the exact blend.
    let q = |c: Color| {
        let [r, g, b, _] = c.to_rgba8();
        Color::from_rgba8(r, g, b, 255)
    };
    let best = |end: Color| -> Option<(f32, Color)> {
        if wcag_ratio(end, bg) < target {
            return None;
        }
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..24 {
            let mid = (lo + hi) / 2.0;
            if wcag_ratio(q(solid.lerp(end, mid)), bg) >= target { hi = mid } else { lo = mid }
        }
        Some((hi, q(solid.lerp(end, hi))))
    };
    match (best(Color::BLACK), best(Color::WHITE)) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a.1 } else { b.1 }),
        (a, b) => a.or(b).map(|x| x.1),
    }
}

/// A colour vision deficiency to simulate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cvd {
    Protanopia,
    Deuteranopia,
    Tritanopia,
    Achromatopsia,
}

impl Cvd {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "protanopia" | "protan" => Cvd::Protanopia,
            "deuteranopia" | "deutan" => Cvd::Deuteranopia,
            "tritanopia" | "tritan" => Cvd::Tritanopia,
            "achromatopsia" | "monochromacy" | "grayscale" | "greyscale" => Cvd::Achromatopsia,
            _ => return None,
        })
    }

    pub fn id(self) -> &'static str {
        match self {
            Cvd::Protanopia => "protanopia",
            Cvd::Deuteranopia => "deuteranopia",
            Cvd::Tritanopia => "tritanopia",
            Cvd::Achromatopsia => "achromatopsia",
        }
    }

    /// The 3x3 matrix applied to linear RGB (rows are the output r, g, b).
    pub fn matrix(self) -> [[f64; 3]; 3] {
        match self {
            Cvd::Protanopia => [[0.152_286, 1.052_583, -0.204_868], [0.114_503, 0.786_281, 0.099_216], [-0.003_882, -0.048_116, 1.051_998]],
            Cvd::Deuteranopia => [[0.367_322, 0.860_646, -0.227_968], [0.280_085, 0.672_501, 0.047_413], [-0.011_820, 0.042_940, 0.968_881]],
            Cvd::Tritanopia => [[1.255_528, -0.076_749, -0.178_779], [-0.078_411, 0.930_809, 0.147_602], [0.004_733, 0.691_367, 0.303_900]],
            // Relative luminance: every channel gets Y.
            Cvd::Achromatopsia => [[0.2126, 0.7152, 0.0722]; 3],
        }
    }

    /// How `c` looks (alpha kept).
    pub fn simulate(self, c: Color) -> Color {
        let m = self.matrix();
        let v = [lin(c.r), lin(c.g), lin(c.b)];
        let row = |r: &[f64; 3]| unlin(r[0] * v[0] + r[1] * v[1] + r[2] * v[2]);
        Color::new(row(&m[0]), row(&m[1]), row(&m[2]), c.a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Color {
        Color::from_hex(s).unwrap()
    }

    #[test]
    fn grey_767676_on_white_is_4_54() {
        assert_eq!(format!("{:.2}", wcag_ratio(hex("#767676"), Color::WHITE)), "4.54");
        assert_eq!(format!("{:.1}", wcag_ratio(Color::BLACK, Color::WHITE)), "21.0");
        assert_eq!(format!("{:.2}", wcag_ratio(hex("#777777"), Color::WHITE)), "4.48");
    }

    #[test]
    fn apca_black_on_white_and_reverse() {
        assert_eq!(format!("{:.1}", apca_lc(Color::BLACK, Color::WHITE)), "106.0");
        assert!(apca_lc(Color::WHITE, Color::BLACK) < -100.0);
        assert_eq!(apca_lc(hex("#808080"), hex("#808080")), 0.0);
    }

    #[test]
    fn thresholds_follow_wcag() {
        assert_eq!(wcag_required(false, false), 4.5);
        assert_eq!(wcag_required(true, false), 3.0);
        assert_eq!(wcag_required(false, true), 7.0);
        assert_eq!(wcag_required(true, true), 4.5);
        assert!(is_large_text(24.0, false) && is_large_text(19.0, true) && !is_large_text(19.0, false));
    }

    #[test]
    fn translucent_text_is_blended_over_the_background() {
        // 50% black over white is #808080-ish: far below black's 21.
        let r = wcag_ratio(Color::BLACK.with_alpha(0.5), Color::WHITE);
        assert!((r - 3.95).abs() < 0.1, "{r}");
    }

    #[test]
    fn fix_reaches_the_target() {
        let fixed = fix_text_colour(hex("#999999"), Color::WHITE, 4.5).unwrap();
        assert!(wcag_ratio(fixed, Color::WHITE) >= 4.5);
        assert_eq!(fixed.to_hex(), "767676");
    }

    #[test]
    fn cvd_keeps_greys_and_collapses_red_green() {
        let grey = hex("#808080");
        for c in [Cvd::Protanopia, Cvd::Deuteranopia, Cvd::Tritanopia, Cvd::Achromatopsia] {
            let g = c.simulate(grey).to_rgba8();
            assert!(g[0].abs_diff(128) <= 2 && g[1].abs_diff(128) <= 2 && g[2].abs_diff(128) <= 2, "{c:?} {g:?}");
        }
        let red = Cvd::Deuteranopia.simulate(hex("#FF0000")).to_rgba8();
        let green = Cvd::Deuteranopia.simulate(hex("#00FF00")).to_rgba8();
        assert!(red[2] < 40 && green[2] < 80, "{red:?} {green:?}");
        assert_eq!(Cvd::Achromatopsia.simulate(hex("#FF0000")).to_hex(), "7F7F7F");
        assert_eq!(Cvd::parse("Deuteranopia"), Some(Cvd::Deuteranopia));
        assert_eq!(Cvd::parse("blue"), None);
    }
}
