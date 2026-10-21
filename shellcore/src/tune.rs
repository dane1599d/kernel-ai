//! `tune [process][: intent]`: the local model proposes a scheduler change, the
//! founder confirms it, and the kernel tries it (M10a-4).
//!
//! 1. Code watches one 3 s window (SCHED_STATS) and builds the summary; the
//!    target is the process named, by default the shell itself.
//! 2. The llm service answers under the schema; [`tune::decide`] checks the
//!    answer here, so anything invalid is a keep and installs nothing.
//! 3. The proposal card fits one screen; only `y` or `yes` to its id within
//!    120 s starts the trial (SCHED_TRIAL). A choice the kernel rolled back for
//!    the same target is not proposed again for 10 minutes.
//! 4. The kernel owns the trial. The shell prints a line per window and the
//!    verdict card, in code's words; if the shell dies, the kernel reverts.
//!
//! `tune undo` puts back the policy a kept trial replaced. Labels never name
//! the shell, supervisor or llm (the summary offers none of them; the kernel
//! checks again).

use crate::{Host, Reply};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use kabi::sched::{SchedStats, TrialRequest, TrialStatus, reason, state};
use llmproto::tune::{self, Decision, Label, Observation, ProcessRow, Summary, two_sig};
use sched::judge::{Reason, Verdict};
use sched::spec::{PolicyKind, PolicySpec, Preset};

/// The observe window, the trial's windows and its pairs.
pub const OBSERVE_MS: u64 = 3_000;
pub const WINDOW_MS: u64 = 3_000;
pub const PAIRS: u64 = 3;
/// How long a proposal waits for its yes, and a rolled-back choice is not
/// proposed again.
const CONFIRM_NS: u64 = 120_000_000_000;
const BAN_NS: u64 = 600_000_000_000;
/// How often the shell looks at a running trial.
const POLL_MS: u64 = 250;
/// Services no label may name.
pub const PROTECTED: [&str; 3] = ["shell", "supervisor", "llm"];

/// A proposal waiting for its yes.
struct Proposal {
    id: u64,
    created_ns: u64,
    target: String,
    target_pid: u64,
    spec: PolicySpec,
    /// Labelled names and their pids.
    background: Vec<(String, u64)>,
    /// Every process by pid, with the summary's name.
    names: Vec<(u64, String)>,
    previous: (u64, u64),
}

/// A choice the kernel rolled back.
struct Ban {
    target: String,
    spec: PolicySpec,
    background: Vec<String>,
    at_ns: u64,
}

#[derive(Default)]
pub(crate) struct Tuner {
    pending: Option<Proposal>,
    bans: Vec<Ban>,
    /// The policy a kept trial replaced, for `tune undo`.
    undo: Option<(u64, u64, String)>,
    /// The last verdicts, for the summary.
    verdicts: Vec<String>,
}

pub(crate) fn spec_of(policy: u64, preset: u64) -> PolicySpec {
    let kind = match policy {
        kabi::policy::ROUND_ROBIN => PolicyKind::RoundRobin,
        kabi::policy::CLASSES => PolicyKind::Classes,
        kabi::policy::RESPONSIVE => PolicyKind::Responsive,
        _ => PolicyKind::Interactive,
    };
    let preset = match preset {
        kabi::sched::preset::FIRM => Preset::Firm,
        _ => Preset::Gentle,
    };
    PolicySpec::new(kind, preset)
}

fn ids_of(spec: PolicySpec) -> (u64, u64) {
    let policy = match spec.policy {
        PolicyKind::Interactive => kabi::policy::INTERACTIVE,
        PolicyKind::RoundRobin => kabi::policy::ROUND_ROBIN,
        PolicyKind::Classes => kabi::policy::CLASSES,
        PolicyKind::Responsive => kabi::policy::RESPONSIVE,
    };
    let preset = match spec.preset {
        Preset::Gentle => kabi::sched::preset::GENTLE,
        Preset::Firm => kabi::sched::preset::FIRM,
    };
    (policy, preset)
}

/// What one window showed: the rows, with each row's pid.
struct Watched {
    observation: Observation,
    pids: Vec<u64>,
    background: Vec<String>,
    previous: (u64, u64),
}

