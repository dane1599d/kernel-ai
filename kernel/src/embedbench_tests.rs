//! `test=embed-bench` (spike S13-3, ADR 0013, not for merge): runs the
//! hard-float ring-3 `embedbench` program on the embedding model the
//! `kai-model` disk holds, which prints indexing tokens/s and a query's latency,
//! and passes when it exits 0 (the model loaded and every vector was right; the
//! timings are reported, not asserted).

use crate::process::{self, SpawnRequest, supervision};
use crate::{log, threads};
use kabi::{ExitReason, Limits, cap};

/// The engine's scratch and a 512-position KV cache (56 MiB of f16), the
/// 10,000 stored vectors and the tokenizer fit in 65,536 pages with room to
/// spare; the model is mapped, not counted.
const LIMITS: Limits = Limits {
    max_pages: 65536,
    max_burst_ms: 600_000,
    caps: cap::MODEL | cap::THREADS,
    stack_pages: 512,
};
const EXIT_TIMEOUT_MS: u64 = 900_000;

pub fn check() -> Result<(), &'static str> {
    let pid = process::spawn(&SpawnRequest {
        name: "embedbench",
        program: "embedbench",
        args: b"",
        limits: LIMITS,
        spawner: 0,
        supervisor: false,
        uses: &[],
    })
    .map_err(|error| {
        log!("embed-bench: cannot start: {error:?}");
        "embed-bench: cannot start the program"
    })?;
    for _ in 0..EXIT_TIMEOUT_MS / 100 {
        if let Some((reason, detail)) = supervision::kernel_child_exit(pid) {
            return match (reason, detail) {
                (ExitReason::Exited, 0) => Ok(()),
                _ => Err("embed-bench: the model did not load or a vector was wrong"),
            };
        }
        threads::sleep_ms(100);
    }
    Err("embed-bench: the program did not end")
}
