//! `test=tls-bench` (spike P5-S1 `tls-nostd`, not for merge): runs the
//! hard-float ring-3 `tlsbench` program, which prints the TLS handshake and
//! record figures, and passes when it exits 0 (every handshake and transfer
//! right; the timings are reported, not asserted).

use crate::process::{self, SpawnRequest, supervision};
use crate::{log, threads};
use kabi::{ExitReason, Limits};

/// Two 8 MiB transfer buffers, the in-flight records and both sides' state fit
/// in 16,384 pages.
const LIMITS: Limits = Limits {
    max_pages: 16384,
    max_burst_ms: 120_000,
    caps: 0,
    stack_pages: 512,
};
const EXIT_TIMEOUT_MS: u64 = 600_000;

pub fn check() -> Result<(), &'static str> {
    let pid = process::spawn(&SpawnRequest {
        name: "tlsbench",
        program: "tlsbench",
        args: b"",
        limits: LIMITS,
        spawner: 0,
        supervisor: false,
        uses: &[],
    })
    .map_err(|error| {
        log!("tls-bench: cannot start: {error:?}");
        "tls-bench: cannot start the program"
    })?;
    for _ in 0..EXIT_TIMEOUT_MS / 100 {
        if let Some((reason, detail)) = supervision::kernel_child_exit(pid) {
            return match (reason, detail) {
                (ExitReason::Exited, 0) => Ok(()),
                _ => Err("tls-bench: a result was wrong or the program stopped"),
            };
        }
        threads::sleep_ms(100);
    }
    Err("tls-bench: the program did not end")
}