fn observe(
    host: &dyn Host,
    target: &str,
    intent: Option<String>,
    verdicts: &[String],
) -> Result<Watched, String> {
    let metrics = host
        .metrics()
        .map_err(|e| format!("cannot read processes: {e:?}"))?;
    let stats: Box<SchedStats> = host
        .sched_stats(OBSERVE_MS)
        .map_err(|e| format!("cannot watch the scheduler: {e:?}"))?;
    let window = stats.window_ns.max(1);
    let listed =
        &metrics.processes[..(metrics.process_count as usize).min(metrics.processes.len())];
    let mut rows = Vec::new();
    let mut pids = Vec::new();
    let labelled =
        &stats.background[..(stats.background_count as usize).min(stats.background.len())];
    let mut background = Vec::new();
    for process in listed {
        let threads: Vec<_> = stats
            .records()
            .iter()
            .filter(|t| t.pid == process.pid)
            .collect();
        let sum =
            |f: fn(&kabi::sched::ThreadRecord) -> u64| threads.iter().map(|t| f(t)).sum::<u64>();
        let max = |f: fn(&kabi::sched::ThreadRecord) -> u64| {
            threads.iter().map(|t| f(t)).max().unwrap_or(0)
        };
        let (cpu, wakes) = (sum(|t| t.cpu_ns), sum(|t| t.wakes));
        let name = process.name().to_string();
        let label = match labelled.contains(&process.pid) {
            true => {
                background.push(tune::sanitize(&name));
                Label::Background
            }
            false => Label::Normal,
        };
        rows.push(ProcessRow {
            protected: PROTECTED.contains(&name.as_str()),
            name,
            threads: threads.len().max(1) as u32,
            cpu_permille: cpu * 1000 / window,
            wakes_per_s: wakes * 1_000_000_000 / window,
            mean_burst_us: cpu / wakes.max(1) / 1000,
            wait_p50_us: max(|t| t.p50_ns) / 1000,
            wait_p99_us: max(|t| t.p99_ns) / 1000,
            wait_max_us: max(|t| t.max_ns) / 1000,
            label,
        });
        pids.push(process.pid);
    }
    let target = rows
        .iter()
        .position(|row| row.name == target)
        .ok_or_else(|| format!("no process named \"{target}\" is running"))?;
    let cpus = stats.cpus.max(1);
    let observation = Observation {
        target,
        intent,
        current: spec_of(stats.policy, stats.preset),
        since_s: stats.now_ns.saturating_sub(stats.since_ns) / 1_000_000_000,
        verdicts: verdicts.iter().rev().take(3).rev().cloned().collect(),
        cpus: cpus as u32,
        busy_permille: stats.busy_ns * 1000 / (window * cpus),
        window_ms: window / 1_000_000,
        processes: rows,
    };
    Ok(Watched {
        observation,
        pids,
        background,
        previous: (stats.policy, stats.preset),
    })
}

/// `p99 6.1 ms` style numbers.
fn ms(ns: u64) -> String {
    two_sig(ns / 1000, 3)
}

impl Tuner {
    /// A line other than the answer came: the proposal lapses.
    pub(crate) fn discard(&mut self) {
        self.pending = None;
    }

    /// Whether a proposal waits for its answer.
    pub(crate) fn proposing(&self) -> bool {
        self.pending.is_some()
    }

    pub(crate) fn waiting(&self, id: u64) -> bool {
        self.pending.as_ref().is_some_and(|p| p.id == id)
    }

    /// `tune [process][: intent]` or `tune undo`, as proposal `id`.
    pub(crate) fn command(&mut self, host: &dyn Host, args: &str, id: u64) -> Reply {
        self.pending = None;
        let args = args.trim();
        if args == "undo" {
            return self.undo(host);
        }
        let (target, intent) = match args.split_once(':') {
            Some((target, intent)) => (target.trim(), Some(intent.trim())),
            None => (args, None),
        };
        let target = if target.is_empty() { "shell" } else { target };
        let intent = intent
            .filter(|i| !i.is_empty())
            .map(|i| i.chars().take(tune::INTENT_MAX_CHARS).collect::<String>());
        host.audit(&format!(
            "tune {id}: watching \"{target}\" for {} s",
            OBSERVE_MS / 1000
        ));
        let watched = match observe(host, target, intent, &self.verdicts) {
            Ok(watched) => watched,
            Err(why) => return Reply::err(why),
        };
        let Some(summary) = tune::summary(&watched.observation) else {
            return Reply::err("the summary could not be built");
        };
        let completion = match host.tune(&summary.wire()) {
            Ok(completion) => completion,
            Err(why) => return Reply::err(format!("the model did not answer: {why}")),
        };
        let decision = tune::decide(
            &completion,
            &summary,
            watched.observation.current,
            &watched.background,
        );
        let measured = tune::measured(&watched.observation, &summary);
        let mut lines = tune::card(&summary, &measured, &decision, id as u32);
        let Decision::Try {
            spec, background, ..
        } = &decision
        else {
            host.audit(&format!("tune {id}: kept the current policy"));
            return Reply::ok(lines.join("\n"));
        };
        let now = host.now_ns();
        let mut sorted = background.clone();
        sorted.sort();
        if let Some(ban) = self.bans.iter().find(|ban| {
            ban.target == summary.target_name
                && ban.spec == *spec
                && ban.background == sorted
                && now.saturating_sub(ban.at_ns) < BAN_NS
        }) {
            let minutes = (BAN_NS - now.saturating_sub(ban.at_ns)).div_ceil(60_000_000_000);
            lines.truncate(lines.len().saturating_sub(1));
            lines.push(format!(
                "Not proposed: it was rolled back for \"{}\"; again in {minutes} min.",
                summary.target_name
            ));
            return Reply::ok(lines.join("\n"));
        }
        self.pending = Some(self.proposal(id, now, &summary, &watched, *spec, background));
        host.audit(&format!("tune {id}: proposed {}", spec.label()));
        Reply {
            ok: true,
            text: lines.join("\n"),
            confirm: Some(id),
        }
    }

