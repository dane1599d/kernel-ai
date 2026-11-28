//! `cargo xtask release check [REV]` (P6 G62): a branch against the v0.1
//! checklist lines a machine can check (the P4 plan, section 3.5), each printed
//! as met, missing or the founder's, with the run or file that decides it.
//!
//! - `release/seq` at REV is above the last `v*` tag's, as release.yml's `seq`
//!   job requires on a tag (the last tag by creation date, as there). No tag
//!   in the clone, a failed `git tag`, or a tag's unreadable seq is missing,
//!   never met: a release check never says met falsely.
//! - `NOTICES.txt` is byte-identical in the image and the release, and the
//!   image builds the same three times: a green Release run (release.yml, its
//!   dry run off a tag) at REV, whose `assemble` `cmp`s both.
//! - Fluency at least 47/50, after one repair, on REV's model input: a
//!   fluency-model.yml run of the rhai gate, without `--memories`, at a commit
//!   whose files under the batcher's `MODEL_PATHS` (scripts/batch/config.sh)
//!   are REV's, and whose [`GATE_PATHS`] (the model pin both gates download,
//!   and the gates' own tasks and bars) are too. Renames count as both names.
//! - actbench green on REV's model input: an actbench-model.yml run at such a
//!   commit with an `actbench act seed N` job for each of 42, 1 and 2, each
//!   green, whose log shows ARMS `act`, EXTRA exactly `--gate` (no `--only-m8`
//!   or `--no-m8`), LIMIT empty, and `xtask: actbench act seed N: passes`.
//! - The boot cases at REV under KVM and TCG: a ci.yml run at REV in which every
//!   `Boot test (kvm, ...)` and `Boot test (tcg, ...)` shard passed, at least
//!   one of each (a pull request's own run boots KVM only; a batch, a push to
//!   main or a `full-ci-all` label boots both).
//! - The release signed by the founder's root: always the founder's, as is the
//!   tag itself.
//!
//! Each line names its run's event. A `pull_request` run tested REV merged
//! into its base, not REV's tree alone, so a run from any other event is
//! taken first, and a `pull_request` one says so.
//!
//! It reads git and, through `gh`, the repository's Actions runs; it starts,
//! dispatches and signs nothing. Exit 1 when a line a machine checks is missing.

use std::process::{Command, Stdio};

use crate::workspace_root;

const USAGE: &str = "usage: cargo xtask release check [REV]";
/// The checklist's fluency bar, out of the gate's 50 tasks, after one repair.
const FLUENCY_BAR: u32 = 47;
/// How many runs of a workflow at one commit, or green ones, are read.
const RUNS: u32 = 200;

/// Paths outside `MODEL_PATHS` that a fluency or actbench run at another
/// commit must also share with REV: the model pin both workflows download,
/// and the two gates' tasks and bars. A path ending in `/` is a directory.
pub const GATE_PATHS: [&str; 3] = [
    "xtask/models.json",
    "xtask/src/fluency/",
    "xtask/src/actbench/",
];

/// One Actions run: its id, the commit it ran at (a pull request's head, for
/// a `pull_request` run), its event, and its conclusion.
#[derive(Clone, Debug)]
pub struct Run {
    pub id: u64,
    pub sha: String,
    pub event: String,
    pub conclusion: String,
}

impl Run {
    /// How a line names the run: its id and event, and for a `pull_request`
    /// run what it tested.
    fn named(&self) -> String {
        match self.event.as_str() {
            "pull_request" => format!("{} (pull_request: REV merged into its base)", self.id),
            event => format!("{} ({event})", self.id),
        }
    }
}

/// Runs from any event but `pull_request` first, each group newest first.
fn own_tree_first(mut runs: Vec<Run>) -> Vec<Run> {
    runs.sort_by_key(|r| r.event == "pull_request");
    runs
}

