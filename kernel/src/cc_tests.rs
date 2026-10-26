//! Spike S12-1 (ADR 0012), `test=ccspike`: starts the ring-3 `ccspike` program with
//! `cc`'s planned limits (96 MiB, a 5 s burst raised for the Rhai loops) and passes
//! when it exits 0. The program prints the measurements; `repeats=N` and `rhai=0`
//! on the command line pass through to it. Never merged.

use crate::process::{self, SpawnRequest, supervision};
use crate::{log, threads};
use kabi::{ExitReason, Limits};

/// `cc caps=serve pages=24576 stack_pages=512`; the burst covers the Rhai loops,
/// which make no blocking call for many seconds under TCG.
const LIMITS: Limits = Limits {
    max_pages: 24_576,
    max_burst_ms: 900_000,
    caps: 0,
    stack_pages: 512,
};
const EXIT_TIMEOUT_MS: u64 = 1_500_000;

pub fn check(cmdline: &str) -> Result<(), &'static str> {
    let args: alloc::vec::Vec<&str> = cmdline
        .split_whitespace()
        .filter(|word| word.starts_with("repeats=") || word.starts_with("rhai="))
        .collect();
    let args = args.join(" ");
    let pid = process::spawn(&SpawnRequest {
        name: "ccspike",
        program: "ccspike",
        args: args.as_bytes(),
        limits: LIMITS,
        spawner: 0,
        supervisor: false,
        uses: &[],
    })
    .map_err(|error| {
        log!("ccspike: cannot start: {error:?}");
        "ccspike: cannot start the program"
    })?;
    for _ in 0..EXIT_TIMEOUT_MS / 100 {
        if let Some((reason, detail)) = supervision::kernel_child_exit(pid) {
            log!("ccspike: exited {reason:?} {detail}");
            return match (reason, detail) {
                (ExitReason::Exited, 0) => Ok(()),
                _ => Err("ccspike: a check failed or the program stopped"),
            };
        }
        threads::sleep_ms(100);
    }
    Err("ccspike: the program did not end")
}