    fn proposal(
        &self,
        id: u64,
        now: u64,
        summary: &Summary,
        watched: &Watched,
        spec: PolicySpec,
        background: &[String],
    ) -> Proposal {
        let names = tune::unique_names(&watched.observation.processes);
        let pid_of = |name: &str| {
            summary
                .index_of(name)
                .and_then(|index| watched.pids.get(index).copied())
                .unwrap_or(0)
        };
        Proposal {
            id,
            created_ns: now,
            target: summary.target_name.clone(),
            target_pid: watched.pids[summary.target],
            spec,
            background: background.iter().map(|n| (n.clone(), pid_of(n))).collect(),
            names: watched.pids.iter().copied().zip(names).collect(),
            previous: watched.previous,
        }
    }

    /// `answer <id> <text>` for a waiting proposal.
    pub(crate) fn answer(&mut self, host: &dyn Host, text: &str) -> Reply {
        let Some(proposal) = self.pending.take() else {
            return Reply::err("nothing to confirm");
        };
        if host.now_ns().saturating_sub(proposal.created_ns) > CONFIRM_NS {
            return Reply::err("the proposal expired; run tune again");
        }
        if !matches!(text.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            host.audit(&format!("tune {}: declined", proposal.id));
            return Reply::ok(format!("proposal {} not tried", proposal.id));
        }
        let (policy, preset) = ids_of(proposal.spec);
        let mut request = TrialRequest {
            policy,
            preset,
            target_pid: proposal.target_pid,
            window_ms: WINDOW_MS,
            pairs: PAIRS,
            background_count: proposal.background.len() as u64,
            ..TrialRequest::default()
        };
        for (slot, (_, pid)) in request.background.iter_mut().zip(&proposal.background) {
            *slot = *pid;
        }
        let trial = match host.sched_trial(&request) {
            Ok(trial) => trial,
            Err(error) => return Reply::err(format!("the kernel refused the trial: {error:?}")),
        };
        host.audit(&format!("tune {}: trial {trial} started", proposal.id));
        match self.watch(host, trial, &proposal) {
            Ok(status) => self.verdict(host, &status, &proposal),
            Err(why) => Reply::err(why),
        }
    }

    /// Prints a line per finished window until the trial ends.
    fn watch(&self, host: &dyn Host, trial: u64, p: &Proposal) -> Result<TrialStatus, String> {
        let limit = host.now_ns() + (2 * PAIRS * WINDOW_MS + 30_000) * 1_000_000;
        let mut shown = 0;
        loop {
            host.sleep_ms(POLL_MS);
            let stats = host
                .sched_stats(0)
                .map_err(|e| format!("cannot read the trial: {e:?}"))?;
            let status = stats.trial;
            if status.id != trial {
                return Err(String::from("another trial replaced this one"));
            }
            while shown < (status.windows_done as usize).min(status.windows.len()) {
                host.audit(&window_line(&status, shown, &p.target));
                shown += 1;
            }
            if status.state != state::RUNNING {
                return Ok(status);
            }
            if host.interrupted() {
                return Err(String::from(
                    "stopped watching; the kernel finishes or reverts the trial on its own",
                ));
            }
            if host.now_ns() > limit {
                return Err(String::from("the trial did not end in time"));
            }
        }
    }

