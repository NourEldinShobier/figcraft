//! Colour-vision-deficiency preview: a post-filter over rendered pixels.

pub use figcraft_color::a11y::Cvd;

use crate::Pixmap;

/// Simulate `cvd` over `pix` in place (premultiplied pixels; the matrix is applied to linear RGB).
pub fn apply(pix: &mut Pixmap, cvd: Cvd) {
    let m = cvd.matrix();
    let to_lin: Vec<f32> = (0..256)
        .map(|i| {
            let c = i as f32 / 255.0;
            if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        })
        .collect();
    let to_srgb = |c: f32| {
        let c = c.clamp(0.0, 1.0);
        let v = if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
        (v * 255.0).round() as u8
    };
    let m = m.map(|r| r.map(|x| x as f32));
    for px in pix.data_mut().chunks_exact_mut(4) {
        let a = px[3];
        if a == 0 {
            continue;
        }
        let af = f32::from(a) / 255.0;
        let un = |c: u8| (f32::from(c) / 255.0 / af).min(1.0);
        let lin = |c: u8| to_lin.get((un(c) * 255.0).round() as usize).copied().unwrap_or(1.0);
        let (r, g, b) = (lin(px[0]), lin(px[1]), lin(px[2]));
        for (k, row) in m.iter().enumerate() {
            let v = to_srgb(row[0] * r + row[1] * g + row[2] * b);
            if let Some(slot) = px.get_mut(k) {
                *slot = (f32::from(v) * af).round() as u8;
            }
        }
    }
}

/// `png` (encoded) with `cvd` simulated, re-encoded as PNG.
pub fn filter_png(png: &[u8], cvd: Cvd) -> Result<Vec<u8>, String> {
    let mut pix = crate::images::decode(png).ok_or("cvd: can't read the rendered image")?;
    apply(&mut pix, cvd);
    crate::encode_png(&pix)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deuteranopia_turns_red_into_dark_olive_and_keeps_transparency() {
        let mut pix = Pixmap::new(2, 1).unwrap();
        pix.data_mut()[..4].copy_from_slice(&[255, 0, 0, 255]);
        apply(&mut pix, Cvd::Deuteranopia);
        let d = pix.data();
        // Linear (0.367, 0.280, -0.012 -> 0) is sRGB (163, 144, 0).
        assert_eq!(&d[..4], &[163, 144, 0, 255]);
        assert_eq!(&d[4..], &[0, 0, 0, 0]);
    }

    #[test]
    fn png_round_trip_filters_every_pixel() {
        let mut pix = Pixmap::new(1, 1).unwrap();
        pix.data_mut().copy_from_slice(&[0, 255, 0, 255]);
        let png = crate::encode_png(&pix).unwrap();
        let out = crate::images::decode(&filter_png(&png, Cvd::Achromatopsia).unwrap()).unwrap();
        assert_eq!(out.data(), &[220, 220, 220, 255]);
    }
}