/// What the check reads, so the host tests can give it fixture branches.
pub trait Facts {
    /// REV's full sha.
    fn sha(&self, rev: &str) -> Result<String, String>;
    /// `release/seq` at a commit or tag, if it has one.
    fn seq(&self, rev: &str) -> Option<String>;
    /// The last `v*` tag, by creation date; None when the clone has none.
    fn last_tag(&self) -> Result<Option<String>, String>;
    /// Whether two commits have the same model input; None if one is unknown.
    fn same_model_input(&self, a: &str, b: &str) -> Option<bool>;
    /// A workflow's completed runs, newest first: those at one commit, or its
    /// green ones (a list of every run fills with skipped label events).
    fn runs(&self, workflow: &str, which: Which) -> Result<Vec<Run>, String>;
    /// A run's jobs.
    fn jobs(&self, id: u64) -> Result<Vec<Job>, String>;
    /// One job's log.
    fn job_log(&self, job: u64) -> Result<String, String>;
    /// A run's log.
    fn log(&self, id: u64) -> Result<String, String>;
}

/// Which runs [`Facts::runs`] lists.
#[derive(Clone, Copy, Debug)]
pub enum Which<'a> {
    At(&'a str),
    Green,
}

/// One job of a run: its id, name and conclusion.
#[derive(Clone, Debug)]
pub struct Job {
    pub id: u64,
    pub name: String,
    pub conclusion: String,
}

/// The seeds the checklist's actbench run covers: the gate's own (the
/// workflow's default, and a label's run).
pub const ACT_SEEDS: [u32; 3] = [42, 1, 2];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Met,
    Missing,
    Founders,
}

/// One checklist line: what, its state, and the evidence or what is left.
#[derive(Debug)]
pub struct Line {
    pub what: &'static str,
    pub state: State,
    pub detail: String,
}

impl Line {
    fn new(what: &'static str, state: State, detail: String) -> Line {
        Line {
            what,
            state,
            detail,
        }
    }
}

pub fn main(args: &[String]) -> Result<(), String> {
    let rev = match args {
        [check] if check == "check" => "HEAD",
        [check, rev] if check == "check" => rev.as_str(),
        _ => return Err(USAGE.into()),
    };
    let lines = check(&Live, rev)?;
    for line in &lines {
        let state = match line.state {
            State::Met => "met",
            State::Missing => "MISSING",
            State::Founders => "founder's",
        };
        println!(
            "xtask: release check: {state}: {}: {}",
            line.what, line.detail
        );
    }
    let missing = lines.iter().filter(|l| l.state == State::Missing).count();
    match missing {
        0 => Ok(()),
        n => Err(format!("{n} checklist lines are missing at {rev}")),
    }
}

/// Every line, in the checklist's order.
pub fn check(facts: &dyn Facts, rev: &str) -> Result<Vec<Line>, String> {
    let sha = facts.sha(rev)?;
    let short = &sha[..sha.len().min(12)];
    Ok(vec![
        seq_line(facts, &sha),
        notices_line(facts, &sha, short)?,
        fluency_line(facts, &sha)?,
        actbench_line(facts, &sha)?,
        boot_line(facts, &sha, short)?,
        Line::new(
            "the release signed by the founder's root",
            State::Founders,
            "two signing rounds in the release PR, then the tag (root-ceremony.md)".into(),
        ),
    ])
}

