//! The M9a exit test, as data. The kernel runs it in QEMU (`test=live`) against the
//! real `notes` host; xtask runs it on the host first, against the same versions on
//! wasmi with the ledger standing in for the kernel. A step checks a reply, or what
//! the ledger and the store recorded, never what a program printed.
//!
//! A. Version 1 builds some state. S. A control request from a process is refused.
//! B. A live swap to version 2 migrates the state to a new schema. C. Four broken
//! versions each roll back by themselves, and a failed one needs `force`. D. A
//! version that cannot read the state is refused. E. A host killed while serving
//! comes back with its data. F. A host killed during probation comes back on the
//! active version.

use crate::Reason;
use crate::ledger::Ledger;
use alloc::format;
use alloc::string::String;

#[derive(Clone, Copy, Debug)]
pub enum Expect {
    Exact(&'static str),
    StartsWith(&'static str),
    Contains(&'static str),
}

impl Expect {
    fn matches(self, reply: &str) -> bool {
        match self {
            Self::Exact(text) => reply == text,
            Self::StartsWith(text) => reply.starts_with(text),
            Self::Contains(text) => reply.contains(text),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Count {
    Restores,
    Checkpoints,
    Begins,
    Commits,
    Rollbacks,
    RefusedBad,
    Served,
    Regressions,
    Generation,
    Rejected,
    Because(Reason),
}

impl Count {
    fn read(self, ledger: &Ledger) -> u64 {
        let c = &ledger.counters;
        match self {
            Self::Restores => c.restores,
            Self::Checkpoints => c.checkpoints,
            Self::Begins => c.begins,
            Self::Commits => c.commits,
            Self::Rollbacks => c.rollbacks,
            Self::RefusedBad => c.refused_bad,
            Self::Served => c.served,
            Self::Regressions => c.regressions,
            Self::Generation => ledger.generation,
            Self::Rejected => c.rejected,
            Self::Because(reason) => c.by_reason[reason as usize],
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Step {
    /// A part of the test, for the log.
    Part(&'static str),
    /// A request from the kernel.
    Call(&'static str, Expect),
    /// A request from some other process.
    Spoof(&'static str, Expect),
    /// Ledger counts.
    Counts(&'static [(Count, u64)]),
    /// The active version and its schema.
    Active(&'static str, u64),
    /// Whether a version is on the failed list.
    Bad(&'static str, bool),
    /// How often the store handed out each version.
    Fetches(&'static [(&'static str, u64)]),
    /// Still the host process from the start, or from the last restart.
    SameHost,
    /// Kill the host; the supervisor starts a new one.
    Restart,
}

/// What the scenario can do and see.
pub trait Driver {
    fn call(&mut self, request: &str) -> Result<String, String>;
    fn spoof(&mut self, request: &str) -> Result<String, String>;
    fn ledger(&mut self) -> Ledger;
    fn fetches(&mut self, label: &str) -> u64;
    fn host_pid(&mut self) -> u64;
    fn restart(&mut self) -> Result<(), String>;
    fn log(&mut self, text: &str);
}

use Count::*;
use Expect::*;
use Step::*;

pub const STEPS: &[Step] = &[
    Part("A: version 1 builds state"),
    Call("about", Exact("ok notes 1 schema 1")),
    Call("add buy milk", Exact("ok 1")),
    Call("add call bob today", Exact("ok 2")),
    Call("add fix the roof", Exact("ok 3")),
    Call("add walk", Exact("ok 4")),
    Call("count", Exact("ok 4")),
    Active("1", 1),
    Counts(&[(Restores, 1), (Checkpoints, 4)]),
    Part("S: control comes only from the kernel"),
    Spoof(
        "ctl swap 2",
        Exact("err control requests come only from the kernel"),
    ),
    Counts(&[(Begins, 0)]),
    Part("B: a live swap to version 2 migrates the state"),
    Call("ctl swap 2", Exact("ok probation 2 (3 calls)")),
    Call("count", Exact("ok 4 notes, 9 words")),
    Call("find roof", Exact("ok 3")),
    Call("add paint fence", Exact("ok 5")),
    Active("2", 2),
    Counts(&[(Commits, 1), (Generation, 1), (Served, 3)]),
    Call("get 2", Exact("ok call bob today")),
    Call("ctl source", Contains("components/notes/src/migrate.rs")),
    Part("C: broken versions roll back by themselves"),
    Call(
        "ctl swap 3-import",
        StartsWith("err rolled back 3-import: link"),
    ),
    Call(
        "ctl swap 3-health",
        StartsWith("err rolled back 3-health: health"),
    ),
    Call("count", Exact("ok 5 notes, 11 words")),
    Call("ctl swap 3-trap", Exact("ok probation 3-trap (3 calls)")),
    Call("add sixth note", Exact("ok 6")),
    // Version 3-trap overruns its array on the seventh note; 2 answers instead.
    Call("add seventh", Exact("ok 7")),
    Call("about", Exact("ok notes 2 schema 2")),
    Call("ctl swap 3-spin", Exact("ok probation 3-spin (3 calls)")),
    // Version 3-spin loops until its fuel runs out; 2 answers instead.
    Call("find nothing-matches", Exact("ok none")),
    Call("count", Exact("ok 7 notes, 14 words")),
    Call(
        "ctl swap 3-trap",
        Exact("err 3-trap failed before; use ctl swap 3-trap force"),
    ),
    Counts(&[
        (Begins, 5),
        (Rollbacks, 4),
        (Because(Reason::Link), 1),
        (Because(Reason::Health), 1),
        (Because(Reason::Trap), 1),
        (Because(Reason::Fuel), 1),
        (RefusedBad, 1),
        (Served, 4),
        (Regressions, 0),
    ]),
    Active("2", 2),
    Bad("3-trap", true),
    Bad("3-import", true),
    Part("D: a version that cannot read the state is refused"),
    Call("ctl swap 1", StartsWith("err rolled back 1: no-migration")),
    Counts(&[
        (Rollbacks, 5),
        (Because(Reason::NoMigration), 1),
        (Rejected, 0),
    ]),
    Bad("1", false),
    Fetches(&[
        ("1", 2),
        ("2", 1),
        ("3-import", 1),
        ("3-health", 1),
        ("3-trap", 2),
        ("3-spin", 1),
    ]),
    SameHost,
    Part("E: a host killed while serving comes back with its data"),
    Restart,
    Call("about", Exact("ok notes 2 schema 2")),
    Call("count", Exact("ok 7 notes, 14 words")),
    Counts(&[(Restores, 2)]),
    Fetches(&[("2", 2)]),
    Part("F: a host killed during probation comes back on the active version"),
    Call(
        "ctl swap 3-spin force",
        Exact("ok probation 3-spin (3 calls)"),
    ),
    Restart,
    Counts(&[
        (Rollbacks, 6),
        (Because(Reason::HostRestart), 1),
        (Begins, 7),
        (Restores, 3),
        (Rejected, 0),
    ]),
    Active("2", 2),
    Call("count", Exact("ok 7 notes, 14 words")),
    SameHost,
];

/// Runs every step; the first that fails ends the run with what went wrong.
pub fn run(driver: &mut dyn Driver) -> Result<(), String> {
    let mut host = driver.host_pid();
    for (index, step) in STEPS.iter().enumerate() {
        let fail = |why: String| format!("live scenario step {}: {why}", index + 1);
        match *step {
            Part(name) => driver.log(&format!("live: {name}")),
            Call(request, expect) | Spoof(request, expect) => {
                let reply = match step {
                    Call(..) => driver.call(request),
                    _ => driver.spoof(request),
                }
                .map_err(fail)?;
                if !expect.matches(&reply) {
                    return Err(fail(format!(
                        "{request:?} answered {reply:?}, expected {expect:?}"
                    )));
                }
            }
            Counts(counts) => {
                let ledger = driver.ledger();
                for (count, wanted) in counts {
                    let found = count.read(&ledger);
                    if found != *wanted {
                        return Err(fail(format!("{count:?} is {found}, expected {wanted}")));
                    }
                }
            }
            Active(label, schema) => {
                let ledger = driver.ledger();
                if ledger.active() != Some(label)
                    || ledger.schema != schema
                    || ledger.candidate().is_some()
                {
                    return Err(fail(format!(
                        "active {:?} schema {} (swapping to {:?}), expected {label} schema {schema}",
                        ledger.active(),
                        ledger.schema,
                        ledger.candidate()
                    )));
                }
            }
            Bad(label, wanted) => {
                if driver.ledger().is_bad(label).is_some() != wanted {
                    return Err(fail(format!(
                        "{label} on the failed list should be {wanted}"
                    )));
                }
            }
            Fetches(fetches) => {
                for (label, wanted) in fetches {
                    let found = driver.fetches(label);
                    if found != *wanted {
                        return Err(fail(format!(
                            "version {label} fetched {found} times, expected {wanted}"
                        )));
                    }
                }
            }
            SameHost => {
                let now = driver.host_pid();
                if now != host {
                    return Err(fail(format!("the host changed from pid {host} to {now}")));
                }
            }
            Restart => {
                driver.restart().map_err(fail)?;
                let now = driver.host_pid();
                if now == host {
                    return Err(fail(format!(
                        "the host is still pid {host} after the restart"
                    )));
                }
                host = now;
            }
        }
    }
    Ok(())
}
