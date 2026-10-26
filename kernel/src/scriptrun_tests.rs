//! `test=scriptrun` (P1 task 14, ADR 0001 D6): `scriptrun` started through a Job,
//! and spike S01-1's M3 and M5.
//!
//! - M3: the ring-3 `handles mode=job` driver starts `scriptrun` per request,
//!   with only a run-tier Sched handle and a result `Shm` moved in under a request
//!   node, waits for it, reads its result and revokes the node, 100 times. It logs
//!   p50 and p99; ADR 0001 passes p99 at most 30 ms and pools `scriptrun` between
//!   30 and 150 ms. Recorded here, not judged.
//! - M5: the kernel memory handle tables (at their first 256 entries) and grant
//!   nodes take at 16 processes, and at 64 for ADR 0003. ADR 0001 passes at most
//!   256 KiB at 16 and fails over 1 MiB; the case fails only past 1 MiB.

use crate::fpu_tests::wait_exit;
use crate::log;
use crate::process::{self, MAX_PROCESSES, SpawnRequest};
use kabi::handle::TABLE_START;
use kabi::{ExitReason, Limits, cap};

const LIMITS: Limits = Limits {
    max_pages: 1024,
    max_burst_ms: 30_000,
    caps: cap::SUPERVISE | cap::POLICY | cap::SCRIPT,
    stack_pages: 0,
};

pub fn check() -> Result<(), &'static str> {
    let pid = process::spawn(&SpawnRequest {
        name: "hjob",
        program: "handles",
        args: b"mode=job",
        limits: LIMITS,
        spawner: 0,
        supervisor: false,
        uses: &[],
    })
    .map_err(|_| "scriptrun: cannot start the driver")?;
    if wait_exit(pid)? != (ExitReason::Exited, 0) {
        return Err("scriptrun: the M3 driver failed");
    }
    let (at_16, at_64) = (m5_bytes(16), m5_bytes(64));
    let verdict = match at_16 {
        b if b <= 256 * 1024 => "pass",
        b if b <= 1024 * 1024 => "between pass and fail",
        _ => "fail",
    };
    log!(
        "scriptrun: M5: handle tables (256 entries each; {MAX_PROCESSES} process slots today) and {} grant nodes: {} KiB at 16 processes ({verdict}), {} KiB at 64",
        captable::grant::NODES,
        at_16 / 1024,
        at_64 / 1024
    );
    if at_16 > 1024 * 1024 {
        return Err("scriptrun: M5 is over 1 MiB at 16 processes");
    }
    log!(
        "scriptrun: a Job started scriptrun with only the handles moved in, and its exit came back"
    );
    Ok(())
}

/// Bytes the kernel holds for `processes` handle tables at their first 256
/// entries (each slot an entry, its generation and a free-list index), the grant
/// tree and its generation copies.
fn m5_bytes(processes: usize) -> usize {
    let slot = core::mem::size_of::<Option<captable::Entry>>()
        + core::mem::size_of::<u32>()
        + core::mem::size_of::<u16>();
    processes * TABLE_START * slot
        + core::mem::size_of::<captable::grant::Tree>()
        + captable::grant::NODES * core::mem::size_of::<u32>()
}
