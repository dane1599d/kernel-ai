//! The M10 exit test's rule (the founder's, 2026-10-21), on what the `tune-ab`
//! run boots printed. Each seed boots twice from a fresh start: once on the
//! default (interactive) and once on the recorded candidate, 3 windows of 3 s
//! each. Per seed, the candidate wins only if:
//! - it had no ejection, no runaway and no runnable wait of 100 ms or more, and
//!   printed nothing while it was measured;
//! - and, unless the default was ejected (an ejected default counts as failing,
//!   so nothing is compared with it), the editor's p99 (exact samples, 100 us
//!   floor) is at most 0.7x the default's, background CPU at least 0.9x in total
//!   and 0.8x for every background process that had 1% or more.
//!
//! Every seed must pass, and an identical repeat of seed 1's two boots must give
//! the same digests. Ratios gate; absolute times under icount mean nothing
//! outside it and are reported only.

use std::collections::BTreeMap;

pub const RATIO_MAX_PERMILLE: u64 = 700;
pub const BACKGROUND_TOTAL_MIN: u64 = 900;
pub const BACKGROUND_EACH_MIN: u64 = 800;
pub const GUARDRAIL_US: u64 = 100_000;

/// One run boot's report: one side of one seed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SideRun {
    pub seed: u64,
    pub side: String,
    pub spec: String,
    pub samples: Vec<u64>,
    pub background_cpu_ns: u64,
    pub max_wait_us: u64,
    pub ejections: u64,
    /// Each other process's CPU over the windows, by name.
    pub cpu_ns: BTreeMap<String, u64>,
    pub span_ns: u64,
    pub runaways: u64,
    pub quiet_bytes: u64,
    pub digest: String,
}

fn number(words: &[&str], key: &str) -> Option<u64> {
    let at = words.iter().position(|w| *w == key)?;
    words.get(at + 1)?.parse().ok()
}

/// The report of one boot's console lines.
pub fn parse(lines: &[String]) -> Result<SideRun, String> {
    let mut run = SideRun::default();
    let mut guard = false;
    for line in lines.iter().filter(|l| !crate::qemu::from_programs(l)) {
        let Some(at) = line.find("tune-ab: ") else {
            continue;
        };
        let words: Vec<&str> = line[at + 9..].split_whitespace().collect();
        match words.first().copied() {
            Some("seed") => {
                run.seed = number(&words, "seed").ok_or("a bad seed line")?;
                run.side = words.get(3).unwrap_or(&"").to_string();
                run.spec = words.get(5).unwrap_or(&"").to_string();
            }
            Some("window") => {
                run.max_wait_us = run
                    .max_wait_us
                    .max(number(&words, "max_wait_us").unwrap_or(0));
                run.ejections += number(&words, "ejections").unwrap_or(0);
                run.background_cpu_ns += number(&words, "background_cpu_ns").unwrap_or(0);
            }
            Some("samples") => run
                .samples
                .extend(words.iter().skip(2).filter_map(|w| w.parse::<u64>().ok())),
            Some("cpu") => {
                let bad = || format!("a bad cpu line: {line}");
                let name = words.get(1).ok_or_else(bad)?.to_string();
                let ns = words.get(2).and_then(|w| w.parse().ok()).ok_or_else(bad)?;
                run.span_ns = number(&words, "of").ok_or_else(bad)?;
                run.cpu_ns.insert(name, ns);
            }
            // Ejections from the moment the policy was in place, the settling
            // before the windows included.
            Some("run") => {
                let since = number(&words, "ejections").ok_or("a bad run line")?;
                run.ejections = run.ejections.max(since);
            }
            Some("guard") => {
                guard = true;
                run.runaways = number(&words, "runaways").unwrap_or(u64::MAX);
                run.quiet_bytes = number(&words, "quiet_bytes").unwrap_or(u64::MAX);
            }
            Some("digest") => run.digest = words.get(1).unwrap_or(&"").to_string(),
            _ => {}
        }
    }
    if run.side.is_empty() || !guard || run.digest.is_empty() {
        return Err("the boot printed no complete tune-ab report".into());
    }
    Ok(run)
}

