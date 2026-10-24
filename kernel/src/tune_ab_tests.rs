//! M10a-5, the M10 exit test's boot (`test=tune-ab`), run under icount by
//! `cargo xtask tune ab` on mix A at one CPU:
//!
//! - `phase=observe`: the shell's real `tune editor` flow once, as the founder
//!   would type it. The llm service traces the request (its summary is what a
//!   recording is made from), and answers it from an `llmreplay` disk when there
//!   is one, which only works if the guest's prompt is byte for byte the recorded
//!   one. A proposal is declined: nothing is measured here.
//! - `phase=run tune_spec=default` or `tune_spec=classes/firm:build-1,...`: one
//!   side of a seed (the founder's rule of 2026-10-21): from the same point in a
//!   fresh boot, the default as it runs, or the recorded decision (checked by the
//!   same rules as SCHED_TRIAL) installed with its labels, then, once every
//!   workload process has printed its start-up line, 3 windows of 3 s
//!   measured as a trial's are, the editor as the target, between
//!   `measure-start` and `measure-end`. Nothing prints while it runs unless the
//!   watchdog ejects the policy; afterwards the windows, the editor's exact
//!   samples, each process's CPU, the ejections since the policy was put in
//!   place, the guardrail counts and a digest of all of it, which an identical repeat must reproduce.

use crate::sched_control::trial;
use crate::sched_control::{self, calls};
use crate::tune_tests::{line, pid_named};
use crate::{log, threads};
use alloc::string::String;
use alloc::vec::Vec;
use kabi::model::Fnv1a;
use sched::spec::PolicySpec;

const WINDOW_MS: u64 = 3_000;
/// Windows each side is measured for: as long as a trial's candidate side.
const WINDOWS: usize = 3;
/// How often to look whether the workload has started, and the longest wait.
const SETTLE_STEP_MS: u64 = 10;
const SETTLE_MAX_MS: u64 = 30_000;
/// Samples per report line.
const PER_LINE: usize = 40;

fn word<'a>(cmdline: &'a str, key: &str) -> Option<&'a str> {
    cmdline
        .split_whitespace()
        .find_map(|w| w.strip_prefix(key)?.strip_prefix('='))
}

/// `classes/firm:build-1,build-2` as a spec and the labelled names.
fn parse_spec(text: &str) -> Option<(PolicySpec, Vec<&str>)> {
    let (label, names) = text.split_once(':').unwrap_or((text, ""));
    let (policy, preset) = label.split_once('/').unwrap_or((label, "gentle"));
    let spec = PolicySpec::parse(policy, preset).ok()?;
    let names = names.split(',').filter(|n| !n.is_empty()).collect();
    Some((spec, names))
}

pub fn check(cmdline: &str) -> Result<(), &'static str> {
    let seed = word(cmdline, "seed").unwrap_or("1");
    crate::shell_tests::start()?;
    let editor = pid_named("editor")?;
    threads::sleep_ms(1_000);
    match word(cmdline, "phase") {
        Some("observe") => observe(seed),
        Some("run") => run(cmdline, seed, editor),
        _ => Err("tune-ab: phase= must be observe or run"),
    }
}

fn observe(seed: &str) -> Result<(), &'static str> {
    let reply = line("tune editor")?;
    log!(
        "tune-ab: observe seed {seed}: {}",
        reply.lines().next().unwrap_or("")
    );
    for card in reply.lines().skip(1) {
        log!("tune-ab: card {card}");
    }
    if let Some(rest) = reply.strip_prefix("confirm ") {
        let id = rest.split(' ').next().unwrap_or("");
        line(&alloc::format!("answer {id} no"))?;
    }
    let counts: alloc::vec::Vec<u64> = crate::timer::DIAG_COUNTS
        .iter()
        .map(|c| c.load(core::sync::atomic::Ordering::Relaxed))
        .collect();
    log!("tune-ab: diag APIC counts from tick 1000: {counts:?}");
    log!("tune-ab: observe done");
    Ok(())
}

