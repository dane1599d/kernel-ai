//! `cargo xtask tune ab [--spec SPEC | --decisions FILE] [--mix A|B] [--seeds K]
//! [--shift N] [--timeout SECS]`: the M10 exit test. On each of seeds 1 to K it
//! boots `test=tune-ab phase=run` twice under icount (single-threaded TCG, one
//! CPU, no NIC, a fresh variable store each boot), once on the default and once
//! on the decision, then repeats seed 1's two boots as the canary, applies
//! [`super::ab`]'s rule and writes `target/tune-ab/report-<mix>.json`. The boots
//! run one after another.
//!
//! The decision comes from `xtask/tune/decisions.json`, a provenance-checked
//! recording of the real model (`cargo xtask tune record`), unless `--spec`
//! names one by hand (`classes/firm:build-1,build-2,build-3,build-4`): such a
//! run exercises the harness but is not the M10 exit test, and says so.
//! Mix B is the control: it is reported, never gating.

use super::ab;
use super::boot::{Booter, split};
use crate::icount;
use std::path::PathBuf;

const USAGE: &str = "usage: cargo xtask tune ab [--spec SPEC | --decisions FILE] [--mix A|B] \
                     [--seeds K] [--shift N] [--timeout SECS]";
pub const DEFAULT_SEEDS: u64 = 5;

/// One icount boot of `cmdline`; its console lines.
pub fn icount_boot(booter: &Booter, cmdline: &str, shift: u32) -> Result<Vec<String>, String> {
    let firmware = booter.prepare(cmdline)?;
    let args = icount::measure_args(&firmware, shift);
    println!("xtask: icount boot: {cmdline}");
    let captured =
        icount::run_capture(&booter.qemu, &args, booter.target(), booter.options.timeout)?;
    if !captured.passed {
        return Err(format!("FAIL: the boot `{cmdline}` did not pass"));
    }
    if let Some(ms) = captured.measure_host_ms {
        println!("xtask: the measurement took {ms} ms of host time");
    }
    Ok(captured.lines)
}

/// `test`'s options for an icount boot: TCG unless named, which icount needs.
pub fn icount_flags(mut rest: Vec<String>) -> Vec<String> {
    if !rest.iter().any(|a| a == "--accel") {
        rest.extend(["--accel".to_string(), "tcg".to_string()]);
    }
    rest
}

pub fn main(args: &[String]) -> Result<(), String> {
    let (own, rest) = split(
        args,
        &["--spec", "--decisions", "--mix", "--seeds", "--shift"],
    );
    let value = |name: &str| own.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
    let mix = value("--mix").unwrap_or_else(|| String::from("A"));
    if !matches!(mix.as_str(), "A" | "B") {
        return Err(USAGE.into());
    }
    let seeds: u64 =
        value("--seeds").map_or(Ok(DEFAULT_SEEDS), |s| s.parse().map_err(|_| USAGE))?;
    let shift: u32 =
        value("--shift").map_or(Ok(icount::DEFAULT_SHIFT), |s| s.parse().map_err(|_| USAGE))?;
    if seeds == 0 || shift > icount::MAX_SHIFT {
        return Err(USAGE.into());
    }
    let booter = Booter::build(&icount_flags(rest))?;
    let (spec, recorded) = match value("--spec") {
        Some(spec) => (spec, false),
        None => {
            let path = value("--decisions").map_or_else(super::record::default_path, PathBuf::from);
            (super::record::verified_spec(&booter, &path, shift)?, true)
        }
    };
    if !recorded {
        println!(
            "xtask: a hand-written --spec: this exercises the harness; it is not the M10 exit test"
        );
    }
    let side = |seed: u64, tune_spec: &str| -> Result<ab::SideRun, String> {
        let cmdline =
            format!("test=tune-ab phase=run workload={mix} seed={seed} tune_spec={tune_spec}");
        ab::parse(&icount_boot(&booter, &cmdline, shift)?)
    };
    let pair = |seed: u64| -> Result<(ab::SideRun, ab::SideRun), String> {
        Ok((side(seed, "default")?, side(seed, &spec)?))
    };
    let seeds = (1..=seeds)
        .map(|seed| pair(seed).map(|(d, c)| ab::judge(d, c)))
        .collect::<Result<Vec<_>, _>>()?;
    let canary = Some(pair(1)?);
    let report = ab::evaluate(seeds, canary);
    let dir = booter.target().join("tune-ab");
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let path = dir.join(format!("report-{mix}.json"));
    let text = serde_json::to_string_pretty(&report.json()).unwrap_or_default();
    std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    println!(
        "xtask: tune ab, mix {mix}, {} seeds and a canary; report at {}",
        report.seeds.len(),
        path.display()
    );
    for seed in &report.seeds {
        println!("xtask:   {}", seed.line());
    }
    for failure in &report.failures {
        println!("xtask:   {failure}");
    }
    match (report.pass, mix.as_str(), recorded) {
        (_, "B", _) => {
            println!("xtask: mix B is the control: reported, not gating");
            Ok(())
        }
        (true, _, true) => {
            println!("xtask: PASS: the M10 exit test, on the recorded decision");
            Ok(())
        }
        (true, _, false) => {
            println!("xtask: PASS on a hand-written spec; M10 is not declared done by this run");
            Ok(())
        }
        (false, _, _) => Err("FAIL: the exit test's rule did not hold".into()),
    }
}
