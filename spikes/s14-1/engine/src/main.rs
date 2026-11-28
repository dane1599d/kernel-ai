//! `s14-page [fixture] [runs]`: renders each fixture (or the one named) `runs`
//! times (5 by default) and prints one line a run. The same program runs
//! natively and as a WASI guest, so the runner compares like with like.

fn main() {
    let mut args = std::env::args().skip(1);
    let only = args.next().filter(|a| a != "all");
    let runs: u32 = args.next().and_then(|r| r.parse().ok()).unwrap_or(5);
    for (name, html) in s14_engine::fixtures::all() {
        if only.as_deref().is_some_and(|o| o != name) {
            continue;
        }
        for run in 0..runs {
            let r = s14_engine::render(&html);
            println!(
                "s14 {name} run={run} bytes={} parse_us={} layout_us={} paint_us={} total_us={} pixels={:016x}",
                html.len(),
                r.parse_us,
                r.layout_us,
                r.paint_us,
                r.total_us,
                r.pixels
            );
        }
    }
}
