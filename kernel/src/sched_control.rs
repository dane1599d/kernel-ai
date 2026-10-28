//! Who decides the scheduling policy (M10a): the installed spec and its labels,
//! a person's switch (call 19 and `:policy`), the scheduler's numbers for ring 3
//! (SCHED_STATS) and kernel-owned trials (SCHED_TRIAL, in `trial`).
//!
//! Labels are per process: the pids the installed choice labels background.
//! Each CPU's `sched::Core` holds them per thread, so every Core gets the same
//! table, and a thread that starts gets its process's label on every Core (a
//! thread slot is reused, and a thread may migrate). During a trial a new thread
//! starts unlabelled in both slots: a label only demotes, so that is the safe side.
//!
//! Lock order: `CONTROL`, then a CPU's core (through `threads::with_core`). The
//! timer tick never takes `CONTROL`.

pub mod calls;
pub mod guard;
mod ledger;
mod measure;

use crate::process::{self, Name};
use crate::{hpet, metrics, quiet, smp, threads};
use alloc::boxed::Box;
use alloc::vec::Vec;
use kabi::sched::{MAX_BACKGROUND, SchedStats, TrialStatus};
use kabi::{Error, ExitReason};
use sched::spec::{PolicyKind, PolicySpec, Preset};
use sched::{Class, SchedPolicy, Tid};
use spin::Mutex;

/// Services a label may never name, whatever asks.
pub const PROTECTED: [&str; 3] = ["shell", "supervisor", "llm"];

pub(crate) struct Control {
    /// The choice in force: a person's or a kept trial's.
    installed: PolicySpec,
    since_ns: u64,
    background: Vec<u64>,
    /// The latest trial.
    status: TrialStatus,
    /// Workload processes killed as runaways since boot, for the tests.
    runaways: u64,
    active: Option<trial::Active>,
    /// The latest finished trial's windows, for the tests to judge again.
    last_pairs: Vec<sched::judge::Pair>,
}

static CONTROL: Mutex<Control> = Mutex::new(Control {
    installed: PolicySpec::DEFAULT,
    since_ns: 0,
    background: Vec::new(),
    status: TrialStatus {
        id: 0,
        state: 0,
        policy: 0,
        preset: 0,
        target_pid: 0,
        windows_done: 0,
        windows_total: 0,
        window_ms: 0,
        reason: 0,
        reason_pid: 0,
        reason_value: 0,
        baseline_p99_ns: 0,
        candidate_p99_ns: 0,
        background_permille: 0,
        worst_pid: 0,
        worst_permille: 0,
        windows: [kabi::sched::WindowRecord {
            candidate: 0,
            target_p99_ns: 0,
            target_samples: 0,
            background_cpu_ns: 0,
            max_wait_ns: 0,
            ejections: 0,
        }; kabi::sched::MAX_WINDOWS],
    },
    runaways: 0,
    active: None,
    last_pairs: Vec::new(),
});

/// Runs `f` on the control state with preemption off.
pub(crate) fn with<R>(f: impl FnOnce(&mut Control) -> R) -> R {
    threads::preempt_disable();
    let result = f(&mut CONTROL.lock());
    threads::preempt_enable();
    result
}

