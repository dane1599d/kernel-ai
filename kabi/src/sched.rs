//! The scheduler's numbers and trials for ring 3 (M10a): what
//! [`crate::call::SCHED_STATS`] fills in and what [`crate::call::SCHED_TRIAL`] takes.
//! Both need [`crate::cap::POLICY`], which only the shell holds.
//!
//! A trial alternates windows of the installed policy (baseline) and a candidate,
//! drops the first 200 ms after each switch, and keeps the candidate only if the
//! judge in `sched::judge` says so. It belongs to the kernel: it finishes or
//! reverts whatever happens to the process that started it.

/// Thread records in [`SchedStats`]: the scheduler's thread table.
pub const MAX_THREAD_RECORDS: usize = 64;
/// Most processes a trial may label background.
pub const MAX_BACKGROUND: usize = 8;
/// Windows a trial may run: 2 to 5 pairs.
pub const MIN_PAIRS: u64 = 2;
pub const MAX_PAIRS: u64 = 5;
pub const MAX_WINDOWS: usize = 2 * MAX_PAIRS as usize;
/// Window length bounds, in milliseconds.
pub const MIN_WINDOW_MS: u64 = 1_000;
pub const MAX_WINDOW_MS: u64 = 10_000;

/// A thread's label.
pub mod class {
    pub const NORMAL: u64 = 0;
    pub const BACKGROUND: u64 = 1;
}

/// The `classes` presets.
pub mod preset {
    pub const GENTLE: u64 = 0;
    pub const FIRM: u64 = 1;
}

/// Where a trial is.
pub mod state {
    /// No trial has run since boot.
    pub const NONE: u64 = 0;
    pub const RUNNING: u64 = 1;
    pub const KEPT: u64 = 2;
    pub const REVERTED: u64 = 3;
}

/// Why a trial was reverted ([`TrialStatus::reason`]).
pub mod reason {
    pub const NONE: u64 = 0;
    /// A runnable wait passed 100 ms in a candidate window; `reason_pid` waited
    /// `reason_value` ns.
    pub const GUARDRAIL: u64 = 1;
    /// The watchdog ejected the candidate.
    pub const EJECTED: u64 = 2;
    pub const TOO_FEW_SAMPLES: u64 = 3;
    /// The target's pooled p99 was not at most 0.8x the baseline's.
    pub const NOT_FASTER: u64 = 4;
    /// Pair `reason_value` was worse for the target.
    pub const PAIR_WORSE: u64 = 5;
    /// Background CPU fell to `reason_value` thousandths of the baseline's.
    pub const BACKGROUND_TOTAL: u64 = 6;
    /// Process `reason_pid` fell to `reason_value` thousandths of its CPU.
    pub const BACKGROUND_PROCESS: u64 = 7;
    /// The baseline was ejected or a switch was refused.
    pub const NOT_RUN: u64 = 8;
    /// A person chose a policy during the trial.
    pub const CANCELLED: u64 = 9;
    /// The process that started the trial exited.
    pub const STARTER_EXITED: u64 = 10;
    /// Process `reason_pid` was killed during the trial (a runaway, a crash or
    /// the supervisor's kill).
    pub const KILLED: u64 = 11;
    /// The target exited.
    pub const TARGET_EXITED: u64 = 12;
}

/// One thread's numbers over the window asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct ThreadRecord {
    pub tid: u64,
    /// Its process, or 0 for a kernel thread.
    pub pid: u64,
    /// A [`class`].
    pub class: u64,
    pub cpu_ns: u64,
    pub wakes: u64,
    pub preemptions: u64,
    /// Wake-to-run samples in the window, and their percentiles and maximum.
    pub samples: u64,
    pub p50_ns: u64,
    pub p99_ns: u64,
    pub max_ns: u64,
}

/// One finished window of a trial.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct WindowRecord {
    /// 1 for a candidate window.
    pub candidate: u64,
    pub target_p99_ns: u64,
    pub target_samples: u64,
    /// CPU of every process but the target, after the discarded start.
    pub background_cpu_ns: u64,
    pub max_wait_ns: u64,
    pub ejections: u64,
}

