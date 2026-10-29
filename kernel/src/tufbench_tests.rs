//! `test=tuf-bench` (P4 lane U, spikes S08-3 and S11-1): runs the soft-float
//! ring-3 `tufbench` program, which prints `kaituf`'s verification and SHA-256
//! figures, and passes when it exits 0 (every verification right; the timings
//! are reported, not asserted).

use crate::process::{self, SpawnRequest, supervision};
use crate::{log, threads};
use kabi::{ExitReason, Limits};

/// 16 MiB of SHA-256 input and the parsed 400 KiB role fit in 8,192 pages.
const LIMITS: Limits = Limits {
    max_pages: 8192,
    max_burst_ms: 120_000,
    caps: 0,
    stack_pages: 512,
};
const EXIT_TIMEOUT_MS: u64 = 600_000;

pub fn check() -> Result<(), &'static str> {
    let pid = process::spawn(&SpawnRequest {
        name: "tufbench",
        program: "tufbench",
        args: b"",
        limits: LIMITS,
        spawner: 0,
        supervisor: false,
        uses: &[],
    })
    .map_err(|error| {
        log!("tuf-bench: cannot start: {error:?}");
        "tuf-bench: cannot start the program"
    })?;
    for _ in 0..EXIT_TIMEOUT_MS / 100 {
        if let Some((reason, detail)) = supervision::kernel_child_exit(pid) {
            return match (reason, detail) {
                (ExitReason::Exited, 0) => Ok(()),
                _ => Err("tuf-bench: a result was wrong or the program stopped"),
            };
        }
        threads::sleep_ms(100);
    }
    Err("tuf-bench: the program did not end")
}
