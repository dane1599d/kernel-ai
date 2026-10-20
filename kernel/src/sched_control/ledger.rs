//! A finished trial in code's words: the kabi reason codes and the ledger line.

use super::Control;
use super::trial::{Candidate, Request};
use kabi::sched::{reason, state};
use sched::judge::Reason;

pub(super) fn reason_code(reason: Option<Reason>, pids: &[u64]) -> (u64, u64, u64) {
    let pid = |index: usize| pids.get(index).copied().unwrap_or(0);
    match reason {
        Some(Reason::Guardrail { process, wait_ns }) => (reason::GUARDRAIL, pid(process), wait_ns),
        Some(Reason::Ejected) => (reason::EJECTED, 0, 0),
        Some(Reason::TooFewSamples) => (reason::TOO_FEW_SAMPLES, 0, 0),
        Some(Reason::NotFaster) => (reason::NOT_FASTER, 0, 0),
        Some(Reason::PairWorse { pair }) => (reason::PAIR_WORSE, 0, pair as u64),
        Some(Reason::BackgroundTotal { permille }) => (reason::BACKGROUND_TOTAL, 0, permille),
        Some(Reason::BackgroundProcess { process, permille }) => {
            (reason::BACKGROUND_PROCESS, pid(process), permille)
        }
        Some(Reason::NotRun) | None => (reason::NOT_RUN, 0, 0),
    }
}

/// The ledger line, in code's words.
pub(super) fn describe(
    control: &Control,
    request: &Request,
    pids: &[u64],
) -> alloc::string::String {
    let s = &control.status;
    let candidate = match request.candidate {
        Candidate::Spec(spec) => alloc::format!("{}", spec.label()),
        Candidate::Test(_) => alloc::string::String::from("a test policy"),
    };
    let verdict = match s.state {
        state::KEPT => alloc::format!("kept {candidate}"),
        _ => alloc::format!(
            "reverted {candidate} (reason {}, process {}, value {})",
            s.reason,
            s.reason_pid,
            s.reason_value
        ),
    };
    alloc::format!(
        "sched: trial {} {verdict}: target pid {} p99 {} us -> {} us, background CPU {} permille, {} windows, {} processes",
        s.id,
        request.target_pid,
        s.baseline_p99_ns / 1000,
        s.candidate_p99_ns / 1000,
        s.background_permille,
        s.windows_done,
        pids.len()
    )
}