/// The latest trial, running or finished.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct TrialStatus {
    /// 0 before the first trial; counts up.
    pub id: u64,
    /// A [`state`].
    pub state: u64,
    /// The candidate: a [`crate::policy`] id and a [`preset`].
    pub policy: u64,
    pub preset: u64,
    pub target_pid: u64,
    pub windows_done: u64,
    pub windows_total: u64,
    pub window_ms: u64,
    /// A [`reason`] once reverted, with its process and value.
    pub reason: u64,
    pub reason_pid: u64,
    pub reason_value: u64,
    pub baseline_p99_ns: u64,
    pub candidate_p99_ns: u64,
    /// Candidate background CPU over the baseline's, in thousandths.
    pub background_permille: u64,
    /// The background process that lost the most CPU, and its share.
    pub worst_pid: u64,
    pub worst_permille: u64,
    pub windows: [WindowRecord; MAX_WINDOWS],
}

/// Filled in by [`crate::call::SCHED_STATS`].
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct SchedStats {
    pub cpus: u64,
    pub now_ns: u64,
    pub window_ns: u64,
    /// CPU time spent outside the idle threads, summed over CPUs.
    pub busy_ns: u64,
    /// The installed policy and preset, and since when.
    pub policy: u64,
    pub preset: u64,
    pub since_ns: u64,
    /// Processes labelled background by the installed choice.
    pub background_count: u64,
    pub background: [u64; MAX_BACKGROUND],
    pub count: u64,
    pub trial: TrialStatus,
    pub threads: [ThreadRecord; MAX_THREAD_RECORDS],
}

impl SchedStats {
    pub const fn empty() -> Self {
        Self {
            cpus: 0,
            now_ns: 0,
            window_ns: 0,
            busy_ns: 0,
            policy: 0,
            preset: 0,
            since_ns: 0,
            background_count: 0,
            background: [0; MAX_BACKGROUND],
            count: 0,
            trial: TrialStatus {
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
                windows: [WindowRecord {
                    candidate: 0,
                    target_p99_ns: 0,
                    target_samples: 0,
                    background_cpu_ns: 0,
                    max_wait_ns: 0,
                    ejections: 0,
                }; MAX_WINDOWS],
            },
            threads: [ThreadRecord {
                tid: 0,
                pid: 0,
                class: 0,
                cpu_ns: 0,
                wakes: 0,
                preemptions: 0,
                samples: 0,
                p50_ns: 0,
                p99_ns: 0,
                max_ns: 0,
            }; MAX_THREAD_RECORDS],
        }
    }

    /// The records in use.
    pub fn records(&self) -> &[ThreadRecord] {
        &self.threads[..(self.count as usize).min(MAX_THREAD_RECORDS)]
    }
}

/// Argument of [`crate::call::SCHED_TRIAL`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct TrialRequest {
    /// A [`crate::policy`] id (not [`crate::policy::OTHER`]) and a [`preset`].
    pub policy: u64,
    pub preset: u64,
    /// The process whose latency is judged: the founder's choice.
    pub target_pid: u64,
    pub window_ms: u64,
    pub pairs: u64,
    /// Processes labelled background, only with `classes`; never the target or a
    /// protected service.
    pub background_count: u64,
    pub background: [u64; MAX_BACKGROUND],
}

const _: () = {
    assert!(core::mem::size_of::<ThreadRecord>() == 10 * 8);
    assert!(core::mem::size_of::<WindowRecord>() == 6 * 8);
    assert!(core::mem::size_of::<TrialStatus>() == 16 * 8 + MAX_WINDOWS * 6 * 8);
    assert!(core::mem::size_of::<TrialRequest>() == (6 + MAX_BACKGROUND) * 8);
    assert!(
        core::mem::size_of::<SchedStats>()
            == (9 + MAX_BACKGROUND) * 8
                + core::mem::size_of::<TrialStatus>()
                + MAX_THREAD_RECORDS * 10 * 8
    );
};