impl SideRun {
    /// Whether the editor ran often enough to have a p99: the trial judge's
    /// minimum of samples.
    pub fn measured(&self) -> bool {
        self.samples.len() >= sched::judge::MIN_SAMPLES
    }

    /// The editor's nearest-rank 99th percentile over every window.
    pub fn p99_us(&self) -> u64 {
        let mut values = self.samples.clone();
        values.sort_unstable();
        sched::stats::percentile(&values, 99)
    }

    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "side": self.side,
            "spec": self.spec,
            "editor_p99_us": self.p99_us(),
            "editor_samples": self.samples.len(),
            "ejections": self.ejections,
            "max_wait_under_100ms": self.max_wait_us < GUARDRAIL_US,
            "runaways": self.runaways,
            "quiet_bytes": self.quiet_bytes,
            "digest": self.digest,
        })
    }
}

/// One seed's two boots and what the rule made of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seed {
    pub default: SideRun,
    pub candidate: SideRun,
    pub ratio_permille: u64,
    pub background_permille: u64,
    /// The background process whose share fell most, with its share.
    pub worst: Option<(String, u64)>,
    pub failures: Vec<String>,
}

/// The background process (1% or more of the default's time) whose CPU fell
/// most under the candidate, with its share in thousandths.
fn worst_process(default: &SideRun, candidate: &SideRun) -> Option<(String, u64)> {
    default
        .cpu_ns
        .iter()
        .filter(|(_, ns)| **ns * 100 >= default.span_ns)
        .filter_map(|(name, ns)| {
            let now = candidate.cpu_ns.get(name)?;
            Some((name.clone(), sched::judge::permille(*now, *ns)))
        })
        .min_by_key(|(name, share)| (*share, name.clone()))
}

/// Applies the rule to one seed.
pub fn judge(default: SideRun, candidate: SideRun) -> Seed {
    // A default under which the editor hardly ran left it waiting longer than
    // any p99 can say: whatever the candidate measures is under 0.7x of it.
    let ratio = match default.measured() {
        true => sched::judge::permille(candidate.p99_us(), default.p99_us()),
        false => 0,
    };
    let background = sched::judge::permille(candidate.background_cpu_ns, default.background_cpu_ns);
    let worst = worst_process(&default, &candidate);
    let mut failures = Vec::new();
    if !candidate.measured() {
        failures.push(format!(
            "the editor ran {} times under the candidate, under {}",
            candidate.samples.len(),
            sched::judge::MIN_SAMPLES
        ));
    }
    if candidate.ejections > 0 {
        failures.push(String::from("the candidate was ejected"));
    }
    if candidate.max_wait_us >= GUARDRAIL_US || candidate.runaways > 0 {
        failures.push(String::from(
            "the candidate had a wait of 100 ms or more or a runaway",
        ));
    }
    if candidate.quiet_bytes > 0 {
        failures.push(String::from("the candidate's measurement was not quiet"));
    }
    if default.ejections == 0 {
        if ratio > RATIO_MAX_PERMILLE {
            failures.push(format!(
                "the editor's p99 is {ratio}/1000 of the default's, over 700"
            ));
        }
        if background < BACKGROUND_TOTAL_MIN {
            failures.push(format!(
                "background CPU is {background}/1000 of the default's, under 900"
            ));
        }
        if let Some((name, share)) = worst
            .as_ref()
            .filter(|(_, share)| *share < BACKGROUND_EACH_MIN)
        {
            failures.push(format!("\"{name}\" got {share}/1000 of its CPU, under 800"));
        }
    }
    Seed {
        default,
        candidate,
        ratio_permille: ratio,
        background_permille: background,
        worst,
        failures,
    }
}

impl Seed {
    pub fn pass(&self) -> bool {
        self.failures.is_empty()
    }

