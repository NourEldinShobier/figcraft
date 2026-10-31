//! Render timings on a large file; run by hand:
//! `FIGCRAFT_BENCH_FILE=x.fig cargo test -p figcraft-engine --release --test render_bench -- --ignored --nocapture`
use figcraft_render::{Options, View, render_view};
use std::hash::{Hash, Hasher};
use std::time::Instant;

#[test]
#[ignore]
#[allow(clippy::disallowed_methods)] // a hand-run bench reads its own file, outside any automation roots
fn render_timings() {
    let Ok(path) = std::env::var("FIGCRAFT_BENCH_FILE") else { return };
    let page_name = std::env::var("FIGCRAFT_BENCH_PAGE").unwrap_or_else(|_| "git".into());
    let bytes = std::fs::read(&path).unwrap();
    let (doc, _) = figcraft_format::fig::import(&bytes, "bench").unwrap();
    let page = doc.pages.iter().copied().find(|p| doc.node(*p).is_some_and(|n| n.name == page_name)).unwrap();
    let mut xs = Vec::new();
    let mut ys = Vec::new();
    let mut all = None::<figcraft_geom::Rect>;
    for (i, n) in doc.nodes().enumerate() {
        if i % 40 != 0 || doc.page_of(n.id) != Some(page) || n.is_page() {
            continue;
        }
        if let Some(b) = doc.world_bounds(n.id) {
            xs.push((b.x0 + b.x1) / 2.0);
            ys.push((b.y0 + b.y1) / 2.0);
            all = Some(all.map_or(b, |a| a.union(b)));
        }
    }
    let (xs_raw, ys_raw) = (xs.clone(), ys.clone());
    xs.sort_by(f64::total_cmp);
    ys.sort_by(f64::total_cmp);
    let (cx, cy) = (xs[xs.len() / 2], ys[ys.len() / 2]);
    let all = all.unwrap();
    let (w, h) = (1600u32, 900u32);
    let fit = (f64::from(w) / all.width()).min(f64::from(h) / all.height());
    // The densest tile of the view's size (by sampled layer centres) at a zoom.
    let dense = |s: f64| {
        let (tw, th) = (f64::from(w) / s, f64::from(h) / s);
        let mut cells = std::collections::HashMap::<(i64, i64), usize>::new();
        for (x, y) in xs_raw.iter().zip(&ys_raw) {
            *cells.entry((((x - all.x0) / tw) as i64, ((y - all.y0) / th) as i64)).or_default() += 1;
        }
        let ((i, j), _) = cells.into_iter().max_by_key(|(_, n)| *n).unwrap();
        (all.x0 + i as f64 * tw, all.y0 + j as f64 * th)
    };
    let (d25, d100) = (dense(0.25), dense(1.0));
    let views = [
        ("fit", fit, all.x0, all.y0),
        ("25%", 0.25, cx - 800.0 / 0.25, cy - 450.0 / 0.25),
        ("25% dense", 0.25, d25.0, d25.1),
        ("100%", 1.0, cx - 800.0, cy - 450.0),
        ("100% dense", 1.0, d100.0, d100.1),
    ];
    let opts = Options { lod: true, background: None, ..Default::default() };
    println!("nodes {} fit {fit:.4} center {cx:.0},{cy:.0}", doc.node_count());
    for (name, scale, ox, oy) in views {
        let v = View { origin: (ox, oy), scale, width: w, height: h };
        let mut ts = Vec::new();
        let mut hashes = Vec::new();
        for _ in 0..6 {
            let t = Instant::now();
            let pix = render_view(&doc, page, &v, &opts).unwrap();
            ts.push(t.elapsed().as_secs_f64() * 1000.0);
            let mut hs = std::collections::hash_map::DefaultHasher::new();
            pix.data().hash(&mut hs);
            hashes.push(hs.finish());
            if hashes.len() == 1 {
                let _ = pix.save_png(format!(
                    "{}/bench-{}.png",
                    std::env::var("FIGCRAFT_BENCH_OUT").unwrap_or_else(|_| ".".into()),
                    name.replace('%', "pct").replace(' ', "-")
                ));
            }
        }
        let same = hashes.iter().all(|h| *h == hashes[0]);
        let first = ts[0];
        ts.remove(0);
        ts.sort_by(f64::total_cmp);
        println!(
            "{name}: first {first:.0} ms, later min {:.0} median {:.0} ms, hash {:x}, repeat-identical {same}",
            ts[0],
            ts[ts.len() / 2],
            hashes[0]
        );
    }
}
