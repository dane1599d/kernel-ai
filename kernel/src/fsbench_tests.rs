//! `test=fs-bench` (lane S-io, the kaifs re-baseline): kaifs end to end from
//! a ring-3 client, `filecheck step=bench`, which moves 32 MiB each way in
//! 7 KiB messages and then over a 1 MiB bulk `Shm`, and logs each run's MB/s
//! and its time waiting on `fs`. `fs` runs with `profile=phases`, so it logs
//! each phase's split (outside `fs`, PART_* calls, the rest), #262's
//! breakdown. The kernel only waits for `bench.ok`; xtask reads the lines.

use crate::files_tests::ask;
use crate::{log, threads};
use alloc::format;
use alloc::string::String;
use fsproto::{How, Reply, Request};

/// `fs` with its phase profile, and `filecheck` doing the bench in its home
/// folder: a 1 MiB piece, its `Shm` and the minicbor stack want more than the
/// default 4 MiB.
pub fn plan() -> String {
    format!(
        "{} profile=phases\nfilecheck caps=files stack_pages=64 pages=2048 step=bench",
        kabi::plan::FS
    )
}

/// Long enough for 64 MiB each way under TCG, at about 2 MB/s.
const WAIT_MS: u64 = 600_000;
const POLL_MS: u64 = 250;

pub fn check() -> Result<(), &'static str> {
    for _ in 0..WAIT_MS / POLL_MS {
        let open = Request::Open {
            path: "bench.ok".into(),
            rights: 0,
            how: How::Existing,
        };
        if matches!(ask(kabi::fs::HOME_USER, open), Ok(Reply::Opened(..))) {
            log!("fs-bench: filecheck moved 32 MiB each way, both ways");
            return Ok(());
        }
        threads::sleep_ms(POLL_MS);
    }
    Err("fs-bench: filecheck did not finish the bench")
}
