//! Spike S04-1 (ADR 0004), `test=hashbench`: runs the ring-3 `hashbench` program,
//! built for the target `fs` ships on, and passes when it exits 0. Never merged.

use crate::process::{self, SpawnRequest, supervision};
use crate::{log, threads};
use kabi::{ExitReason, Limits};

const LIMITS: Limits = Limits {
    max_pages: 1024,
    max_burst_ms: 600_000,
    caps: 0,
    stack_pages: 0,
};

pub fn check() -> Result<(), &'static str> {
    let pid = process::spawn(&SpawnRequest {
        name: "hashbench",
        program: "hashbench",
        args: b"",
        limits: LIMITS,
        spawner: 0,
        supervisor: false,
        uses: &[],
    })
    .map_err(|error| {
        log!("hashbench: cannot start: {error:?}");
        "hashbench: cannot start the program"
    })?;
    for _ in 0..6000 {
        if let Some((reason, detail)) = supervision::kernel_child_exit(pid) {
            return match (reason, detail) {
                (ExitReason::Exited, 0) => Ok(()),
                _ => Err("hashbench: the program failed"),
            };
        }
        threads::sleep_ms(100);
    }
    Err("hashbench: the program did not end")
}
