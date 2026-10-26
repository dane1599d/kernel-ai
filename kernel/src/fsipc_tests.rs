//! Spike S04-2 (ADR 0004), `test=fs-ipc`: `fs` over today's copied IPC. Starts the
//! ring-3 stub `fs` as a service and `fsbench` with one route to it, and passes
//! when both exit 0; the figures are `fsbench`'s lines. Never merged.

use crate::fpu_tests::wait_exit;
use crate::log;
use crate::process::{self, SpawnRequest};
use kabi::{ExitReason, Limits, cap};

/// The stub holds the 64 MiB file and two message buffers.
const FS_LIMITS: Limits = Limits {
    max_pages: 20_480,
    max_burst_ms: 600_000,
    caps: cap::SERVE,
    stack_pages: 0,
};

/// The client holds a 1 MiB pattern and two message buffers.
const BENCH_LIMITS: Limits = Limits {
    max_pages: 1_024,
    max_burst_ms: 600_000,
    caps: cap::CALL,
    stack_pages: 0,
};

pub fn check() -> Result<(), &'static str> {
    let start = |name, limits, uses: &'static [&'static str]| {
        process::spawn(&SpawnRequest {
            name,
            program: name,
            args: b"",
            limits,
            spawner: 0,
            supervisor: false,
            uses,
        })
        .map_err(|error| {
            log!("fs-ipc: cannot start {name}: {error:?}");
            "fs-ipc: cannot start the pair"
        })
    };
    let server = start("fs", FS_LIMITS, &[])?;
    let client = start("fsbench", BENCH_LIMITS, &["fs"])?;
    for pid in [client, server] {
        if wait_exit(pid)? != (ExitReason::Exited, 0) {
            return Err("fs-ipc: the stub or its client failed");
        }
    }
    log!("fs-ipc: the stub fs answered every call");
    Ok(())
}
