use std::collections::BTreeMap;

use super::*;

/// A fixture branch: commits by name, each with its `release/seq` and model
/// input, the last tag, and the runs at them.
#[derive(Default)]
struct Branch {
    seq: BTreeMap<String, String>,
    /// A commit's model input, as a label: two commits with one label match.
    input: BTreeMap<String, &'static str>,
    tag: Option<String>,
    runs: BTreeMap<&'static str, Vec<Run>>,
    jobs: BTreeMap<u64, Vec<(String, String)>>,
    logs: BTreeMap<u64, String>,
}

impl Facts for Branch {
    fn sha(&self, rev: &str) -> Result<String, String> {
        Ok(rev.to_string())
    }
    fn seq(&self, rev: &str) -> Option<String> {
        self.seq.get(rev).cloned()
    }
    fn last_tag(&self) -> Option<String> {
        self.tag.clone()
    }
    fn same_model_input(&self, a: &str, b: &str) -> Option<bool> {
        let input = |s: &str| self.input.get(s);
        Some(input(a)? == input(b)?)
    }
    fn runs(&self, workflow: &str, which: Which) -> Result<Vec<Run>, String> {
        let all = self.runs.get(workflow).cloned().unwrap_or_default();
        Ok(all
            .into_iter()
            .filter(|r| match which {
                Which::At(sha) => r.sha == sha,
                Which::Green => r.conclusion == "success",
            })
            .collect())
    }
    fn jobs(&self, id: u64) -> Result<Vec<(String, String)>, String> {
        Ok(self.jobs.get(&id).cloned().unwrap_or_default())
    }
    fn log(&self, id: u64) -> Result<String, String> {
        self.logs.get(&id).cloned().ok_or(format!("no log {id}"))
    }
}

fn run(id: u64, at: &str, conclusion: &str) -> Run {
    Run {
        id,
        sha: at.into(),
        conclusion: conclusion.into(),
    }
}

fn jobs(names: &[(&str, &str)]) -> Vec<(String, String)> {
    names
        .iter()
        .map(|(n, c)| (n.to_string(), c.to_string()))
        .collect()
}

/// A fluency-model.yml log's lines as GitHub prints them: job, step, time.
fn fluency_log(c_flag: &str, mem_flag: &str, first: u32, repaired: u32) -> String {
    let p =
        "Fluency gate (qwen3-4b-2507, local engine)\tRun the gate's 50 tasks\t2026-11-27T20:59:28Z";
    format!(
        "{p}   C_FLAG: {c_flag}\n{p}   MEM_FLAG: {mem_flag}\n{p}   GATE: fluency\n\
         {p} xtask: fluency: local model Qwen3-4B-Instruct-2507, loaded\n\
         {p} xtask: fluency: {first}/50 first try ({}%), {repaired}/50 after one repair ({}%); gate 70% and 90%\n",
        first * 2,
        repaired * 2
    )
}

fn boots(accels: &[(&str, usize, &str)]) -> Vec<(String, String)> {
    let mut out = vec![("Format, lint, host unit tests".into(), "success".into())];
    for (accel, of, c) in accels {
        for k in 1..=*of {
            out.push((
                format!("Boot test ({accel}, shard {k}/{of})"),
                c.to_string(),
            ));
        }
    }
    out
}

/// An rc.2 branch with everything a machine checks: each line is met.
fn ready() -> Branch {
    let mut b = Branch::default();
    b.seq.insert("v0.1.0-rc.1".into(), "1\n".into());
    b.seq.insert("rc2".into(), "2\n".into());
    b.tag = Some("v0.1.0-rc.1".into());
    // The fluency and actbench runs were at an earlier commit with the same
    // model input; `docs` changed only a doc since.
    b.input.insert("rc2".into(), "pin-1");
    b.input.insert("docs".into(), "pin-1");
    b.input.insert("old".into(), "pin-0");
    b.runs
        .insert("release.yml", vec![run(10, "rc2", "success")]);
    b.runs.insert(
        "fluency-model.yml",
        vec![run(20, "docs", "success"), run(21, "old", "success")],
    );
    b.logs.insert(20, fluency_log("", "", 47, 48));
    b.runs
        .insert("actbench-model.yml", vec![run(30, "docs", "success")]);
    b.jobs.insert(
        30,
        jobs(&[
            ("plan", "success"),
            ("actbench act seed 42", "success"),
            ("actbench act seed 1", "success"),
            ("actbench shipped seed 42", "failure"),
        ]),
    );
    b.runs.insert(
        "ci.yml",
        vec![run(40, "rc2", "success"), run(41, "rc2", "success")],
    );
    // A pull request's run boots KVM only; the batch's boots both.
    b.jobs.insert(40, boots(&[("kvm", 3, "success")]));
    b.jobs
        .insert(41, boots(&[("kvm", 3, "success"), ("tcg", 4, "success")]));
    b
}

fn states(lines: &[Line]) -> Vec<(&'static str, State)> {
    lines.iter().map(|l| (l.what, l.state)).collect()
}

