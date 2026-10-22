//! `test=latency`, spike S02-2's baseline (ADR 0002, P1 plan section 5): how long
//! plain requests take on today's script path, with the model on the `kai-model`
//! disk run by the ring-3 llm service. The harness (`cargo xtask latency`) types
//! one request at each `ready for keys`; for each, the kernel logs, by its own
//! clock, Enter to the busy sign's first paint and Enter to the reply on screen.
//! The first request is typed while llm may still be caching the script prompt,
//! so it is reported apart. Nothing here passes or fails on the numbers: this is
//! a measurement, and the marks (100 ms to the busy sign; 2-4 s to done) are read
//! from its output.

use crate::{console, ipc, llm, log, model, terminal, threads};
use alloc::vec::Vec;

/// How many requests the harness types.
pub const REQUESTS: usize = 5;
/// How long the services get to come up.
const START_WAIT_MS: u64 = 120_000;
/// How long one request may take: two rounds of the shell's 70 s, with room.
const REQUEST_WAIT_MS: u64 = 150_000;

/// One request's times, in microseconds after Enter.
#[derive(Clone, Copy)]
struct Times {
    sign_us: Option<u64>,
    done_us: u64,
}

pub fn check() -> Result<(), &'static str> {
    if llm::target().is_some() {
        return Err("latency: the boot names an LLM server; this case runs without one");
    }
    if model::get().is_none() {
        return Err("latency: the kai-model disk was not loaded");
    }
    wait_within(START_WAIT_MS, || {
        ipc::server_of("llm").is_some() && ipc::server_of("shell").is_some()
    })
    .map_err(|_| "latency: llm and the shell did not come up")?;
    threads::spawn("terminal", terminal::run, 0)
        .map_err(|_| "latency: cannot start the terminal")?;
    threads::sleep_ms(200);
    let mut times = Vec::with_capacity(REQUESTS);
    for n in 1..=REQUESTS {
        let shown = terminal::replies_shown();
        log!("latency: ready for keys");
        wait_within(REQUEST_WAIT_MS, || terminal::replies_shown() > shown)
            .map_err(|_| "latency: a typed request was never answered")?;
        let (entered, done) = terminal::last_times();
        let sign = console::status::sign_painted_ns();
        let t = Times {
            sign_us: (sign >= entered && sign <= done).then(|| (sign - entered) / 1000),
            done_us: done.saturating_sub(entered) / 1000,
        };
        log!(
            "latency: request {n}: busy sign {}, done {}",
            sign_text(t.sign_us),
            ms(t.done_us)
        );
        times.push(t);
    }
    report(&times);
    let health = llm::health().unwrap_or_default();
    log!("latency: health: {health}");
    log!("latency: done");
    Ok(())
}

/// The summary lines: the first request, then the median of the rest.
fn report(times: &[Times]) {
    let Some((first, rest)) = times.split_first() else {
        return;
    };
    log!(
        "latency: first request (cold): busy sign {}, done {}",
        sign_text(first.sign_us),
        ms(first.done_us)
    );
    let signs: Vec<u64> = rest.iter().filter_map(|t| t.sign_us).collect();
    let dones: Vec<u64> = rest.iter().map(|t| t.done_us).collect();
    log!(
        "latency: median of requests 2-{}: busy sign {} ({} of {} painted), done {}",
        times.len(),
        sign_text(median(&signs)),
        signs.len(),
        rest.len(),
        median(&dones).map_or_else(|| "-".into(), ms)
    );
}

fn median(values: &[u64]) -> Option<u64> {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    match sorted.len() {
        0 => None,
        n if n % 2 == 1 => Some(sorted[n / 2]),
        n => Some((sorted[n / 2 - 1] + sorted[n / 2]) / 2),
    }
}

/// Microseconds as milliseconds with one decimal: `123.4 ms`.
fn ms(us: u64) -> alloc::string::String {
    alloc::format!("{}.{} ms", us / 1000, us % 1000 / 100)
}

fn sign_text(us: Option<u64>) -> alloc::string::String {
    us.map_or_else(|| "not painted".into(), ms)
}

fn wait_within(ms: u64, mut done: impl FnMut() -> bool) -> Result<(), ()> {
    for _ in 0..ms / 10 {
        if done() {
            return Ok(());
        }
        threads::sleep_ms(10);
    }
    Err(())
}
