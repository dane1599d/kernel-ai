//! P5-S1's bench on the host, for comparison with ring 3's `tlsbench`: the same
//! code and the same lines (`report::run`).

use std::process::ExitCode;
use std::sync::OnceLock;
use std::time::Instant;

fn now() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

fn main() -> ExitCode {
    let right = spike_tls::report::run("tls-host", now, &mut |line| println!("{line}"));
    if right {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