fn seq_line(facts: &dyn Facts, sha: &str) -> Line {
    const WHAT: &str = "release/seq above the last release's";
    let Some(seq) = facts.seq(sha) else {
        return Line::new(WHAT, State::Missing, "no release/seq".into());
    };
    let Some(n) = seq_number(&seq) else {
        return Line::new(
            WHAT,
            State::Missing,
            format!(
                "release/seq holds `{}`, not a number",
                seq.trim_end_matches('\n')
            ),
        );
    };
    // Never met by default: no tag, a failed `git tag`, or a tag whose seq
    // cannot be read leaves nothing to be above.
    let tag = match facts.last_tag() {
        Ok(Some(tag)) => tag,
        Ok(None) => {
            return Line::new(
                WHAT,
                State::Missing,
                format!("{n}, but this clone has no v* tag to be above: fetch the tags"),
            );
        }
        Err(why) => {
            return Line::new(
                WHAT,
                State::Missing,
                format!("{n}, but git tag failed: {why}"),
            );
        }
    };
    let Some(last) = facts.seq(&tag).as_deref().and_then(seq_number) else {
        return Line::new(
            WHAT,
            State::Missing,
            format!("{n}, but {tag}'s release/seq cannot be read"),
        );
    };
    match n > last {
        true => Line::new(WHAT, State::Met, format!("{n}, above {tag}'s {last}")),
        false => Line::new(
            WHAT,
            State::Missing,
            format!("{n}, not above {tag}'s {last}: bump it in the release PR"),
        ),
    }
}

/// A `release/seq` file's number as release.yml's `seq` job reads it: `$(cat
/// release/seq)` drops the trailing newlines, then digits only (no sign, no
/// spaces).
pub fn seq_number(text: &str) -> Option<u32> {
    let s = text.trim_end_matches('\n');
    match !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
        true => s.parse().ok(),
        false => None,
    }
}

fn notices_line(facts: &dyn Facts, sha: &str, short: &str) -> Result<Line, String> {
    const WHAT: &str = "NOTICES.txt the same in the image and the release";
    let at: Vec<Run> = own_tree_first(
        facts
            .runs("release.yml", Which::At(sha))?
            .into_iter()
            .filter(|r| r.sha == sha)
            .collect(),
    );
    Ok(match at.iter().find(|r| r.conclusion == "success") {
        Some(run) => Line::new(
            WHAT,
            State::Met,
            format!(
                "Release run {} at {short} passed (assemble cmps it)",
                run.named()
            ),
        ),
        None => Line::new(WHAT, State::Missing, none_green("Release", short, &at)),
    })
}

/// A workflow's green runs at commits with REV's model input, newest first.
fn on_model_input(facts: &dyn Facts, workflow: &str, sha: &str) -> Result<Vec<Run>, String> {
    Ok(own_tree_first(
        facts
            .runs(workflow, Which::Green)?
            .into_iter()
            .filter(|r| facts.same_model_input(&r.sha, sha) == Some(true))
            .collect(),
    ))
}

fn fluency_line(facts: &dyn Facts, sha: &str) -> Result<Line, String> {
    const WHAT: &str = "fluency at least 47/50 on the model input";
    let runs = on_model_input(facts, "fluency-model.yml", sha)?;
    let mut seen = Vec::new();
    for run in runs.iter().filter(|r| r.conclusion == "success") {
        let log = facts.log(run.id)?;
        let Some((first, repaired)) = rhai_score(&log) else {
            continue;
        };
        let at = &run.sha[..run.sha.len().min(12)];
        if repaired >= FLUENCY_BAR {
            return Ok(Line::new(
                WHAT,
                State::Met,
                format!(
                    "run {} at {at}: {repaired}/50 after one repair ({first}/50 first try)",
                    run.named()
                ),
            ));
        }
        seen.push(format!("run {} at {at}: {repaired}/50", run.named()));
    }
    let detail = match seen.is_empty() {
        true => format!(
            "no green rhai fluency run at this model input ({} green runs there, none the rhai gate)",
            runs.len()
        ),
        false => format!("under {FLUENCY_BAR}/50: {}", seen.join(", ")),
    };
    Ok(Line::new(WHAT, State::Missing, detail))
}

