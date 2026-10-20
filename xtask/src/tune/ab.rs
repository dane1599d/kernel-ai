//! The M10 exit test's rule, on what the `tune-ab` measure boots printed:
//! - every seed's verdict is kept;
//! - the editor's p99, pooled over every seed's windows (100 us floor, exact
//!   samples), is at most 0.7x the baseline windows', and no seed is worse;
//! - background CPU holds: at least 0.9x in total and 0.8x for every
//!   background process with 1% or more, on every seed (the kernel's judge
//!   checks both; the report carries its numbers);
//! - every runnable wait stays under 100 ms, with no ejection and no runaway;
//! - the measurement window printed nothing;
//! - an identical repeat of the first seed gives the same digest.
//!
//! Ratios only: absolute times under icount mean nothing outside it.

/// The pooled p99 ratio that passes, in thousandths.
pub const POOLED_MAX_PERMILLE: u64 = 700;
pub const BACKGROUND_TOTAL_MIN: u64 = 900;
pub const BACKGROUND_EACH_MIN: u64 = 800;
pub const GUARDRAIL_US: u64 = 100_000;

/// One measure boot's report.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SeedRun {
    pub seed: u64,
    pub spec: String,
    pub kept: bool,
    pub reason: u64,
    pub baseline_p99_us: u64,
    pub candidate_p99_us: u64,
    pub background_permille: u64,
    pub worst_permille: u64,
    pub max_wait_us: u64,
    pub ejections: u64,
    pub runaways: u64,
    pub quiet_bytes: u64,
    pub digest: String,
    pub baseline_samples: Vec<u64>,
    pub candidate_samples: Vec<u64>,
}

fn number(words: &[&str], key: &str) -> Option<u64> {
    let at = words.iter().position(|w| *w == key)?;
    words.get(at + 1)?.parse().ok()
}

/// The report of one boot's console lines.
pub fn parse(lines: &[String]) -> Result<SeedRun, String> {
    let mut run = SeedRun::default();
    let (mut verdict, mut guard) = (false, false);
    for line in lines.iter().filter(|l| !crate::qemu::from_programs(l)) {
        let Some(at) = line.find("tune-ab: ") else {
            continue;
        };
        let words: Vec<&str> = line[at + 9..].split_whitespace().collect();
        match words.first().copied() {
            Some("seed") => {
                run.seed = number(&words, "seed").ok_or("a bad seed line")?;
                run.spec = words.get(3).unwrap_or(&"").to_string();
            }
            Some("window") => {
                run.max_wait_us = run
                    .max_wait_us
                    .max(number(&words, "max_wait_us").unwrap_or(0));
                run.ejections += number(&words, "ejections").unwrap_or(0);
            }
            Some("samples") => {
                let values = words.iter().skip(3).filter_map(|w| w.parse::<u64>().ok());
                match words.get(2).copied() {
                    Some("baseline") => run.baseline_samples.extend(values),
                    Some("candidate") => run.candidate_samples.extend(values),
                    _ => return Err(format!("a bad samples line: {line}")),
                }
            }
            Some("verdict") => {
                verdict = true;
                run.kept = number(&words, "state") == Some(2);
                run.reason = number(&words, "reason").unwrap_or(0);
                run.baseline_p99_us = number(&words, "baseline_p99_us").unwrap_or(0);
                run.candidate_p99_us = number(&words, "candidate_p99_us").unwrap_or(0);
                run.background_permille = number(&words, "background_permille").unwrap_or(0);
                run.worst_permille = number(&words, "worst_permille").unwrap_or(0);
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
    if !verdict || !guard || run.digest.is_empty() {
        return Err("the boot printed no complete tune-ab report".into());
    }
    Ok(run)
}

/// The nearest-rank 99th percentile.
fn p99(mut values: Vec<u64>) -> u64 {
    values.sort_unstable();
    sched::stats::percentile(&values, 99)
}

/// The exit test's verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub pass: bool,
    pub failures: Vec<String>,
    pub pooled_baseline_p99_us: u64,
    pub pooled_candidate_p99_us: u64,
    pub pooled_ratio_permille: u64,
    pub seeds: Vec<SeedRun>,
    pub canary: Option<SeedRun>,
}

impl SeedRun {
    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "seed": self.seed,
            "spec": self.spec,
            "kept": self.kept,
            "reason": self.reason,
            "editor_p99_ratio_permille": sched::judge::permille(self.candidate_p99_us, self.baseline_p99_us),
            "background_permille": self.background_permille,
            "worst_background_permille": self.worst_permille,
            "max_wait_under_100ms": self.max_wait_us < GUARDRAIL_US,
            "ejections": self.ejections,
            "runaways": self.runaways,
            "quiet_bytes": self.quiet_bytes,
            "digest": self.digest,
        })
    }
}

impl Report {
    /// The JSON report: ratios, not absolute times.
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "pass": self.pass,
            "failures": self.failures,
            "pooled_editor_p99_ratio_permille": self.pooled_ratio_permille,
            "seeds": self.seeds.iter().map(SeedRun::json).collect::<Vec<_>>(),
            "canary": self.canary.as_ref().map(SeedRun::json),
        })
    }
}

