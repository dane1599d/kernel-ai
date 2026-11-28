//! `cargo xtask release check [REV]` (P6 G62): a branch against the v0.1
//! checklist lines a machine can check (the P4 plan, section 3.5), each printed
//! as met, missing or the founder's, with the run or file that decides it.
//!
//! - `release/seq` at REV is above the last `v*` tag's, as release.yml's `seq`
//!   job requires on a tag (the last tag by creation date, as there).
//! - `NOTICES.txt` is byte-identical in the image and the release, and the
//!   image builds the same three times: a green Release run (release.yml, its
//!   dry run off a tag) at REV, whose `assemble` `cmp`s both.
//! - Fluency at least 47/50, after one repair, on REV's model input: a
//!   fluency-model.yml run of the rhai gate, without `--memories`, at a commit
//!   whose files under the batcher's `MODEL_PATHS` (scripts/batch/config.sh)
//!   are REV's.
//! - actbench green on REV's model input: a green actbench-model.yml run with
//!   the gated `act` arm, at such a commit.
//! - The boot cases at REV under KVM and TCG: a ci.yml run at REV in which every
//!   `Boot test (kvm, ...)` and `Boot test (tcg, ...)` shard passed, at least
//!   one of each (a pull request's own run boots KVM only; a batch, a push to
//!   main or a `full-ci-all` label boots both).
//! - The release signed by the founder's root: always the founder's, as is the
//!   tag itself.
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

/// One Actions run: its id, the commit it ran at, and its conclusion.
#[derive(Clone, Debug)]
pub struct Run {
    pub id: u64,
    pub sha: String,
    pub conclusion: String,
}

/// What the check reads, so the host tests can give it fixture branches.
pub trait Facts {
    /// REV's full sha.
    fn sha(&self, rev: &str) -> Result<String, String>;
    /// `release/seq` at a commit or tag, if it has one.
    fn seq(&self, rev: &str) -> Option<String>;
    /// The last `v*` tag, by creation date.
    fn last_tag(&self) -> Option<String>;
    /// Whether two commits have the same model input; None if one is unknown.
    fn same_model_input(&self, a: &str, b: &str) -> Option<bool>;
    /// A workflow's completed runs, newest first: those at one commit, or its
    /// green ones (a list of every run fills with skipped label events).
    fn runs(&self, workflow: &str, which: Which) -> Result<Vec<Run>, String>;
    /// A run's jobs, by name and conclusion.
    fn jobs(&self, id: u64) -> Result<Vec<(String, String)>, String>;
    /// A run's log.
    fn log(&self, id: u64) -> Result<String, String>;
}

/// Which runs [`Facts::runs`] lists.
#[derive(Clone, Copy, Debug)]
pub enum Which<'a> {
    At(&'a str),
    Green,
}

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
    let Ok(n) = seq.trim().parse::<u32>() else {
        return Line::new(
            WHAT,
            State::Missing,
            format!("release/seq holds `{}`, not a number", seq.trim()),
        );
    };
    let (tag, last) = match facts.last_tag() {
        Some(tag) => {
            let last = facts.seq(&tag).and_then(|s| s.trim().parse().ok());
            (tag, last.unwrap_or(0))
        }
        None => ("no tag".into(), 0),
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

fn notices_line(facts: &dyn Facts, sha: &str, short: &str) -> Result<Line, String> {
    const WHAT: &str = "NOTICES.txt the same in the image and the release";
    let at: Vec<Run> = facts
        .runs("release.yml", Which::At(sha))?
        .into_iter()
        .filter(|r| r.sha == sha)
        .collect();
    Ok(match at.iter().find(|r| r.conclusion == "success") {
        Some(run) => Line::new(
            WHAT,
            State::Met,
            format!(
                "Release run {} at {short} passed (assemble cmps it)",
                run.id
            ),
        ),
        None => Line::new(WHAT, State::Missing, none_green("Release", short, &at)),
    })
}

/// A workflow's green runs at commits with REV's model input, newest first.
fn on_model_input(facts: &dyn Facts, workflow: &str, sha: &str) -> Result<Vec<Run>, String> {
    Ok(facts
        .runs(workflow, Which::Green)?
        .into_iter()
        .filter(|r| facts.same_model_input(&r.sha, sha) == Some(true))
        .collect())
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
                    run.id
                ),
            ));
        }
        seen.push(format!("run {} at {at}: {repaired}/50", run.id));
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
    for run in runs.iter().filter(|r| r.conclusion == "success") {
        let jobs = facts.jobs(run.id)?;
        let act: Vec<_> = jobs
            .iter()
            .filter(|(n, _)| n.starts_with("actbench act "))
            .collect();
        if !act.is_empty() && act.iter().all(|(_, c)| c == "success") {
            let at = &run.sha[..run.sha.len().min(12)];
            return Ok(Line::new(
                WHAT,
                State::Met,
                format!("run {} at {at}: {} act jobs passed", run.id, act.len()),
            ));
        }
    }
    Ok(Line::new(
        WHAT,
        State::Missing,
        format!(
            "no green run of the act arm at this model input ({} green runs there)",
            runs.len()
        ),
    ))
}