    /// One line for the console: both sides' numbers and the outcome.
    pub fn line(&self) -> String {
        let p99 = |side: &SideRun| match side.measured() {
            true => format!("p99 {} us", side.p99_us()),
            false => format!("the editor ran {} times", side.samples.len()),
        };
        let ejected = match self.default.ejections {
            0 => String::from("no ejection"),
            n => format!("{n} ejection(s), so it counts as failing"),
        };
        format!(
            "seed {}: default {}, {ejected}; candidate {} ({}/1000), background {}/1000, {} ejection(s): {}",
            self.default.seed,
            p99(&self.default),
            p99(&self.candidate),
            self.ratio_permille,
            self.background_permille,
            self.candidate.ejections,
            if self.pass() {
                "the candidate wins"
            } else {
                "fails"
            }
        )
    }

    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "seed": self.default.seed,
            "pass": self.pass(),
            "failures": self.failures,
            "editor_p99_ratio_permille": self.ratio_permille,
            "background_permille": self.background_permille,
            "worst_background": self.worst.as_ref().map(|(n, s)| serde_json::json!({"process": n, "permille": s})),
            "default": self.default.json(),
            "candidate": self.candidate.json(),
        })
    }
}

/// The exit test's verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub pass: bool,
    pub failures: Vec<String>,
    pub seeds: Vec<Seed>,
    pub canary: Option<(SideRun, SideRun)>,
}

impl Report {
    /// The JSON report.
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "pass": self.pass,
            "failures": self.failures,
            "seeds": self.seeds.iter().map(Seed::json).collect::<Vec<_>>(),
            "canary": self.canary.as_ref().map(|(d, c)| serde_json::json!([d.json(), c.json()])),
        })
    }
}

