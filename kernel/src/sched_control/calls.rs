//! The system calls that reach the scheduler's control: POLICY (19), SCHED_STATS
//! (33) and SCHED_TRIAL (34). The dispatcher has checked `cap::POLICY` for all
//! three; everything else from ring 3 is checked here before it is used.

use super::trial::{self, Candidate, Request};
use super::{installed, labelable, spec_of};
use crate::process::{self, uaccess};
use crate::{hpet, log, threads};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use kabi::Error;
use kabi::sched::{MAX_BACKGROUND, SchedStats, TrialRequest};
use sched::spec::{PolicyKind, PolicySpec};

/// Fastest policy switches may follow each other.
pub const MIN_POLICY_INTERVAL_NS: u64 = 100_000_000;

/// POLICY: a person's switch, unless it is already installed or the last one was
/// too recent (a loop of switches would disturb every CPU).
pub fn policy(id: u64, preset: u64) -> Result<u64, Error> {
    static LAST_SWITCH_NS: AtomicU64 = AtomicU64::new(0);
    let preset = match id {
        kabi::policy::CLASSES => preset,
        _ => kabi::sched::preset::GENTLE,
    };
    let spec = spec_of(id, preset).ok_or(Error::Invalid)?;
    let name = spec.policy.name();
    if installed().0 == spec && threads::installed_policy_everywhere(name) {
        return Ok(0);
    }
    let now = hpet::nanos();
    let last = LAST_SWITCH_NS.load(Ordering::Relaxed);
    if last != 0 && now.saturating_sub(last) < MIN_POLICY_INTERVAL_NS {
        return Err(Error::Busy);
    }
    LAST_SWITCH_NS.store(now, Ordering::Relaxed);
    super::set_policy(spec)?;
    log!(
        "sched: policy switched to {} by a ring-3 request",
        spec.label()
    );
    Ok(0)
}

/// SCHED_STATS: copies the numbers out, returning how many threads they cover.
pub fn stats(slot: usize, out: u64, window_ms: u64) -> Result<u64, Error> {
    let stats = super::stats(slot, window_ms)?;
    // SAFETY: SchedStats is plain old data (repr(C), integers only).
    let bytes = unsafe {
        core::slice::from_raw_parts(
            (&raw const *stats).cast::<u8>(),
            core::mem::size_of::<SchedStats>(),
        )
    };
    uaccess::copy_to_user(slot, out, bytes)?;
    Ok(stats.count)
}

/// The checks every trial's spec, target and labels pass, from ring 3 or from
/// the `tune-ab` command line: labels only with `classes`, at most 8, distinct,
/// never the target, never a protected service; the target a live process that
/// is not the supervisor.
pub fn check(spec: PolicySpec, target: u64, background: &[u64]) -> Result<(), Error> {
    if background.len() > MAX_BACKGROUND
        || (!background.is_empty() && spec.policy != PolicyKind::Classes)
    {
        return Err(Error::Invalid);
    }
    match process::name_of(target) {
        Some((_, false)) => {}
        _ => return Err(Error::Invalid),
    }
    for (i, &pid) in background.iter().enumerate() {
        if pid == target || background[..i].contains(&pid) || !labelable(pid) {
            return Err(Error::Invalid);
        }
    }
    Ok(())
}

/// SCHED_TRIAL: checks the request and starts the trial, owned by the kernel from
/// then on. A label only with `classes`, at most 8, distinct, never the target,
/// never a protected service; the target a live process that is not the
/// supervisor.
pub fn trial(slot: usize, request: u64) -> Result<u64, Error> {
    let request: TrialRequest = uaccess::read_struct(slot, request)?;
    let spec = spec_of(request.policy, request.preset).ok_or(Error::Invalid)?;
    let count = usize::try_from(request.background_count).map_err(|_| Error::Invalid)?;
    let background: Vec<u64> = request
        .background
        .get(..count)
        .ok_or(Error::Invalid)?
        .to_vec();
    check(spec, request.target_pid, &background)?;
    let pairs = usize::try_from(request.pairs).map_err(|_| Error::Invalid)?;
    let starter_pid = process::identity(slot).pid;
    trial::start(Request {
        candidate: Candidate::Spec(spec),
        target_pid: request.target_pid,
        background,
        window_ns: request.window_ms.saturating_mul(1_000_000),
        pairs,
        starter_pid,
    })
}
