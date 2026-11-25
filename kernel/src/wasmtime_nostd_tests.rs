//! `test=wasmtime-nostd` (S14-2, spike/wasmtime-nostd, never merged): the ring-3
//! program `wasmtimespike` loads a Pulley `.cwasm` into Wasmtime's `no_std`
//! runtime, calls two exports and checks the answers (see its source). The
//! case checks it exited 0, and so that its line reached the console.

use crate::fpu_tests::wait_exit;
use crate::log;
use crate::process::{self, SpawnRequest};
use kabi::{ExitReason, Limits};

/// 64 MiB of pages: the engine, the deserialized module and one instance.
const LIMITS: Limits = Limits {
    max_pages: 16_384,
    max_burst_ms: 60_000,
    caps: 0,
    stack_pages: 0,
};

pub fn check() -> Result<(), &'static str> {
    let pid = process::spawn(&SpawnRequest {
        name: "wasmtimespike",
        program: "wasmtimespike",
        args: b"",
        limits: LIMITS,
        spawner: 0,
        supervisor: false,
        uses: &[],
    })
    .map_err(|_| "wasmtime-nostd: cannot start the program")?;
    let exit = wait_exit(pid)?;
    log!("wasmtime-nostd: the program ended {exit:?}");
    if exit != (ExitReason::Exited, 0) {
        return Err("wasmtime-nostd: the program did not exit 0");
    }
    Ok(())
}