#[test]
fn a_ready_branch_meets_every_line_a_machine_checks() {
    let lines = check(&ready(), "rc2").unwrap();
    let got: Vec<State> = lines.iter().map(|l| l.state).collect();
    use State::*;
    assert_eq!(got, [Met, Met, Met, Met, Met, Founders], "{lines:#?}");
    assert_eq!(lines[0].detail, "2, above v0.1.0-rc.1's 1");
    assert!(
        lines[1].detail.starts_with("Release run 10 at rc2"),
        "{lines:#?}"
    );
    assert!(
        lines[2]
            .detail
            .starts_with("run 20 at docs: 48/50 after one repair (47/50 first try)"),
        "{lines:#?}"
    );
    assert!(lines[3].detail.contains("2 act jobs passed"), "{lines:#?}");
    assert!(
        lines[4]
            .detail
            .starts_with("CI run 41 at rc2: 3 KVM and 4 TCG shards"),
        "{lines:#?}"
    );
}

/// What release/v0.1 looked like before rc.2: seq not bumped, fluency at
/// 46/50, actbench and the dry run never run there, and a KVM-only CI run.
#[test]
fn a_branch_short_of_rc2_names_each_line_left() {
    let mut b = ready();
    b.seq.insert("rc2".into(), "1".into());
    b.runs.insert(
        "release.yml",
        vec![run(11, "rc2", "failure"), run(12, "docs", "success")],
    );
    b.logs.insert(20, fluency_log("", "", 46, 46));
    b.runs
        .insert("actbench-model.yml", vec![run(31, "old", "success")]);
    b.runs.insert("ci.yml", vec![run(40, "rc2", "success")]);
    let lines = check(&b, "rc2").unwrap();
    use State::*;
    assert_eq!(
        states(&lines).iter().map(|s| s.1).collect::<Vec<_>>(),
        [Missing, Missing, Missing, Missing, Missing, Founders],
        "{lines:#?}"
    );
    assert_eq!(
        lines[0].detail,
        "1, not above v0.1.0-rc.1's 1: bump it in the release PR"
    );
    assert_eq!(lines[1].detail, "no green Release run at rc2: 11 failure");
    assert_eq!(lines[2].detail, "under 47/50: run 20 at docs: 46/50");
    assert_eq!(
        lines[3].detail,
        "no green run of the act arm at this model input (0 green runs there)"
    );
    assert_eq!(
        lines[4].detail,
        "no run with every shard of both: run 40: KVM 3/3, TCG 0/0"
    );
}

/// A run at another model input never counts, nor one of another language,
/// with `--memories`, or with a shard that failed.
#[test]
fn only_the_rhai_gate_at_the_same_model_input_counts() {
    let mut b = ready();
    // The passing run is at the old pin; at this pin only c and memories ran.
    b.runs.insert(
        "fluency-model.yml",
        vec![
            run(21, "old", "success"),
            run(22, "docs", "success"),
            run(23, "docs", "success"),
            run(24, "docs", "failure"),
        ],
    );
    b.logs.insert(21, fluency_log("", "", 50, 50));
    b.logs.insert(22, fluency_log("--c", "", 49, 50));
    b.logs.insert(23, fluency_log("", "--memories", 49, 50));
    b.jobs.insert(
        41,
        boots(&[
            ("kvm", 3, "success"),
            ("tcg", 4, "success"),
            ("tcg", 1, "failure"),
        ]),
    );
    b.jobs
        .insert(30, jobs(&[("actbench shipped seed 42", "success")]));
    let lines = check(&b, "rc2").unwrap();
    assert_eq!(lines[2].state, State::Missing);
    assert_eq!(
        lines[2].detail,
        "no green rhai fluency run at this model input (2 green runs there, none the rhai gate)"
    );
    assert_eq!(lines[3].state, State::Missing, "{lines:#?}");
    assert_eq!(lines[4].state, State::Missing);
    assert!(
        lines[4].detail.contains("run 41: KVM 3/3, TCG 4/5"),
        "{lines:#?}"
    );
}

#[test]
fn no_tag_and_an_odd_seq() {
    let mut b = ready();
    b.tag = None;
    assert_eq!(check(&b, "rc2").unwrap()[0].detail, "2, above no tag's 0");
    b.seq.insert("rc2".into(), "two".into());
    let line = &check(&b, "rc2").unwrap()[0];
    assert_eq!(
        (line.state, line.detail.as_str()),
        (State::Missing, "release/seq holds `two`, not a number")
    );
    b.seq.remove("rc2");
    assert_eq!(check(&b, "rc2").unwrap()[0].detail, "no release/seq");
}

#[test]
fn the_score_is_read_from_a_real_log_line() {
    assert_eq!(rhai_score(&fluency_log("", "", 46, 48)), Some((46, 48)));
    assert_eq!(rhai_score(&fluency_log("--c", "", 46, 48)), None);
    assert_eq!(rhai_score("xtask: fluency: 46/50 first try (92%)"), None);
    // release/v0.1's older workflow prints only C_FLAG.
    let old = fluency_log("", "", 48, 48)
        .lines()
        .filter(|l| !l.contains("MEM_FLAG") && !l.contains("GATE"))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(rhai_score(&old), Some((48, 48)));
    let app = fluency_log("", "", 48, 48).replace("GATE: fluency", "GATE: app-gate");
    assert_eq!(rhai_score(&app), None);
}

/// The pattern is the batcher's, read from its own file.
#[test]
fn the_model_paths_are_the_batchers() {
    let config = std::fs::read_to_string(workspace_root().join("scripts/batch/config.sh")).unwrap();
    let pattern = model_paths_in(&config).unwrap();
    assert!(pattern.starts_with("^(llmproto/|infer/src/"), "{pattern}");
    assert!(pattern.ends_with(')'), "{pattern}");
    assert_eq!(model_paths_in("MODEL_PATHS=x"), None);
}