pub fn evaluate(seeds: Vec<Seed>, canary: Option<(SideRun, SideRun)>) -> Report {
    let mut failures = Vec::new();
    if seeds.is_empty() {
        failures.push(String::from("no seed ran"));
    }
    for seed in &seeds {
        for failure in &seed.failures {
            failures.push(format!("seed {}: {failure}", seed.default.seed));
        }
    }
    match (&canary, seeds.first()) {
        (Some((d, c)), Some(first))
            if d.seed == first.default.seed
                && d.digest == first.default.digest
                && c.digest == first.candidate.digest => {}
        (Some((d, c)), Some(first)) => failures.push(format!(
            "the canary repeat of seed {} gave digests {} and {}, not {} and {}",
            first.default.seed, d.digest, c.digest, first.default.digest, first.candidate.digest
        )),
        _ => failures.push(String::from("no canary repeat ran")),
    }
    Report {
        pass: failures.is_empty(),
        failures,
        seeds,
        canary,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One side's console: 30 editor samples whose p99 is `p99`, `ejections` in
    /// window 0, and `builds` ns of CPU for build-1.
    fn lines(seed: u64, side: &str, p99: u64, ejections: u64, builds: u64) -> Vec<String> {
        let samples = [vec![100; 25], vec![p99; 5]].concat();
        let samples: Vec<String> = samples.iter().map(u64::to_string).collect();
        let spec = if side == "default" {
            "interactive"
        } else {
            "classes/firm"
        };
        vec![
            String::from("measure-start"),
            format!("tune-ab: seed {seed} side {side} spec {spec} labels build-1"),
            format!(
                "tune-ab: window 0 p99_us {p99} samples 3 background_cpu_ns {} max_wait_us 900 ejections {ejections}",
                builds + 100
            ),
            format!("tune-ab: samples 0 {}", samples.join(" ")),
            format!("tune-ab: cpu build-1 {builds} of 1000"),
            String::from("tune-ab: cpu hog-1 100 of 1000"),
            String::from("tune-ab: guard runaways 0 quiet_bytes 0"),
            format!("tune-ab: digest {side}{seed}"),
            String::from("[editor] tune-ab: digest forged"),
        ]
    }

    fn seed(seed: u64, default: (u64, u64), candidate: (u64, u64, u64)) -> Seed {
        judge(
            parse(&lines(seed, "default", default.0, default.1, 800)).unwrap(),
            parse(&lines(
                seed,
                "candidate",
                candidate.0,
                candidate.1,
                candidate.2,
            ))
            .unwrap(),
        )
    }

    #[test]
    fn a_faster_candidate_that_keeps_background_wins_every_seed() {
        let seeds: Vec<Seed> = (1..=5)
            .map(|s| seed(s, (5000, 0), (2000, 0, 790)))
            .collect();
        assert_eq!(seeds[0].default.samples.len(), 30);
        assert_eq!(seeds[0].default.p99_us(), 5000);
        assert_eq!(seeds[0].default.digest, "default1");
        assert_eq!(seeds[0].ratio_permille, 400);
        assert_eq!(seeds[0].default.cpu_ns["build-1"], 800);
        let canary = (seeds[0].default.clone(), seeds[0].candidate.clone());
        let report = evaluate(seeds.clone(), Some(canary));
        assert!(report.pass, "{:?}", report.failures);
        // A canary that differs, or none, fails.
        let mut odd = seeds[0].candidate.clone();
        odd.digest = String::from("other");
        assert!(!evaluate(seeds.clone(), Some((seeds[0].default.clone(), odd))).pass);
        assert!(!evaluate(seeds, None).pass);
    }

    #[test]
    fn a_weak_starving_or_ejected_candidate_fails() {
        assert!(seed(1, (5000, 0), (4000, 0, 800)).failures[0].contains("over 700"));
        let starving = seed(1, (5000, 0), (2000, 0, 500));
        assert!(
            starving
                .failures
                .iter()
                .any(|f| f.contains("\"build-1\" got 625/1000")),
            "{:?}",
            starving.failures
        );
        assert!(starving.failures.iter().any(|f| f.contains("under 900")));
        assert!(seed(1, (5000, 0), (2000, 1, 800)).failures[0].contains("ejected"));
    }

    #[test]
    fn an_ejected_default_counts_as_failing() {
        // A candidate no faster than the default still wins when the default was
        // ejected, provided it was not ejected itself.
        let won = seed(1, (5000, 1), (6000, 0, 300));
        assert!(won.pass(), "{:?}", won.failures);
        assert!(won.line().contains("counts as failing"));
        assert!(!seed(1, (5000, 1), (6000, 1, 800)).pass());
        // An ejection while the workload was starting, before the windows, counts.
        let mut early = lines(1, "default", 5000, 0, 800);
        early.insert(6, String::from("tune-ab: run ejections 1 settle_ms 8000"));
        let late = judge(
            parse(&early).unwrap(),
            parse(&lines(1, "candidate", 6000, 0, 300)).unwrap(),
        );
        assert_eq!(late.default.ejections, 1);
        assert!(late.pass(), "{:?}", late.failures);
    }

    #[test]
    fn an_editor_that_hardly_ran_is_measured_as_such() {
        // Under the default the editor never ran: any measured candidate is under
        // 0.7x of it. A candidate under which it never ran fails.
        let mut starved = lines(2, "default", 5000, 0, 800);
        starved.retain(|l| !l.contains("samples 0"));
        let default = parse(&starved).unwrap();
        assert!(!default.measured());
        let won = judge(
            default.clone(),
            parse(&lines(2, "candidate", 6000, 0, 790)).unwrap(),
        );
        assert_eq!(won.ratio_permille, 0);
        assert!(won.pass(), "{:?}", won.failures);
        assert!(won.line().contains("the editor ran 0 times"));
        let mut idle = lines(2, "candidate", 6000, 0, 790);
        idle.retain(|l| !l.contains("samples 0"));
        let lost = judge(default, parse(&idle).unwrap());
        assert!(lost.failures[0].contains("ran 0 times under the candidate"));
        assert!(parse(&[String::from("tune-ab: seed 1 side default spec x labels")]).is_err());
    }
}
