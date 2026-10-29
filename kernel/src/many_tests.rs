//! `test=many-processes` (P3 task 4, ADR 0003 §6): the raised table limits. The
//! plan adds 40 idle processes beside the shell and llm; the supervisor starts
//! them all, and each stays up at once, which the old limit of 16 processes could
//! not hold. Each prints a line, so all 40 also ran in ring 3.

use crate::{log, process, threads};
use alloc::format;
use alloc::string::String;

const COUNT: usize = 40;
const START_WAIT_MS: u64 = 120_000;
/// Long enough that a process killed or restarted would show.
const HOLD_MS: u64 = 2_000;

/// The plan lines this case adds: `idle-0` to `idle-39`, each a sleeping
/// `spinner` with a small page limit.
pub fn plan() -> String {
    (0..COUNT)
        .map(|n| format!("\nidle-{n} program=spinner pages=64 idle=1"))
        .collect()
}

fn running() -> usize {
    (0..COUNT)
        .filter(|n| !process::named(&format!("idle-{n}")).is_empty())
        .count()
}

pub fn check() -> Result<(), &'static str> {
    let mut up = 0;
    for _ in 0..START_WAIT_MS / 50 {
        up = running();
        if up == COUNT {
            break;
        }
        threads::sleep_ms(50);
    }
    if up != COUNT {
        log!("many-processes: {up} of {COUNT} came up");
        return Err("many-processes: not every process came up");
    }
    let pids: alloc::vec::Vec<u64> = (0..COUNT)
        .flat_map(|n| process::named(&format!("idle-{n}")))
        .map(|(pid, _)| pid)
        .collect();
    threads::sleep_ms(HOLD_MS);
    let still: alloc::vec::Vec<u64> = (0..COUNT)
        .flat_map(|n| process::named(&format!("idle-{n}")))
        .map(|(pid, _)| pid)
        .collect();
    if still != pids {
        return Err("many-processes: a process ended or restarted while all were up");
    }
    if process::named("shell").is_empty() || process::named("llm").is_empty() {
        return Err("many-processes: the shell or llm is not running beside them");
    }
    log!("many-processes: {COUNT} idle processes ran at once beside the shell and llm");
    Ok(())
}
