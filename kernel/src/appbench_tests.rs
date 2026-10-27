//! `test=appbench` (lane C, ADR 0003 S03-1's deferred figures): runs the hard-float
//! ring-3 `appbench` program, which prints the view and layout figures, and passes
//! when it exits 0 (every result right; the timings are reported, not asserted).

use crate::process::{self, SpawnRequest, supervision};
use crate::{log, threads};
use kabi::{ExitReason, Limits};

/// comphost's plan line: 8,192 pages and 512 stack pages.
const LIMITS: Limits = Limits {
    max_pages: 8192,
    max_burst_ms: 120_000,
    caps: 0,
    stack_pages: 512,
};
const EXIT_TIMEOUT_MS: u64 = 600_000;

pub fn check() -> Result<(), &'static str> {
    let pid = process::spawn(&SpawnRequest {
        name: "appbench",
        program: "appbench",
        args: b"",
        limits: LIMITS,
        spawner: 0,
        supervisor: false,
        uses: &[],
    })
    .map_err(|error| {
        log!("appbench: cannot start: {error:?}");
        "appbench: cannot start the program"
    })?;
    for _ in 0..EXIT_TIMEOUT_MS / 100 {
        if let Some((reason, detail)) = supervision::kernel_child_exit(pid) {
            return match (reason, detail) {
                (ExitReason::Exited, 0) => Ok(()),
                _ => Err("appbench: a result was wrong or the program stopped"),
            };
        }
        threads::sleep_ms(100);
    }
    Err("appbench: the program did not end")
}
