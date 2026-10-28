//! Before/after check for import speed work: `FIGCRAFT_FIG_SNAPSHOT=<file.fig> cargo test --release
//! -p figcraft-format import_snapshot -- --ignored --nocapture` prints node counts per page and a
//! SHA-256 of the document JSON, which must not change.

use sha2::{Digest, Sha256};

struct HashWriter(Sha256);

impl std::io::Write for HashWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.update(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
#[ignore = "needs FIGCRAFT_FIG_SNAPSHOT=<path to a large .fig file>"]
fn import_snapshot() {
    let Ok(path) = std::env::var("FIGCRAFT_FIG_SNAPSHOT") else { return };
    let bytes = std::fs::read(&path).unwrap();
    // FIGCRAFT_FIG_STAGED=1: through the page-first path, which must end with the same document.
    let started = std::time::Instant::now();
    let staged = std::env::var("FIGCRAFT_FIG_STAGED").is_ok_and(|v| v == "1");
    let (doc, warnings) = if staged {
        let mut first = |doc: figcraft_doc::Document| {
            println!(
                "SNAPSHOT first_page_at={:.2}s pages={} nodes={} resident_kb={}",
                started.elapsed().as_secs_f64(),
                doc.pages.len(),
                doc.node_count(),
                resident_kb()
            );
        };
        super::import_staged(&bytes, "snapshot", super::PageFirst::ALWAYS, &mut first).unwrap()
    } else {
        super::import(&bytes, "snapshot").unwrap()
    };
    println!("SNAPSHOT whole_at={:.2}s", started.elapsed().as_secs_f64());
    println!("SNAPSHOT resident_after_import_kb={}", resident_kb());
    let mut w = HashWriter(Sha256::new());
    crate::write_json(&doc, &mut w).unwrap();
    let hash: String = w.0.finalize().iter().map(|b| format!("{b:02x}")).collect();
    println!("SNAPSHOT pages={} warnings={warnings:?}", doc.pages.len());
    println!("SNAPSHOT json_sha256={hash}");
}

/// Working set of this process in KB (Windows `tasklist`; 0 elsewhere or on failure).
fn resident_kb() -> u64 {
    let pid = std::process::id();
    let Ok(out) = std::process::Command::new("tasklist").args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"]).output() else {
        return 0;
    };
    let s = String::from_utf8_lossy(&out.stdout);
    let digits: String = s.rsplit("\",\"").next().unwrap_or("").chars().filter(|c| c.is_ascii_digit()).collect();
    digits.parse().unwrap_or(0)
}