/// The rhai gate's score from a fluency-model.yml log, first try and after one
/// repair; None for another language, `--memories`, or no score line. An
/// older workflow (release/v0.1's) prints no `GATE` or `MEM_FLAG` line and
/// had neither choice: its runs are the rhai gate without memories.
pub fn rhai_score(log: &str) -> Option<(u32, u32)> {
    let env = |name: &str| {
        log.lines()
            .find_map(|l| l.split_once(&format!("  {name}:")).map(|(_, v)| v.trim()))
    };
    let unset = |name: &str| env(name).is_none_or(str::is_empty);
    if env("GATE").is_some_and(|g| g != "fluency") || !unset("C_FLAG") || !unset("MEM_FLAG") {
        return None;
    }
    let rest = log.lines().find_map(|l| {
        l.split_once("xtask: fluency: ")
            .map(|(_, r)| r)
            .filter(|r| r.contains(" first try "))
    })?;
    let count = |s: &str| s.split_once("/50").and_then(|(n, _)| n.trim().parse().ok());
    let first = count(rest)?;
    let repaired = count(rest.split_once("), ")?.1)?;
    Some((first, repaired))
}

fn actbench_line(facts: &dyn Facts, sha: &str) -> Result<Line, String> {
    const WHAT: &str = "actbench green on the model input";
    let runs = on_model_input(facts, "actbench-model.yml", sha)?;
    let mut why = Vec::new();
    for run in &runs {
        let at = &run.sha[..run.sha.len().min(12)];
        match gated_seeds(facts, run.id)? {
            Ok(n) => {
                return Ok(Line::new(
                    WHAT,
                    State::Met,
                    format!(
                        "run {} at {at}: the gated act arm passed on seeds {n}",
                        run.named()
                    ),
                ));
            }
            Err(e) => why.push(format!("run {} at {at}: {e}", run.named())),
        }
    }
    let detail = match why.is_empty() {
        true => "no green actbench run at this model input".to_string(),
        false => format!("no full gated act run: {}", why.join("; ")),
    };
    Ok(Line::new(WHAT, State::Missing, detail))
}

/// Whether a green run is the checklist's: an `actbench act seed N` job for
/// each of [`ACT_SEEDS`], each green, run with `--gate` and nothing else
/// (`--only-m8` or `--no-m8` narrow it), no `--limit`, and logging that its
/// seed passes. Ok(the seeds) or Err(what is short).
fn gated_seeds(facts: &dyn Facts, run: u64) -> Result<Result<String, String>, String> {
    let jobs = facts.jobs(run)?;
    let mut passed = Vec::new();
    for job in jobs
        .iter()
        .filter(|j| j.name.starts_with("actbench act seed "))
    {
        if job.conclusion != "success" {
            return Ok(Err(format!("{} is {}", job.name, job.conclusion)));
        }
        match act_job(&facts.job_log(job.id)?) {
            Ok(seed) => passed.push(seed),
            Err(e) => return Ok(Err(format!("{}: {e}", job.name))),
        }
    }
    let missing: Vec<String> = ACT_SEEDS
        .iter()
        .filter(|s| !passed.contains(s))
        .map(u32::to_string)
        .collect();
    if !missing.is_empty() {
        return Ok(Err(format!(
            "no gated act job for seed {}",
            missing.join(", ")
        )));
    }
    Ok(Ok(ACT_SEEDS.map(|s| s.to_string()).join(", ")))
}

/// One `actbench act` job's log: its seed when it ran the act arm alone,
/// with EXTRA exactly `--gate`, LIMIT empty, and a `passes` line for that seed.
pub fn act_job(log: &str) -> Result<u32, String> {
    let env = |name: &str| {
        log.lines()
            .find_map(|l| l.split_once(&format!("  {name}:")).map(|(_, v)| v.trim()))
    };
    if env("ARMS") != Some("act") {
        return Err(format!(
            "arms {:?}, not act",
            env("ARMS").unwrap_or("unset")
        ));
    }
    if env("EXTRA") != Some("--gate") {
        return Err(format!(
            "options {:?}, not --gate alone",
            env("EXTRA").unwrap_or("unset")
        ));
    }
    if env("LIMIT") != Some("") {
        return Err(format!("limit {:?}", env("LIMIT").unwrap_or("unset")));
    }
    let seed: u32 = env("SEEDS")
        .and_then(|s| s.parse().ok())
        .ok_or("no single seed")?;
    let passes = format!("xtask: actbench act seed {seed}: passes");
    match log.lines().any(|l| l.trim_end().ends_with(&passes)) {
        true => Ok(seed),
        false => Err(format!("no `{passes}` line")),
    }
}