fn run(cmdline: &str, seed: &str, editor: u64) -> Result<(), &'static str> {
    let text = word(cmdline, "tune_spec").ok_or("tune-ab: run needs tune_spec=")?;
    let (side, spec, names) = match text {
        "default" => ("default", PolicySpec::DEFAULT, Vec::new()),
        text => {
            let (spec, names) = parse_spec(text).ok_or("tune-ab: tune_spec= is not a spec")?;
            ("candidate", spec, names)
        }
    };
    let background = names
        .iter()
        .map(|name| pid_named(name))
        .collect::<Result<Vec<_>, _>>()?;
    calls::check(spec, editor, &background).map_err(|_| "tune-ab: the spec breaks the rules")?;
    let runaways = sched_control::runaways();
    let ejections = ejections_so_far();
    if side == "candidate" {
        sched_control::install(spec, background)
            .map_err(|_| "tune-ab: the candidate did not install")?;
    }
    let settle_ms = settle();
    log!("measure-start");
    let (windows, pids, quiet_bytes) = trial::measure_only(editor, WINDOW_MS * 1_000_000, WINDOWS)
        .ok_or("tune-ab: the measurement was interrupted")?;
    log!("measure-end");
    let ejected = ejections_so_far() - ejections;
    let mut digest = Fnv1a::new();
    let mut report = |text: String| {
        digest.update(text.as_bytes());
        log!("tune-ab: {text}");
    };
    report(alloc::format!(
        "seed {seed} side {side} spec {} labels {}",
        spec.label(),
        names.join(",")
    ));
    for (i, w) in windows.iter().enumerate() {
        report(alloc::format!(
            "window {i} p99_us {} samples {} background_cpu_ns {} max_wait_us {} ejections {}",
            w.p99() / 1000,
            w.target_excess_ns.len(),
            w.cpu_ns.iter().skip(1).sum::<u64>(),
            w.max_wait_ns.iter().copied().max().unwrap_or(0) / 1000,
            w.ejections
        ));
        for chunk in w.target_excess_ns.chunks(PER_LINE) {
            let us: Vec<String> = chunk
                .iter()
                .map(|ns| alloc::format!("{}", ns / 1000))
                .collect();
            report(alloc::format!("samples {i} {}", us.join(" ")));
        }
    }
    let span_ns: u64 = windows.iter().map(|w| w.to - w.from).sum();
    for (index, pid) in pids.iter().enumerate().skip(1) {
        let name = crate::process::name_of(*pid)
            .map_or(String::from("?"), |(n, _)| String::from(n.as_str()));
        let cpu: u64 = windows.iter().map(|w| w.cpu_ns[index]).sum();
        report(alloc::format!("cpu {name} {cpu} of {span_ns}"));
    }
    report(alloc::format!(
        "run ejections {ejected} settle_ms {settle_ms}"
    ));
    report(alloc::format!(
        "guard runaways {} quiet_bytes {quiet_bytes}",
        sched_control::runaways() - runaways
    ));
    log!("tune-ab: digest {:016x}", digest.finish());
    Ok(())
}

/// Watchdog ejections on every CPU since boot.
fn ejections_so_far() -> u64 {
    (0..crate::smp::online())
        .filter_map(|cpu| threads::with_core(cpu, |core, _| u64::from(core.ejections())))
        .sum()
}

/// Waits until the workload has started: every process but the protected
/// services has printed its start-up line (a `work` process prints once it has
/// calibrated), at most `SETTLE_MAX_MS`. Returns how long it waited.
fn settle() -> u64 {
    let mut workload = Vec::new();
    crate::process::services(|pid, name| {
        if !sched_control::PROTECTED.contains(&name.as_str()) {
            workload.push(pid);
        }
    });
    let mut waited = 0;
    while waited < SETTLE_MAX_MS && !workload.iter().all(|&pid| crate::process::has_written(pid)) {
        threads::sleep_ms(SETTLE_STEP_MS);
        waited += SETTLE_STEP_MS;
    }
    waited
}