pub fn evaluate(seeds: Vec<SeedRun>, canary: Option<SeedRun>) -> Report {
    let mut failures = Vec::new();
    let base = p99(seeds
        .iter()
        .flat_map(|s| s.baseline_samples.clone())
        .collect());
    let cand = p99(seeds
        .iter()
        .flat_map(|s| s.candidate_samples.clone())
        .collect());
    let ratio = sched::judge::permille(cand, base);
    if seeds.is_empty() {
        failures.push(String::from("no seed ran"));
    }
    if ratio > POOLED_MAX_PERMILLE {
        failures.push(format!(
            "pooled editor p99 is {ratio}/1000 of the baseline's, over 700"
        ));
    }
    for s in &seeds {
        let mut at = |what: &str| failures.push(format!("seed {}: {what}", s.seed));
        if !s.kept {
            at(&format!("not kept (reason {})", s.reason));
        }
        if s.candidate_p99_us > s.baseline_p99_us {
            at("the editor's p99 is worse");
        }
        if s.background_permille < BACKGROUND_TOTAL_MIN {
            at("background CPU under 0.9x");
        }
        if s.worst_permille < BACKGROUND_EACH_MIN {
            at("a background process under 0.8x of its CPU");
        }
        if s.max_wait_us >= GUARDRAIL_US || s.ejections > 0 || s.runaways > 0 {
            at("a wait of 100 ms or more, an ejection or a runaway");
        }
        if s.quiet_bytes > 0 {
            at("the measurement window was not quiet");
        }
    }
    match (&canary, seeds.first()) {
        (Some(c), Some(first)) if c.seed == first.seed && c.digest == first.digest => {}
        (Some(c), Some(first)) => failures.push(format!(
            "the canary repeat of seed {} gave digest {}, not {}",
            first.seed, c.digest, first.digest
        )),
        _ => failures.push(String::from("no canary repeat ran")),
    }
    Report {
        pass: failures.is_empty(),
        failures,
        pooled_baseline_p99_us: base,
        pooled_candidate_p99_us: cand,
        pooled_ratio_permille: ratio,
        seeds,
        canary,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(seed: u64, base: u64, cand: u64, digest: &str) -> Vec<String> {
        let mut out = vec![
            format!("tune-ab: seed {seed} spec classes/firm labels build-1"),
            String::from(
                "tune-ab: window 0 baseline p99_us 5000 samples 3 background_cpu_ns 9 max_wait_us 900 ejections 0",
            ),
            format!("tune-ab: samples 0 baseline {base} {base} 100"),
            format!("tune-ab: samples 0 candidate {cand} {cand} 100"),
            format!(
                "tune-ab: verdict state 2 reason 0 baseline_p99_us {base} candidate_p99_us {cand} background_permille 950 worst_permille 850"
            ),
            String::from("tune-ab: guard runaways 0 quiet_bytes 0"),
            format!("tune-ab: digest {digest}"),
            String::from("[editor] tune-ab: digest forged"),
        ];
        out.insert(0, String::from("measure-start"));
        out
    }

    #[test]
    fn five_kept_seeds_and_a_matching_canary_pass() {
        let seeds: Vec<SeedRun> = (1..=5)
            .map(|s| parse(&lines(s, 5000, 2000, "aa")).unwrap())
            .collect();
        assert_eq!(seeds[0].baseline_samples, [5000, 5000, 100]);
        assert_eq!(seeds[0].digest, "aa");
        let canary = parse(&lines(1, 5000, 2000, "aa")).ok();
        let report = evaluate(seeds.clone(), canary);
        assert!(report.pass, "{:?}", report.failures);
        assert_eq!(report.pooled_ratio_permille, 400);
        // A different canary, a weak effect or a missing canary fails.
        assert!(!evaluate(seeds.clone(), parse(&lines(1, 5000, 2000, "bb")).ok()).pass);
        let weak: Vec<SeedRun> = (1..=5)
            .map(|s| parse(&lines(s, 5000, 4000, "aa")).unwrap())
            .collect();
        let report = evaluate(weak, parse(&lines(1, 5000, 4000, "aa")).ok());
        assert!(report.failures.iter().any(|f| f.contains("over 700")));
        assert!(!evaluate(seeds, None).pass);
        assert!(parse(&[String::from("tune-ab: seed 1 spec x labels")]).is_err());
    }

    #[test]
    fn a_noisy_or_starving_seed_fails() {
        let mut bad = parse(&lines(2, 5000, 2000, "aa")).unwrap();
        bad.quiet_bytes = 12;
        bad.worst_permille = 700;
        let first = parse(&lines(1, 5000, 2000, "aa")).unwrap();
        let report = evaluate(vec![first.clone(), bad], Some(first));
        assert_eq!(report.failures.len(), 2, "{:?}", report.failures);
    }
}