fn boot_line(facts: &dyn Facts, sha: &str, short: &str) -> Result<Line, String> {
    const WHAT: &str = "the boot cases under KVM and TCG";
    let at: Vec<Run> = facts
        .runs("ci.yml", Which::At(sha))?
        .into_iter()
        .filter(|r| r.sha == sha)
        .collect();
    let mut partial = Vec::new();
    for run in &at {
        let jobs = facts.jobs(run.id)?;
        let shards = |accel: &str| {
            let of: Vec<_> = jobs
                .iter()
                .filter(|(n, _)| n.starts_with(&format!("Boot test ({accel}, ")))
                .collect();
            (of.len(), of.iter().filter(|(_, c)| c == "success").count())
        };
        let (kvm, tcg) = (shards("kvm"), shards("tcg"));
        if kvm.0 > 0 && kvm.0 == kvm.1 && tcg.0 > 0 && tcg.0 == tcg.1 {
            return Ok(Line::new(
                WHAT,
                State::Met,
                format!(
                    "CI run {} at {short}: {} KVM and {} TCG shards passed",
                    run.id, kvm.0, tcg.0
                ),
            ));
        }
        partial.push(format!(
            "run {}: KVM {}/{}, TCG {}/{}",
            run.id, kvm.1, kvm.0, tcg.1, tcg.0
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
                .map(|r| format!("{} {}", r.id, r.conclusion))
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

    fn last_tag(&self) -> Option<String> {
        let tags = git(&["tag", "--list", "v*", "--sort=-creatordate"]).ok()?;
        tags.lines().next().map(str::to_string)
    }

    fn same_model_input(&self, a: &str, b: &str) -> Option<bool> {
        if a == b {
            return Some(true);
        }
        let names = git(&["diff", "--name-only", a, b]).ok()?;
        let pattern = model_paths().ok()?;
        let mut grep = Command::new("grep")
            .args(["-Eq", &pattern])
            .stdin(Stdio::piped())
            .spawn()
            .ok()?;
        use std::io::Write as _;
        grep.stdin.take()?.write_all(names.as_bytes()).ok()?;
        // grep -q exits 0 on a match: a model path differs.
        Some(!grep.wait().ok()?.success())
    }

    fn runs(&self, workflow: &str, which: Which) -> Result<Vec<Run>, String> {
        let limit = RUNS.to_string();
        let mut args = vec!["run", "list", "--workflow", workflow, "--limit", &limit];
        match which {
            Which::At(sha) => args.extend(["--commit", sha]),
            Which::Green => args.extend(["--status", "success"]),
        }
        args.extend(["--json", "databaseId,headSha,conclusion,status"]);
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
                    conclusion: r["conclusion"].as_str().unwrap_or("").to_string(),
                })
            })
            .collect())
    }

    fn jobs(&self, id: u64) -> Result<Vec<(String, String)>, String> {
        let json =
            output(Command::new("gh").args(["run", "view", &id.to_string(), "--json", "jobs"]))?;
        let value: serde_json::Value =
            serde_json::from_str(&json).map_err(|e| format!("gh run view {id}: {e}"))?;
        Ok(value["jobs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|j| {
                Some((
                    j["name"].as_str()?.to_string(),
                    j["conclusion"].as_str().unwrap_or("").to_string(),
                ))
            })
            .collect())
    }

    fn log(&self, id: u64) -> Result<String, String> {
        output(Command::new("gh").args(["run", "view", &id.to_string(), "--log"]))
    }
}

#[cfg(test)]
mod tests;