/// A spec's [`kabi::policy`] id and [`kabi::sched::preset`].
pub fn ids(spec: PolicySpec) -> (u64, u64) {
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

/// The spec for a [`kabi::policy`] id and preset; `None` outside the catalog.
pub fn spec_of(policy: u64, preset: u64) -> Option<PolicySpec> {
    let kind = match policy {
        kabi::policy::INTERACTIVE => PolicyKind::Interactive,
        kabi::policy::ROUND_ROBIN => PolicyKind::RoundRobin,
        kabi::policy::CLASSES => PolicyKind::Classes,
        kabi::policy::RESPONSIVE => PolicyKind::Responsive,
        _ => return None,
    };
    let preset = match preset {
        kabi::sched::preset::GENTLE => Preset::Gentle,
        kabi::sched::preset::FIRM => Preset::Firm,
        _ => return None,
    };
    let spec = PolicySpec::new(kind, preset);
    spec.validate().ok().map(|()| spec)
}

/// The [`kabi::policy`] id of a policy by name, for METRICS: a policy outside the
/// catalog (a test's) is OTHER, not interactive.
pub fn policy_id(name: &str) -> u64 {
    PolicyKind::from_name(name).map_or(kabi::policy::OTHER, |kind| {
        ids(PolicySpec::new(kind, Preset::Gentle)).0
    })
}

/// The installed choice.
pub fn installed() -> (PolicySpec, Vec<u64>) {
    with(|control| (control.installed, control.background.clone()))
}

/// Whether `pid` may be labelled background: a live process that is not a
/// protected service.
pub fn labelable(pid: u64) -> bool {
    process::name_of(pid).is_some_and(|(name, supervisor)| !supervisor && !protected(&name))
}

fn protected(name: &Name) -> bool {
    PROTECTED.contains(&name.as_str())
}

/// The label table for `background`: every thread of those processes.
pub(crate) fn class_table(background: &[u64]) -> [Class; sched::MAX_THREADS] {
    let pids = process::pids_by_tid();
    let mut table = [Class::Normal; sched::MAX_THREADS];
    for (tid, pid) in pids.iter().enumerate() {
        if *pid != 0 && background.contains(pid) {
            table[tid] = Class::Background;
        }
    }
    table
}

/// A person's choice: ends any trial (the person wins) and installs `spec` on
/// every CPU, keeping the installed labels. Allocates; thread context only.
pub fn set_policy(spec: PolicySpec) -> Result<(), Error> {
    let fresh: Vec<Box<dyn SchedPolicy>> = (0..smp::online())
        .map(|_| spec.build())
        .collect::<Result<_, _>>()
        .map_err(|_| Error::Invalid)?;
    let mut old = Vec::new();
    let cancelled = with(|control| {
        let cancelled = trial::cancel(control, &mut old);
        for (cpu, policy) in fresh.into_iter().enumerate() {
            threads::with_core(cpu, |core, _| {
                old.push(core.set_policy(policy));
                old.extend(core.take_standby());
            });
        }
        control.installed = spec;
        control.since_ns = hpet::nanos();
        cancelled
    });
    drop(old);
    if cancelled {
        trial::wake_driver();
    }
    metrics::invalidate();
    quiet!("sched: policy set to {}", spec.label());
    Ok(())
}

/// Installs `spec` with `background` labelled, as a kept trial would, for the M10
/// exit test's candidate boot: `set_policy`, then the labels on every thread of
/// those processes on every CPU. Thread context only.
pub fn install(spec: PolicySpec, background: Vec<u64>) -> Result<(), Error> {
    set_policy(spec)?;
    with(|control| {
        let table = class_table(&background);
        for cpu in 0..smp::online() {
            threads::with_core(cpu, |core, _| {
                for (tid, class) in table.iter().enumerate() {
                    core.clear_class(tid);
                    if *class == Class::Background {
                        core.set_class(tid, Class::Background);
                    }
                }
            });
        }
        control.background = background;
    });
    metrics::invalidate();
    Ok(())
}

/// A profile's class for process `pid` (PROC_CLASS_SET): `background` or not
/// from now on, every thread on every CPU, kept for its threads to come.
/// Busy while a trial runs, which owns the labels until it ends.
pub fn label(pid: u64, background: bool) -> Result<(), Error> {
    // Outside every lock: it reads the process table, and allocates nothing.
    let by_tid = process::pids_by_tid();
    with(|control| {
        if trial::running(control) {
            return Err(Error::Busy);
        }
        control.background.retain(|&p| p != pid);
        if background {
            control.background.push(pid);
        }
        for cpu in 0..smp::online() {
            threads::with_core(cpu, |core, _| {
                for tid in (0..by_tid.len()).filter(|&tid| by_tid[tid] == pid) {
                    core.clear_class(tid);
                    if background {
                        core.set_class(tid, Class::Background);
                    }
                }
            });
        }
        Ok(())
    })?;
    metrics::invalidate();
    Ok(())
}

/// A thread of process `pid` starts: it gets the process's installed label on
/// every CPU, or none while a trial runs.
pub fn on_thread_start(tid: Tid, pid: u64) {
    with(|control| {
        let label = !trial::running(control) && control.background.contains(&pid);
        for cpu in 0..smp::online() {
            threads::with_core(cpu, |core, _| {
                core.clear_class(tid);
                if label {
                    core.set_class(tid, Class::Background);
                }
            });
        }
    });
}

/// Process `pid` is gone: it leaves the labels, and a trial it started, judged
/// or was killed in reverts at once.
pub fn on_process_exit(pid: u64, reason: ExitReason) {
    let ended = with(|control| {
        control.background.retain(|&p| p != pid);
        if reason == ExitReason::Runaway {
            control.runaways += 1;
        }
        trial::on_process_exit(control, pid, reason)
    });
    if ended {
        trial::wake_driver();
    }
}

/// The latest finished trial's windows.
pub fn last_pairs() -> Vec<sched::judge::Pair> {
    with(|control| control.last_pairs.clone())
}

/// Workload processes killed as runaways since boot.
pub fn runaways() -> u64 {
    with(|control| control.runaways)
}

/// The latest trial's status.
pub fn status() -> TrialStatus {
    with(|control| control.status)
}

/// SCHED_STATS for process slot `slot`: the header now, then every thread over
/// `window_ms`, watched from the calling thread.
pub fn stats(slot: usize, window_ms: u64) -> Result<Box<SchedStats>, Error> {
    if window_ms > kabi::sched::MAX_WINDOW_MS {
        return Err(Error::Invalid);
    }
    let mut out = Box::new(SchedStats::empty());
    with(|control| {
        let (policy, preset) = ids(control.installed);
        out.policy = policy;
        out.preset = preset;
        out.since_ns = control.since_ns;
        for (slot, pid) in out.background.iter_mut().zip(&control.background) {
            *slot = *pid;
        }
        out.background_count = control.background.len().min(MAX_BACKGROUND) as u64;
        out.trial = control.status;
    });
    out.cpus = smp::online() as u64;
    out.now_ns = hpet::nanos();
    if window_ms > 0 {
        measure::observe(slot, window_ms * 1_000_000, &mut out)?;
    }
    Ok(out)
}

pub mod trial;
