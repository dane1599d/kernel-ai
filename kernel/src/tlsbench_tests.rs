//! `test=tls-bench` (P5 lane N-t, spike P5-S1): runs the hard-float ring-3
//! `tlsbench` program, which prints `kainet`'s TLS 1.3 figures (a full
//! handshake, the AEADs' speed and data through rustls's record layer, all
//! in memory), and passes when it exits 0 (every handshake and transfer
//! right; the timings are reported, not asserted).

use crate::process::{self, SpawnRequest, supervision};
use crate::{log, threads};
use kabi::{ExitReason, Limits};

/// 16 MiB through each AEAD and suite, a record at a time, fits in 8,192
/// pages.
const LIMITS: Limits = Limits {
    max_pages: 8192,
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
