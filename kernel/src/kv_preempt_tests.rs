//! `test=kv-preempt`, spike S13-1's guest half (ADR 0013 section 2): what it costs,
//! with the real model on the `kai-model` disk, to snapshot a KV slot into RAM, to
//! restore it, and to spill it to the `kai-cache` partition and back, at 512, 1,024
//! and 1,950 positions (the slot's cap is 2,048, and the follow-up the
//! measurement asks of the restored slot needs a few dozen positions of its own),
//! and whether the restored slot answers as the live one did.
//!
//! llm does the work (`infer::preempt`, `kv-bench`, answered only when the plan line
//! says `kvbench=on`, which only this image's does): the prompts are computed by the
//! model in the guest, so the case takes minutes, and every figure is the guest's
//! own clock. This case waits for llm to have both prompts cached, asks for the
//! sizes, logs each line llm returns as `kv-preempt:`, and fails only if a restored
//! slot answered differently or a size could not be measured. The bars (a RAM
//! snapshot and restore of 1,024 positions in at most 50 ms, a spill round trip in at
//! most 1.5 s) are read from the lines. `cargo xtask kv-preempt` boots it, and
//! `tune-model.yml` runs that on a KVM runner (mode `kv-preempt`); never on a PC
//! without the founder's OK.
//!
//! The foreground request's typing-to-done figure with two background agents
//! decoding is not here: it needs the scheduler wired into llm, which comes after
//! the v0.1 tag.

use crate::{ipc, llm, log, model, threads};

/// The positions asked for; the third is what fits, with a margin, beside the
/// follow-up in a slot of 2,048.
const SIZES: &str = "512,1024,1950";
/// The spill variants llm measures after the sizes (`SPILL_VARIANTS` in
/// `user/src/bin/llm/local.rs`), each one more row.
const VARIANTS: usize = 4;
/// Extra pages for llm: room for the one snapshot of the largest size, taken once for
/// every size (281 MiB), and a margin of 71 MiB, beside the two KV slots. The holder's
/// ceiling is raised to hold them (`kabi::model::MAX_PAGES_HOLDER`).
pub const EXTRA_PAGES: u64 = (352 << 20) / 4096;
// What llm is granted with it must fit what its holder may hold.
const _: () = assert!(crate::model::holder_pages() + EXTRA_PAGES <= kabi::model::MAX_PAGES_HOLDER);
/// How long the services get to come up.
const START_WAIT_MS: u64 = 120_000;
/// How long llm gets to cache both prompts: about 2,000 tokens at the runners'
/// 26 tokens/s, with room.
const WARM_WAIT_MS: u64 = 600_000;
/// How often the warm wait asks.
const WARM_POLL_MS: u64 = 2_000;
/// How long the measurement may take: 3,500 prompt tokens at 10 tokens/s is under
/// 6 minutes, and each size answers seven follow-ups.
const BENCH_WAIT_MS: u64 = 3_600_000;

pub fn check() -> Result<(), &'static str> {
    if llm::target().is_some() {
        return Err("kv-preempt: the boot names an LLM server; this case runs without one");
    }
    if model::get().is_none() {
        return Err("kv-preempt: the kai-model disk was not loaded");
    }
    wait_every(START_WAIT_MS, 10, || ipc::server_of("llm").is_some())
        .map_err(|_| "kv-preempt: llm did not come up")?;
    let started = crate::hpet::nanos();
    wait_every(WARM_WAIT_MS, WARM_POLL_MS, llm::warm)
        .map_err(|_| "kv-preempt: llm never cached both prompts")?;
    log!(
        "kv-preempt: both prompts cached {} ms after llm came up",
        (crate::hpet::nanos() - started) / 1_000_000
    );
    log!("kv-preempt: ready: asking for {SIZES}");
    let report = llm::kv_bench(SIZES, BENCH_WAIT_MS).map_err(|why| {
        log!("kv-preempt: llm said: {why}");
        "kv-preempt: llm could not measure"
    })?;
    let mut rows = 0;
    let mut wrong = false;
    for line in report.lines() {
        log!("{line}");
        rows += usize::from(line.contains(" positions: prefill "));
        wrong |= line.contains("equal after restore:") && line.contains("false");
        wrong |= !line.contains(" positions: prefill ");
    }
    if rows != SIZES.split(',').count() + VARIANTS || wrong {
        return Err("kv-preempt: a size was not measured, or a restored slot answered differently");
    }
    log!("kv-preempt: done");
    Ok(())
}

/// Asks `done` every `every_ms` until it holds or about `ms` have passed.
fn wait_every(ms: u64, every_ms: u64, mut done: impl FnMut() -> bool) -> Result<(), ()> {
    for _ in 0..ms / every_ms {
        if done() {
            return Ok(());
        }
        threads::sleep_ms(every_ms);
    }
    Err(())
}
