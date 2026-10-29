//! `test=proc-usage` (for P4's Monitor app): METRICS gives each process the
//! pages counted against its quota and the CPU time its threads used. `busy`
//! spins and `rest` sleeps; over a second, `busy` used most of a CPU and `rest`
//! almost none, and both hold pages.

use crate::{log, metrics, threads};
use kabi::Metrics;

/// The plan lines this case adds.
pub const PLAN: &str = "busy program=spinner burst_ms=3600000\nrest program=spinner idle=1";
const WAIT_MS: u64 = 30_000;
const WINDOW_MS: u64 = 1_000;

/// `name`'s pages and CPU time in `m`.
fn usage(m: &Metrics, name: &str) -> Option<(u64, u64)> {
    m.processes[..(m.process_count as usize).min(m.processes.len())]
        .iter()
        .find(|p| p.name() == name)
        .map(|p| (p.pages, p.cpu_ns))
}

pub fn check() -> Result<(), &'static str> {
    let both = |m: &Metrics| usage(m, "busy").is_some() && usage(m, "rest").is_some();
    let mut tries = 0;
    while !both(&metrics::snapshot()) {
        tries += 1;
        if tries > WAIT_MS / 50 {
            return Err("proc-usage: busy and rest never both ran");
        }
        threads::sleep_ms(50);
    }
    // Let both settle into what they do.
    threads::sleep_ms(500);
    let before = metrics::snapshot();
    threads::sleep_ms(WINDOW_MS);
    let after = metrics::snapshot();
    let elapsed = after.uptime_ns.saturating_sub(before.uptime_ns).max(1);
    let share = |name: &str| -> Option<(u64, u64)> {
        let (_, was) = usage(&before, name)?;
        let (pages, now) = usage(&after, name)?;
        Some((pages, now.saturating_sub(was) * 1000 / elapsed))
    };
    let (Some((busy_pages, busy)), Some((rest_pages, rest))) = (share("busy"), share("rest"))
    else {
        return Err("proc-usage: a process ended mid-window");
    };
    log!(
        "proc-usage: busy {busy_pages} pages, {busy} per mille of a CPU; rest {rest_pages} pages, {rest} per mille"
    );
    if busy_pages == 0 || rest_pages == 0 {
        return Err("proc-usage: a process showed no pages");
    }
    if busy < 300 || rest > 100 {
        return Err("proc-usage: the CPU shares are not the spinner's and the sleeper's");
    }
    log!("proc-usage: each process's pages and CPU time came through METRICS");
    Ok(())
}
