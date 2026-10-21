//! M10a-5, the M10 exit test's boot (`test=tune-ab`), run under icount by
//! `cargo xtask tune ab` on mix A at one CPU:
//!
//! - `phase=observe`: the shell's real `tune editor` flow once, as the founder
//!   would type it. The llm service traces the request (its summary is what a
//!   recording is made from), and answers it from an `llmreplay` disk when there
//!   is one, which only works if the guest's prompt is byte for byte the recorded
//!   one. A proposal is declined: nothing is measured here.
//! - `phase=measure tune_spec=classes/firm:build-1,build-2,...`: the recorded
//!   decision, checked by the same rules as SCHED_TRIAL, run by the same trial
//!   code (3 pairs of 3 s windows, the editor as the target) between
//!   `measure-start` and `measure-end`. Nothing prints while it runs; afterwards
//!   the windows, the editor's exact samples, the verdict, the guardrail counts
//!   and a digest of all of it, which an identical repeat must reproduce.

use crate::sched_control::trial::{self, Candidate, Request};
use crate::sched_control::{self, calls};
use crate::tune_tests::{finished, line, pid_named};
use crate::{log, threads};
use alloc::string::String;
use alloc::vec::Vec;
use kabi::model::Fnv1a;
use sched::spec::PolicySpec;

const WINDOW_MS: u64 = 3_000;
const PAIRS: usize = 3;
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
        Some("measure") => measure(cmdline, seed, editor),
        _ => Err("tune-ab: phase= must be observe or measure"),
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
    log!("tune-ab: observe done");
    Ok(())
}

fn measure(cmdline: &str, seed: &str, editor: u64) -> Result<(), &'static str> {
    let text = word(cmdline, "tune_spec").ok_or("tune-ab: measure needs tune_spec=")?;
    let (spec, names) = parse_spec(text).ok_or("tune-ab: tune_spec= is not a spec")?;
    let background = names
        .iter()
        .map(|name| pid_named(name))
        .collect::<Result<Vec<_>, _>>()?;
    calls::check(spec, editor, &background).map_err(|_| "tune-ab: the spec breaks the rules")?;
    let runaways = sched_control::runaways();
    log!("measure-start");
    let id = trial::start(Request {
        candidate: Candidate::Spec(spec),
        target_pid: editor,
        background,
        window_ns: WINDOW_MS * 1_000_000,
        pairs: PAIRS,
        starter_pid: 0,
    })
    .map_err(|_| "tune-ab: the kernel refused the trial")?;
    let status = finished(id)?;
    log!("measure-end");
    let mut digest = Fnv1a::new();
    let mut report = |text: String| {
        digest.update(text.as_bytes());
        log!("tune-ab: {text}");
    };
    report(alloc::format!(
        "seed {seed} spec {} labels {}",
        spec.label(),
        names.join(",")
    ));
    for (i, w) in status
        .windows
        .iter()
        .take(status.windows_done as usize)
        .enumerate()
    {
        let side = if w.candidate == 1 {
            "candidate"
        } else {
            "baseline"
        };
        report(alloc::format!(
            "window {i} {side} p99_us {} samples {} background_cpu_ns {} max_wait_us {} ejections {}",
            w.target_p99_ns / 1000,
            w.target_samples,
            w.background_cpu_ns,
            w.max_wait_ns / 1000,
            w.ejections
        ));
    }
    for (pair, p) in sched_control::last_pairs().iter().enumerate() {
        for (side, w) in [("baseline", &p.baseline), ("candidate", &p.candidate)] {
            for chunk in w.target_excess_ns.chunks(PER_LINE) {
                let us: Vec<String> = chunk
                    .iter()
                    .map(|ns| alloc::format!("{}", ns / 1000))
                    .collect();
                report(alloc::format!("samples {pair} {side} {}", us.join(" ")));
            }
        }
    }
    report(alloc::format!(
        "verdict state {} reason {} baseline_p99_us {} candidate_p99_us {} background_permille {} worst_permille {}",
        status.state,
        status.reason,
        status.baseline_p99_ns / 1000,
        status.candidate_p99_ns / 1000,
        status.background_permille,
        if status.worst_pid == 0 {
            1000
        } else {
            status.worst_permille
        }
    ));
    report(alloc::format!(
        "guard runaways {} quiet_bytes {}",
        sched_control::runaways() - runaways,
        trial::window_bytes()
    ));
    log!("tune-ab: digest {:016x}", digest.finish());
    Ok(())
}
