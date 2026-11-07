//! The `contact_sheet` tool's picture: rendered frames in a grid on a dark background, each with
//! its index number in a small box at its corner (a built-in 5x7 digit font, no font files).

use figcraft_render::Pixmap;

/// Most frames on one sheet.
pub const MAX_FRAMES: usize = 64;

/// Margin around the grid and gap between cells, in pixels.
const PAD: u32 = 8;
const GAP: u32 = 8;
/// Sheet background (opaque dark grey).
const BG: [u8; 4] = [38, 38, 38, 255];
/// The most pixels a sheet may take (the PNG is encoded and sent whole).
const MAX_PIXELS: u64 = 64 << 20;

/// 5x7 digits, one byte per row, bit 4 the leftmost pixel.
const FONT: [[u8; 7]; 10] = [
    [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
    [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
    [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
    [0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110],
    [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
    [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
    [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
    [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
    [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
    [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
];

/// `frames` in rows of `cols`, numbered 1.. in reading order, on a sheet whose longer side is
/// about `max_size` pixels: when the grid would be larger, every frame is averaged down first (the
/// numbers are drawn after, at full size).
pub fn compose(frames: &[Pixmap], cols: usize, max_size: u32) -> Result<Pixmap, String> {
    if frames.is_empty() {
        return Err("no frames to put on the sheet".into());
    }
    let cols = cols.clamp(1, frames.len());
    let rows = frames.len().div_ceil(cols);
    let all: Vec<&Pixmap> = frames.iter().collect();
    let (col_w, row_h) = grid(&all, cols);
    let fit = |sum: u32, fixed: u32| (f64::from(max_size) - f64::from(fixed)).max(1.0) / f64::from(sum.max(1));
    let fixed_x = 2 * PAD + GAP * (cols as u32 - 1);
    let fixed_y = 2 * PAD + GAP * (rows as u32 - 1);
    let factor = fit(col_w.iter().sum(), fixed_x).min(fit(row_h.iter().sum(), fixed_y)).min(1.0);

    let small: Vec<Pixmap>;
    let shown: Vec<&Pixmap> = if factor < 1.0 {
        small = frames.iter().map(|p| shrink(p, factor)).collect::<Option<Vec<_>>>().ok_or("contact sheet: out of memory")?;
        small.iter().collect()
    } else {
        all
    };
    let (col_w, row_h) = grid(&shown, cols);
    let (w, h) = (fixed_x + col_w.iter().sum::<u32>(), fixed_y + row_h.iter().sum::<u32>());
    if u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err(format!("the sheet would be {w}x{h} pixels: lower `scale` or `maxSize`"));
    }
    let mut sheet = Pixmap::new(w, h).ok_or("contact sheet: out of memory")?;
    for px in sheet.data_mut().chunks_exact_mut(4) {
        px.copy_from_slice(&BG);
    }
    let digit_scale = if w.max(h) > 1600 { 3 } else { 2 };
    let mut y = PAD;
    for (r, row_height) in row_h.iter().enumerate() {
        let mut x = PAD;
        for (c, col_width) in col_w.iter().enumerate() {
            let i = r * cols + c;
            if let Some(frame) = shown.get(i) {
                let (ox, oy) = (x + (col_width - frame.width()) / 2, y + (row_height - frame.height()) / 2);
                blit(&mut sheet, frame, ox, oy);
                badge(&mut sheet, ox, oy, i + 1, digit_scale);
            }
            x += col_width + GAP;
        }
        y += row_height + GAP;
    }
    Ok(sheet)
}

/// Width of each column and height of each row: the largest frame in it.
fn grid(frames: &[&Pixmap], cols: usize) -> (Vec<u32>, Vec<u32>) {
    let rows = frames.len().div_ceil(cols);
    let mut col_w = vec![0u32; cols];
    let mut row_h = vec![0u32; rows];
    for (i, f) in frames.iter().enumerate() {
        if let Some(w) = col_w.get_mut(i % cols) {
            *w = (*w).max(f.width());
        }
        if let Some(h) = row_h.get_mut(i / cols) {
            *h = (*h).max(f.height());
        }
    }
    (col_w, row_h)
}

/// `src` (premultiplied) over the opaque sheet at (`ox`, `oy`); parts outside the sheet are skipped.
fn blit(dst: &mut Pixmap, src: &Pixmap, ox: u32, oy: u32) {
    let (dw, sw) = (dst.width() as usize, src.width() as usize);
    let (ox, oy) = (ox as usize, oy as usize);
    let dst_data = dst.data_mut();
    for (j, srow) in src.data().chunks_exact(sw * 4).enumerate() {
        let start = ((oy + j) * dw + ox) * 4;
        let Some(drow) = dst_data.get_mut(start..start + sw * 4) else { continue };
        for (s, d) in srow.chunks_exact(4).zip(drow.chunks_exact_mut(4)) {
            let keep = 255 - u32::from(s.get(3).copied().unwrap_or(255));
            for (sc, dc) in s.iter().zip(d.iter_mut()).take(3) {
                *dc = (u32::from(*sc) + (u32::from(*dc) * keep + 127) / 255).min(255) as u8;
            }
        }
    }
}

/// `src` averaged down to `factor` (0..1) of its size; the average of premultiplied pixels is
/// premultiplied too.
fn shrink(src: &Pixmap, factor: f64) -> Option<Pixmap> {
    let (sw, sh) = (src.width() as usize, src.height() as usize);
    let dw = ((sw as f64 * factor).floor() as usize).clamp(1, sw);
    let dh = ((sh as f64 * factor).floor() as usize).clamp(1, sh);
    let mut out = Pixmap::new(u32::try_from(dw).ok()?, u32::try_from(dh).ok()?)?;
    let from = src.data();
    let to = out.data_mut();
    for dy in 0..dh {
        let (y0, y1) = (dy * sh / dh, ((dy + 1) * sh / dh).max(dy * sh / dh + 1).min(sh));
        for dx in 0..dw {
            let (x0, x1) = (dx * sw / dw, ((dx + 1) * sw / dw).max(dx * sw / dw + 1).min(sw));
            let mut sum = [0u32; 4];
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = (y * sw + x) * 4;
                    for (acc, v) in sum.iter_mut().zip(from.get(i..i + 4).unwrap_or(&[0; 4])) {
                        *acc += u32::from(*v);
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)).max(1) as u32;
            let i = (dy * dw + dx) * 4;
            if let Some(px) = to.get_mut(i..i + 4) {
                for (o, acc) in px.iter_mut().zip(sum) {
                    *o = ((acc + n / 2) / n) as u8;
                }
            }
        }
    }
    Some(out)
}

/// Blend grey level `grey` over the pixel at (`x`, `y`) with opacity `alpha` (0..=255).
fn put(sheet: &mut Pixmap, x: u32, y: u32, grey: u8, alpha: u32) {
    let w = sheet.width();
    if x >= w || y >= sheet.height() {
        return;
    }
    let i = (y as usize * w as usize + x as usize) * 4;
    if let Some(px) = sheet.data_mut().get_mut(i..i + 4) {
        // Premultiplied: alpha moves towards opaque the same way (no change on opaque pixels).
        for (k, c) in px.iter_mut().enumerate() {
            let target = if k == 3 { 255 } else { u32::from(grey) };
            *c = ((target * alpha + u32::from(*c) * (255 - alpha) + 127) / 255) as u8;
        }
    }
}

/// The number `n` in white 5x7 digits, each pixel `k` square, on a dark rounded box whose top left
/// is (`x`, `y`).
fn badge(sheet: &mut Pixmap, x: u32, y: u32, n: usize, k: u32) {
    let digits: Vec<usize> = n.to_string().bytes().map(|b| usize::from(b.saturating_sub(b'0'))).collect();
    let pad = 2 * k;
    let text_w = digits.len() as u32 * 5 * k + (digits.len() as u32).saturating_sub(1) * k;
    let (bw, bh) = (text_w + 2 * pad, 7 * k + 2 * pad);
    let r = 2 * k;
    for j in 0..bh {
        for i in 0..bw {
            let (dx, dy) = (i.min(bw - 1 - i), j.min(bh - 1 - j));
            // Rounded corners: skip the pixels outside a circle of radius r.
            if dx < r && dy < r && (r - dx) * (r - dx) + (r - dy) * (r - dy) > r * r + r {
                continue;
            }
            put(sheet, x + i, y + j, 0, 205);
        }
    }
    for (d, digit) in digits.iter().enumerate() {
        if let Some(rows) = FONT.get(*digit) {
            draw_digit(sheet, rows, (x + pad + d as u32 * 6 * k, y + pad), k);
        }
    }
}

/// One digit's `rows` in white, each font pixel `k` square, its top left at `at`.
fn draw_digit(sheet: &mut Pixmap, rows: &[u8; 7], at: (u32, u32), k: u32) {
    for (ry, bits) in rows.iter().enumerate() {
        for cx in 0..5u32 {
            if (bits >> (4 - cx)) & 1 == 0 {
                continue;
            }
            for (sx, sy) in (0..k).flat_map(|sx| (0..k).map(move |sy| (sx, sy))) {
                put(sheet, at.0 + cx * k + sx, at.1 + ry as u32 * k + sy, 255, 255);
            }
        }
    }
}

/// Most badges `screenshot` draws by default.
pub const MAX_MARKS: usize = 60;

/// Badges on `pix` for the `screenshot` tool's `marks`: badge `labels[i]` is wanted with its top
/// left at `at[i]` (image pixels). It is moved inside the image and off the badges drawn before it
/// (nudged down, then right; with no free place it stays at the last try). Returns where each ended.
pub fn draw_marks(pix: &mut Pixmap, labels: &[u64], at: &[(i64, i64)]) -> Vec<(u32, u32)> {
    let (w, h) = (i64::from(pix.width()), i64::from(pix.height()));
    let k = match w.max(h) {
        0..=499 => 1,
        500..=1599 => 2,
        _ => 3,
    };
    let mut placed: Vec<(i64, i64, i64, i64)> = Vec::new();
    let mut out = Vec::new();
    for (label, (wx, wy)) in labels.iter().zip(at) {
        let n = label.to_string().len() as i64;
        let (bw, bh) = (n * 5 * k + (n - 1) * k + 4 * k, 7 * k + 4 * k);
        let clamp = |x: i64, y: i64| (x.clamp(0, (w - bw).max(0)), y.clamp(0, (h - bh).max(0)));
        let hit =
            |x: i64, y: i64, p: &[(i64, i64, i64, i64)]| p.iter().any(|(px, py, pw, ph)| x < px + pw && *px < x + bw && y < py + ph && *py < y + bh);
        let (mut x, mut y) = clamp(*wx, *wy);
        'search: for _ in 0..40 {
            if !hit(x, y, &placed) {
                break;
            }
            for (dx, dy) in [(0, bh + 1), (bw + 1, 0)] {
                let (nx, ny) = clamp(x + dx, y + dy);
                if (nx, ny) != (x, y) && !hit(nx, ny, &placed) {
                    (x, y) = (nx, ny);
                    break 'search;
                }
            }
            (x, y) = clamp(x + bw + 1, y + bh + 1);
        }
        placed.push((x, y, bw, bh));
        badge(pix, x as u32, y as u32, *label as usize, k as u32);
        out.push((x as u32, y as u32));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, rgba: [u8; 4]) -> Pixmap {
        let mut p = Pixmap::new(w, h).unwrap();
        for px in p.data_mut().chunks_exact_mut(4) {
            px.copy_from_slice(&rgba);
        }
        p
    }

    fn at(p: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        let i = (y * p.width() + x) as usize * 4;
        [p.data()[i], p.data()[i + 1], p.data()[i + 2], p.data()[i + 3]]
    }

    #[test]
    fn frames_sit_in_a_grid_with_margins_and_gaps() {
        let frames = [solid(40, 30, [255, 0, 0, 255]), solid(20, 50, [0, 255, 0, 255]), solid(30, 10, [0, 0, 255, 255])];
        let s = compose(&frames, 2, 2400).unwrap();
        // Columns 40 and 20 wide, rows 50 and 10 tall.
        assert_eq!((s.width(), s.height()), (8 + 40 + 8 + 20 + 8, 8 + 50 + 8 + 10 + 8));
        // Red is centred in its 40x50 cell (5 px down), green starts after the gap, beside the red one.
        assert_eq!(at(&s, 30, 40), [255, 0, 0, 255]);
        assert_eq!(at(&s, 60, 40), [0, 255, 0, 255]);
        assert_eq!(at(&s, 4, 4), BG);
        // Blue is on the second row under the first column, centred (5 px in), clear of its badge.
        assert_eq!(at(&s, 8 + 5 + 25, 8 + 50 + 8 + 4), [0, 0, 255, 255]);
    }

    #[test]
    fn a_translucent_frame_is_drawn_over_the_background() {
        let s = compose(&[solid(60, 40, [0, 0, 0, 0])], 4, 2400).unwrap();
        assert_eq!(at(&s, 40, 30), BG);
        let half = compose(&[solid(60, 40, [127, 127, 127, 127])], 1, 2400).unwrap();
        let [r, g, b, a] = at(&half, 50, 35);
        assert_eq!((r, g, b, a), (127 + 19, 127 + 19, 127 + 19, 255));
    }

    #[test]
    fn each_frame_gets_a_numbered_badge_at_its_corner() {
        let s = compose(&[solid(80, 60, [200, 200, 200, 255]), solid(80, 60, [200, 200, 200, 255])], 2, 2400).unwrap();
        // The badge is a dark box with white digits: both are found next to the frame's corner.
        let corner =
            |x0: u32| -> Vec<[u8; 4]> { (0..16).flat_map(|dy| (0..16).map(move |dx| (dx, dy))).map(|(dx, dy)| at(&s, x0 + dx, 8 + dy)).collect() };
        for x0 in [8, 8 + 80 + 8] {
            assert!(corner(x0).iter().any(|p| p[0] > 250), "white digit pixels at {x0}");
            assert!(corner(x0).iter().any(|p| p[0] < 120), "dark box at {x0}");
        }
        // The two numbers differ.
        assert_ne!((0..16).map(|dx| at(&s, 8 + dx, 12)).collect::<Vec<_>>(), (0..16).map(|dx| at(&s, 96 + dx, 12)).collect::<Vec<_>>());
    }

    #[test]
    fn a_grid_over_max_size_is_averaged_down_to_it() {
        let frames: Vec<Pixmap> = (0..4).map(|_| solid(400, 300, [10, 20, 30, 255])).collect();
        let s = compose(&frames, 2, 500).unwrap();
        assert!(s.width() <= 500 && s.height() <= 500, "{}x{}", s.width(), s.height());
        assert!(s.width() > 400, "{}", s.width());
        // A flat colour stays that colour when averaged.
        assert_eq!(&at(&s, 108, 108)[..3], &[10, 20, 30]);
    }

    #[test]
    fn marks_stay_inside_and_do_not_overlap() {
        let mut p = solid(200, 100, [200, 200, 200, 255]);
        // Three badges wanted at one corner, one past the bottom right.
        let spots = draw_marks(&mut p, &[1, 2, 3, 4], &[(0, 0), (0, 0), (0, 0), (500, 90)]);
        assert_eq!(spots.len(), 4);
        assert_eq!(spots[0], (0, 0));
        for (i, (x, y)) in spots.iter().enumerate() {
            assert!(*x + 9 <= 200 && *y + 11 <= 100, "badge {i} at {x},{y}");
            for (j, (x2, y2)) in spots.iter().enumerate().skip(i + 1) {
                assert!(x.abs_diff(*x2) >= 9 || y.abs_diff(*y2) >= 11, "badges {i} and {j} overlap");
            }
        }
        let px: Vec<[u8; 4]> = (0..100).map(|i| at(&p, i % 10, i / 10)).collect();
        assert!(px.iter().any(|c| c[0] > 250) && px.iter().any(|c| c[0] < 120));
    }

    #[test]
    fn no_frames_is_an_error() {
        assert!(compose(&[], 4, 2400).is_err());
    }
}
