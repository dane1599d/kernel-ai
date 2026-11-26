//! Compares two directories of page renders, `page-<n>.png` from
//! hayro-render and `<prefix>-<n>.png` from pdftoppm (n zero-padded or not),
//! page by page. A page's pixels are compared over the area both share; a
//! pixel differs when any channel is more than 48 apart (anti-aliasing and
//! hinting differ between renderers, so a small threshold would count every
//! glyph edge). One line a page, then a summary:
//! `summary <pages both> <only hayro> <only poppler> <size mismatches>
//!  <mean MAE> <mean % differing> <worst % differing>`.

use std::collections::BTreeMap;
use std::path::Path;

/// Each `<anything>-<n>.png` in `dir`, by `n`.
fn pages(dir: &Path) -> BTreeMap<u32, std::path::PathBuf> {
    let mut found = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if path.extension().and_then(|e| e.to_str()) != Some("png") {
            continue;
        }
        if let Some(n) = stem.rsplit('-').next().and_then(|n| n.parse().ok()) {
            found.insert(n, path);
        }
    }
    found
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, ours, theirs] = &args[..] else {
        eprintln!("usage: pdfdiff <hayro-dir> <pdftoppm-dir>");
        std::process::exit(2);
    };
    let (ours, theirs) = (pages(Path::new(ours)), pages(Path::new(theirs)));
    let (mut both, mut mismatched) = (0u32, 0u32);
    let (mut mae_sum, mut pct_sum, mut worst) = (0f64, 0f64, 0f64);
    for (n, a) in &ours {
        let Some(b) = theirs.get(n) else { continue };
        let (Ok(a), Ok(b)) = (image::open(a), image::open(b)) else {
            println!("page {n} unreadable");
            continue;
        };
        let (a, b) = (a.to_rgb8(), b.to_rgb8());
        if a.dimensions() != b.dimensions() {
            mismatched += 1;
        }
        let (w, h) = (a.width().min(b.width()), a.height().min(b.height()));
        let (mut abs, mut differing) = (0u64, 0u64);
        for y in 0..h {
            for x in 0..w {
                let (p, q) = (a.get_pixel(x, y).0, b.get_pixel(x, y).0);
                let mut most = 0u8;
                for c in 0..3 {
                    let d = p[c].abs_diff(q[c]);
                    abs += u64::from(d);
                    most = most.max(d);
                }
                differing += u64::from(most > 48);
            }
        }
        let count = f64::from(w) * f64::from(h);
        let mae = abs as f64 / (count * 3.0).max(1.0);
        let pct = 100.0 * differing as f64 / count.max(1.0);
        println!(
            "page {n} {}x{} vs {}x{} mae {mae:.2} differing {pct:.2}%",
            a.width(),
            a.height(),
            b.width(),
            b.height()
        );
        both += 1;
        mae_sum += mae;
        pct_sum += pct;
        worst = worst.max(pct);
    }
    let only_ours = ours.keys().filter(|n| !theirs.contains_key(n)).count();
    let only_theirs = theirs.keys().filter(|n| !ours.contains_key(n)).count();
    let pages = f64::from(both.max(1));
    println!(
        "summary {both} {only_ours} {only_theirs} {mismatched} {:.2} {:.2} {worst:.2}",
        mae_sum / pages,
        pct_sum / pages
    );
}