    fn verdict(&mut self, host: &dyn Host, status: &TrialStatus, p: &Proposal) -> Reply {
        let names: Vec<&str> = p.names.iter().map(|(_, n)| n.as_str()).collect();
        let index = |pid: u64| {
            p.names
                .iter()
                .position(|(q, _)| *q == pid)
                .unwrap_or(usize::MAX)
        };
        let worst =
            (status.worst_pid != 0).then(|| (index(status.worst_pid), status.worst_permille));
        let judged = |reason| Verdict {
            kept: status.state == state::KEPT,
            reason,
            baseline_p99_ns: status.baseline_p99_ns,
            candidate_p99_ns: status.candidate_p99_ns,
            background_permille: status.background_permille,
            worst_process: worst,
        };
        let value = status.reason_value;
        let pid = status.reason_pid;
        let reason = match status.reason {
            _ if status.state == state::KEPT => None,
            reason::GUARDRAIL => Some(Reason::Guardrail {
                process: index(pid),
                wait_ns: value,
            }),
            reason::EJECTED => Some(Reason::Ejected),
            reason::TOO_FEW_SAMPLES => Some(Reason::TooFewSamples),
            reason::NOT_FASTER => Some(Reason::NotFaster),
            reason::PAIR_WORSE => Some(Reason::PairWorse {
                pair: value as usize,
            }),
            reason::BACKGROUND_TOTAL => Some(Reason::BackgroundTotal { permille: value }),
            reason::BACKGROUND_PROCESS => Some(Reason::BackgroundProcess {
                process: index(pid),
                permille: value,
            }),
            reason::NOT_RUN => Some(Reason::NotRun),
            other => {
                let why = match other {
                    reason::CANCELLED => {
                        String::from("a policy was chosen by hand during the trial")
                    }
                    reason::STARTER_EXITED => String::from("the shell that started it exited"),
                    reason::TARGET_EXITED => format!("\"{}\" exited", p.target),
                    _ => format!(
                        "\"{}\" was killed during the trial",
                        names.get(index(pid)).copied().unwrap_or("a process")
                    ),
                };
                let line = format!("Rolled back {} at once: {why}.", p.spec.label());
                return self.finish(host, p, false, line);
            }
        };
        let lines = tune::verdict_card(p.spec, &names, &p.target, &judged(reason));
        self.finish(host, p, status.state == state::KEPT, lines.join("\n"))
    }

    fn finish(&mut self, host: &dyn Host, p: &Proposal, kept: bool, text: String) -> Reply {
        let mut background: Vec<String> = p.background.iter().map(|(n, _)| n.clone()).collect();
        background.sort();
        let short = match kept {
            true => {
                let before = spec_of(p.previous.0, p.previous.1);
                self.undo = Some((p.previous.0, p.previous.1, format!("{}", before.label())));
                format!("kept {} for \"{}\"", p.spec.label(), p.target)
            }
            false => {
                self.bans.push(Ban {
                    target: p.target.clone(),
                    spec: p.spec,
                    background,
                    at_ns: host.now_ns(),
                });
                format!("rolled back {} for \"{}\"", p.spec.label(), p.target)
            }
        };
        host.audit(&format!("tune {}: {short}", p.id));
        self.verdicts.push(short);
        if self.verdicts.len() > 3 {
            self.verdicts.remove(0);
        }
        Reply::ok(text)
    }

    fn undo(&mut self, host: &dyn Host) -> Reply {
        let Some((policy, preset, label)) = self.undo.take() else {
            return Reply::err("nothing to undo: no trial kept a policy");
        };
        match host.set_spec(policy, preset) {
            Ok(()) => Reply::ok(format!("back to {label}")),
            Err(error) => {
                self.undo = Some((policy, preset, label));
                Reply::err(format!("cannot switch back: {error:?}"))
            }
        }
    }
}

/// `window 4/6 candidate: "editor" p99 0.4 ms, background CPU 97%`.
fn window_line(status: &TrialStatus, index: usize, target: &str) -> String {
    let w = &status.windows[index];
    let side = if w.candidate == 1 {
        "candidate"
    } else {
        "baseline"
    };
    let share = match (w.candidate, index.checked_sub(1)) {
        (1, Some(base)) => {
            let base = status.windows[base].background_cpu_ns.max(1);
            format!(
                ", background CPU {}%",
                two_sig(w.background_cpu_ns * 1000 / base, 1)
            )
        }
        _ => String::new(),
    };
    format!(
        "window {}/{} {side}: \"{target}\" p99 {} ms{share}",
        index + 1,
        status.windows_total,
        ms(w.target_p99_ns)
    )
}