fn boot_line(facts: &dyn Facts, sha: &str, short: &str) -> Result<Line, String> {
    const WHAT: &str = "the boot cases under KVM and TCG";
    let at: Vec<Run> = own_tree_first(
        facts
            .runs("ci.yml", Which::At(sha))?
            .into_iter()
            .filter(|r| r.sha == sha)
            .collect(),
    );
    let mut partial = Vec::new();
    for run in &at {
        let jobs = facts.jobs(run.id)?;
        let shards = |accel: &str| {
            let of: Vec<_> = jobs
                .iter()
                .filter(|j| j.name.starts_with(&format!("Boot test ({accel}, ")))
                .collect();
            (
                of.len(),
                of.iter().filter(|j| j.conclusion == "success").count(),
            )
        };
        let (kvm, tcg) = (shards("kvm"), shards("tcg"));
        if kvm.0 > 0 && kvm.0 == kvm.1 && tcg.0 > 0 && tcg.0 == tcg.1 {
            return Ok(Line::new(
                WHAT,
                State::Met,
                format!(
                    "CI run {} at {short}: {} KVM and {} TCG shards passed",
                    run.named(),
                    kvm.0,
                    tcg.0
                ),
            ));
        }
        partial.push(format!(
            "run {}: KVM {}/{}, TCG {}/{}",
            run.named(),
            kvm.1,
            kvm.0,
            tcg.1,
            tcg.0
        ));
    }
    let detail = match partial.is_empty() {
        true => format!("no CI run at {short}"),
        false => format!("no run with every shard of both: {}", partial.join(", ")),
    };
    Ok(Line::new(WHAT, State::Missing, detail))
}

