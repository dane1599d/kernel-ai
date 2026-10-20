//! M10's workload on the kernel command line: `workload=A seed=N` adds the mix's
//! processes to the service plan as ring-3 `work` programs (see
//! `sched::workload`), scaled to the CPUs online and to the room the process
//! table leaves beside the standing services.

use crate::{process, smp};
use alloc::string::String;
use sched::workload::Mix;

/// Process slots kept for the supervisor, shell, llm and one more service.
const SERVICE_SLOTS: usize = 4;
/// The seed when the command line names none.
const DEFAULT_SEED: u64 = 1;

/// The mix and seed the command line asks for, if any.
pub fn requested(cmdline: &str) -> Option<(Mix, u64)> {
    let value = |key: &str| {
        cmdline
            .split_whitespace()
            .find_map(|word| word.strip_prefix(key)?.strip_prefix('='))
    };
    let mix = Mix::by_name(value("workload")?)?;
    let seed = value("seed")
        .and_then(|seed| seed.parse().ok())
        .unwrap_or(DEFAULT_SEED);
    Some((mix, seed))
}

/// The plan lines for the requested mix, one per line, or an empty string.
pub fn plan_lines() -> String {
    let Some((mix, seed)) = requested(crate::cmdline()) else {
        return String::new();
    };
    let room = process::MAX_PROCESSES - SERVICE_SLOTS;
    mix.plan_lines(seed, smp::online(), room).join("\n")
}
