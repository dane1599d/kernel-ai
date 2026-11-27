//! What an exit leads to (the restart policy) and the state the supervisor
//! saves in the kernel-held region, so a restarted supervisor carries on
//! where it left off.

use crate::*;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// What a service's exit leads to before its restart policy is asked: the
/// supervisor's own reasons for an exit, which neither restart it nor count
/// as a failure, come first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
    /// pkg withdrew it: it waits for a version.
    Withdrawn,
    /// An undo of a start that found it waiting on demand ended it: it waits
    /// on demand again, with no restart and none counted (#1181's review).
    BackOnDemand,
    /// A restart was asked for: it starts again at once.
    RestartOnRequest,
    /// It was stopped: it stays down.
    Stopped,
    /// None of those: its restart policy decides ([`Record::on_exit`]).
    Policy,
}

/// [`Ending`] for an exit, from the supervisor's flags on the service.
pub fn ending(
    withdrawn: bool,
    back_on_demand: bool,
    restart_on_exit: bool,
    stopped: bool,
) -> Ending {
    match (withdrawn, back_on_demand, restart_on_exit, stopped) {
        (true, ..) => Ending::Withdrawn,
        (_, true, ..) => Ending::BackOnDemand,
        // A stop wins over a restart asked for: a service whose record says
        // stopped never starts again by itself (H2-14). The supervisor also
        // clears the restart flag on a stop, so the later request wins.
        (_, _, _, true) => Ending::Stopped,
        (_, _, true, _) => Ending::RestartOnRequest,
        _ => Ending::Policy,
    }
}

/// When to restart a service that ended, or whether to stop trying.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    RestartAfterMs(u64),
    GiveUp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    pub first_backoff_ms: u64,
    pub max_backoff_ms: u64,
    /// A run at least this long resets the backoff.
    pub healthy_run_ms: u64,
    /// More than `max_restarts` restarts within `window_ms` and the service is given up.
    pub window_ms: u64,
    pub max_restarts: usize,
}

pub const POLICY: Policy = Policy {
    first_backoff_ms: 50,
    max_backoff_ms: 1600,
    healthy_run_ms: 5000,
    window_ms: 60_000,
    max_restarts: 30,
};

/// What the supervisor remembers about a service, saved across its own restarts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Record {
    pub name: String,
    /// Restarts so far, ever.
    pub restarts: u64,
    /// Restarts in a row that ran shorter than `healthy_run_ms`.
    pub quick_failures: u64,
    /// Boot-relative times of recent restarts, in milliseconds, oldest first.
    pub recent: Vec<u64>,
    pub gave_up: bool,
    /// Stopped by a `stop` request: not restarted, even by a new supervisor,
    /// until `start`.
    pub stopped: bool,
    /// Kept running by the user's switch ([`keep`]): an app that never naps,
    /// passed `keep=1` at each start.
    pub keep: bool,
    /// The running instance's pid, or 0. Lets a restarted supervisor match exit
    /// records for instances started by its predecessor.
    pub pid: u64,
    /// When the running instance started (boot-relative ms), so a restarted
    /// supervisor can tell how long it ran.
    pub started_ms: u64,
    /// The package version a `start=pkg` service runs (`p-` and 12 hex
    /// digits), so a restarted supervisor still knows it: a withdraw ends it, a
    /// second `run` of it is the same version, and an exit starts it again.
    pub version: Option<String>,
}

impl Record {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            ..Self::default()
        }
    }

    /// The service ended at `now_ms` after running `ran_ms`.
    pub fn on_exit(&mut self, policy: &Policy, now_ms: u64, ran_ms: u64) -> Decision {
        if self.gave_up {
            return Decision::GiveUp;
        }
        self.recent
            .retain(|&at| now_ms.saturating_sub(at) < policy.window_ms);
        if self.recent.len() >= policy.max_restarts {
            self.gave_up = true;
            return Decision::GiveUp;
        }
        self.quick_failures = if ran_ms >= policy.healthy_run_ms {
            0
        } else {
            self.quick_failures + 1
        };
        self.restarts += 1;
        self.recent.push(now_ms);
        let shift = self.quick_failures.saturating_sub(1).min(16);
        let backoff = (policy.first_backoff_ms << shift).min(policy.max_backoff_ms);
        Decision::RestartAfterMs(if self.quick_failures == 0 { 0 } else { backoff })
    }

    /// A restart the user asked for (`restart <name>`): counted in
    /// [`Self::restarts`], which every start passes on as `restart=N`, so the
    /// program sees it is not this boot's first start (`updated` judges a
    /// model trial by that); not in `recent` or `quick_failures`, so it counts
    /// neither against the cap nor towards the backoff.
    pub fn restarted_on_request(&mut self) {
        self.restarts += 1;
    }
}

pub(crate) const STATE_MAGIC: &str = "supervisor-state 6";
/// The state before `version`, still read: its records have none.
pub(crate) const STATE_MAGIC_5: &str = "supervisor-state 5";
/// The state before `keep`, still read so an update keeps the counts.
pub(crate) const STATE_MAGIC_4: &str = "supervisor-state 4";
/// The state before `stopped`, still read too.
pub(crate) const STATE_MAGIC_3: &str = "supervisor-state 3";

/// Encodes records for the kernel-held state region.
pub fn encode(records: &[Record]) -> String {
    let mut text = String::from(STATE_MAGIC);
    for record in records {
        text.push('\n');
        text.push_str(&format!(
            "{} {} {} {} {} {} {} {} {}",
            record.name,
            record.restarts,
            record.quick_failures,
            u8::from(record.gave_up),
            u8::from(record.stopped),
            u8::from(record.keep),
            record.pid,
            record.started_ms,
            record.version.as_deref().unwrap_or("-")
        ));
        for at in &record.recent {
            text.push_str(&format!(" {at}"));
        }
    }
    text
}

/// Decodes saved records. Anything unreadable yields no records rather than an error:
/// a supervisor must still start when its saved state is damaged.
pub fn decode(bytes: &[u8]) -> Vec<Record> {
    let Ok(text) = core::str::from_utf8(bytes) else {
        return Vec::new();
    };
    let mut lines = text.lines();
    let (has_stopped, has_keep, has_version) = match lines.next() {
        Some(STATE_MAGIC) => (true, true, true),
        Some(STATE_MAGIC_5) => (true, true, false),
        Some(STATE_MAGIC_4) => (true, false, false),
        Some(STATE_MAGIC_3) => (false, false, false),
        _ => return Vec::new(),
    };
    lines
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let name = words.next()?.to_string();
            let restarts = words.next()?.parse().ok()?;
            let quick_failures = words.next()?.parse().ok()?;
            let gave_up = words.next()? == "1";
            let stopped = has_stopped && words.next()? == "1";
            let keep = has_keep && words.next()? == "1";
            let pid = words.next()?.parse().ok()?;
            let started_ms = words.next()?.parse().ok()?;
            let version = if has_version {
                match words.next()? {
                    "-" => None,
                    label if kabi::live::is_package_label(label) => Some(label.to_string()),
                    _ => return None,
                }
            } else {
                None
            };
            let recent = words
                .map(str::parse)
                .collect::<Result<Vec<u64>, _>>()
                .ok()?;
            Some(Record {
                name,
                restarts,
                quick_failures,
                recent,
                gave_up,
                stopped,
                keep,
                pid,
                started_ms,
                version,
            })
        })
        .collect()
}