fn none_green(name: &str, short: &str, at: &[Run]) -> String {
    match at.is_empty() {
        true => format!("no {name} run at {short}"),
        false => format!(
            "no green {name} run at {short}: {}",
            at.iter()
                .map(|r| format!("{} {}", r.named(), r.conclusion))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// The real repository: git in the workspace, and `gh` for the runs.
struct Live;

fn output(cmd: &mut Command) -> Result<String, String> {
    let what = format!("{cmd:?}");
    let out = cmd
        .current_dir(workspace_root())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("{what}: {e}"))?;
    match out.status.success() {
        true => Ok(String::from_utf8_lossy(&out.stdout).into_owned()),
        false => Err(format!(
            "{what}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )),
    }
}

fn git(args: &[&str]) -> Result<String, String> {
    output(Command::new("git").args(args))
}

/// The batcher's model-input pattern, read from scripts/batch/config.sh so the
/// two never differ.
fn model_paths() -> Result<String, String> {
    let config = workspace_root().join("scripts/batch/config.sh");
    let text =
        std::fs::read_to_string(&config).map_err(|e| format!("{}: {e}", config.display()))?;
    model_paths_in(&text).ok_or_else(|| format!("{}: no MODEL_PATHS line", config.display()))
}

/// `MODEL_PATHS=${KAI_MODEL_PATHS:-'<pattern>'}`'s pattern.
pub fn model_paths_in(config: &str) -> Option<String> {
    let line = config.lines().find(|l| l.starts_with("MODEL_PATHS="))?;
    let (_, rest) = line.split_once(":-'")?;
    Some(rest.strip_suffix("'}")?.to_string())
}

/// Whether a changed file is one of [`GATE_PATHS`].
pub fn is_gate_path(name: &str) -> bool {
    GATE_PATHS.iter().any(|p| match p.ends_with('/') {
        true => name.starts_with(p),
        false => name == *p,
    })
}

/// `grep -Eq MODEL_PATHS` over the changed names, by its exit code: 0 is a
/// model path changed, 1 is none did, and anything else (2, a bad pattern or
/// a read error; or a signal) is unknown, so the run never counts.
pub fn same_by_grep(code: Option<i32>) -> Option<bool> {
    match code {
        Some(0) => Some(false),
        Some(1) => Some(true),
        _ => None,
    }
}

impl Facts for Live {
    fn sha(&self, rev: &str) -> Result<String, String> {
        Ok(
            git(&["rev-parse", "--verify", &format!("{rev}^{{commit}}")])?
                .trim()
                .to_string(),
        )
    }

    fn seq(&self, rev: &str) -> Option<String> {
        git(&["show", &format!("{rev}:release/seq")]).ok()
    }

    fn last_tag(&self) -> Result<Option<String>, String> {
        let tags = git(&["tag", "--list", "v*", "--sort=-creatordate"])?;
        Ok(tags.lines().next().map(str::to_string))
    }

    fn same_model_input(&self, a: &str, b: &str) -> Option<bool> {
        if a == b {
            return Some(true);
        }
        // Without rename detection a file moved out of a model path is listed
        // under its old name too.
        let names = git(&["diff", "--no-renames", "--name-only", a, b]).ok()?;
        if names.lines().any(is_gate_path) {
            return Some(false);
        }
        let pattern = model_paths().ok()?;
        let mut grep = Command::new("grep")
            .args(["-Eq", &pattern])
            .stdin(Stdio::piped())
            .spawn()
            .ok()?;
        use std::io::Write as _;
        grep.stdin.take()?.write_all(names.as_bytes()).ok()?;
        same_by_grep(grep.wait().ok()?.code())
    }

    fn runs(&self, workflow: &str, which: Which) -> Result<Vec<Run>, String> {
        let limit = RUNS.to_string();
        let mut args = vec!["run", "list", "--workflow", workflow, "--limit", &limit];
        match which {
            Which::At(sha) => args.extend(["--commit", sha]),
            Which::Green => args.extend(["--status", "success"]),
        }
        args.extend(["--json", "databaseId,headSha,event,conclusion,status"]);
        let json = output(Command::new("gh").args(args))?;
        let value: serde_json::Value =
            serde_json::from_str(&json).map_err(|e| format!("gh run list {workflow}: {e}"))?;
        Ok(value
            .as_array()
            .into_iter()
            .flatten()
            .filter(|r| r["status"] == "completed")
            .filter_map(|r| {
                Some(Run {
                    id: r["databaseId"].as_u64()?,
                    sha: r["headSha"].as_str()?.to_string(),
                    event: r["event"].as_str().unwrap_or("").to_string(),
                    conclusion: r["conclusion"].as_str().unwrap_or("").to_string(),
                })
            })
            .collect())
    }

    fn jobs(&self, id: u64) -> Result<Vec<Job>, String> {
        let json =
            output(Command::new("gh").args(["run", "view", &id.to_string(), "--json", "jobs"]))?;
        let value: serde_json::Value =
            serde_json::from_str(&json).map_err(|e| format!("gh run view {id}: {e}"))?;
        Ok(value["jobs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|j| {
                Some(Job {
                    id: j["databaseId"].as_u64()?,
                    name: j["name"].as_str()?.to_string(),
                    conclusion: j["conclusion"].as_str().unwrap_or("").to_string(),
                })
            })
            .collect())
    }

    fn job_log(&self, job: u64) -> Result<String, String> {
        output(Command::new("gh").args(["run", "view", "--job", &job.to_string(), "--log"]))
    }

    fn log(&self, id: u64) -> Result<String, String> {
        output(Command::new("gh").args(["run", "view", &id.to_string(), "--log"]))
    }
}

#[cfg(test)]
mod tests;
