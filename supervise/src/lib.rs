//! The ring-3 supervisor's decisions, kept free of system calls so they can be
//! tested on the host: reading the service plan, the restart policy (exponential
//! backoff with a restart-intensity cap, as in Erlang/OTP), and the state saved in
//! the kernel-held region so a restarted supervisor carries on where it left off.

#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use kabi::{Limits, cap};

/// Defaults for a service that does not set its own limits.
pub const DEFAULT_PAGES: u64 = 1024;
pub const DEFAULT_BURST_MS: u64 = 2000;

/// One service from the plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceSpec {
    pub name: String,
    /// The program to run; the service name unless the plan says `program=`.
    pub program: String,
    /// Arguments passed through to the program.
    pub args: String,
    pub limits: Limits,
    /// Restart policy; the plan may change `max_backoff_ms` and `max_restarts`.
    pub policy: Policy,
}

/// A parsed plan: the services that parsed, and why any other line was skipped.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub services: Vec<ServiceSpec>,
    pub errors: Vec<String>,
}

/// Reads the plan: one service per line, `name [program=P] [pages=N] [stack_pages=N]
/// [burst_ms=N] [caps=serve+net+call+policy+component+publish] [backoff_max_ms=N] [max_restarts=N]
/// [other=args ...]`. The supervisor keys set limits and policy; everything else is
/// passed to the program. A bad line skips only that service, so one typo cannot
/// keep every other service down.
pub fn parse_plan(text: &str) -> Plan {
    let mut plan = Plan::default();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        match parse_line(line, &plan.services) {
            Ok(spec) => plan.services.push(spec),
            Err(error) => plan.errors.push(error),
        }
    }
    plan
}

fn parse_line(line: &str, earlier: &[ServiceSpec]) -> Result<ServiceSpec, String> {
    let mut words = line.split_whitespace();
    let name = words.next().unwrap_or_default();
    if !valid_name(name) {
        return Err(format!("bad service name {name:?}"));
    }
    if earlier.iter().any(|s| s.name == name) {
        return Err(format!("service {name} listed twice"));
    }
    let mut limits = Limits {
        max_pages: DEFAULT_PAGES,
        max_burst_ms: DEFAULT_BURST_MS,
        caps: 0,
        stack_pages: 0,
    };
    let mut policy = POLICY;
    let mut args = Vec::new();
    let mut program = name;
    for word in words {
        match word.split_once('=') {
            Some(("program", value)) => {
                if !valid_name(value) {
                    return Err(format!("{name}: bad program name {value:?}"));
                }
                // The supervisor runs only as the kernel starts it.
                if value == "supervisor" {
                    return Err(format!("{name}: the supervisor cannot run as a service"));
                }
                program = value;
            }
            Some(("pages", value)) => limits.max_pages = number(name, value)?,
            Some(("stack_pages", value)) => {
                limits.stack_pages = number(name, value)?;
                if limits.stack_pages > kabi::layout::MAX_STACK_PAGES {
                    return Err(format!(
                        "{name}: stack_pages over {}",
                        kabi::layout::MAX_STACK_PAGES
                    ));
                }
            }
            Some(("burst_ms", value)) => limits.max_burst_ms = number(name, value)?,
            Some(("caps", value)) => limits.caps = caps(name, value)?,
            Some(("backoff_max_ms", value)) => policy.max_backoff_ms = number(name, value)?,
            Some(("max_restarts", value)) => policy.max_restarts = number(name, value)? as usize,
            _ => args.push(word),
        }
    }
    Ok(ServiceSpec {
        name: name.to_string(),
        program: program.to_string(),
        args: args.join(" "),
        limits,
        policy,
    })
}

/// A service or program name: letters, digits and '-', at most `MAX_NAME` bytes.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= kabi::MAX_NAME
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn number(service: &str, value: &str) -> Result<u64, String> {
    value
        .parse()
        .ok()
        .filter(|&n| n > 0)
        .ok_or_else(|| format!("{service}: bad number {value:?}"))
}

fn caps(service: &str, value: &str) -> Result<u64, String> {
    value.split('+').try_fold(0, |bits, name| match name {
        "serve" => Ok(bits | cap::SERVE),
        "net" => Ok(bits | cap::NET),
        "call" => Ok(bits | cap::CALL),
        "policy" => Ok(bits | cap::POLICY),
        "component" => Ok(bits | cap::COMPONENT),
        "publish" => Ok(bits | cap::PUBLISH),
        "model" => Ok(bits | cap::MODEL),
        other => Err(format!("{service}: unknown capability {other:?}")),
    })
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
    /// The running instance's pid, or 0. Lets a restarted supervisor match exit
    /// records for instances started by its predecessor.
    pub pid: u64,
    /// When the running instance started (boot-relative ms), so a restarted
    /// supervisor can tell how long it ran.
    pub started_ms: u64,
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
}

const STATE_MAGIC: &str = "supervisor-state 3";

/// Encodes records for the kernel-held state region.
pub fn encode(records: &[Record]) -> String {
    let mut text = String::from(STATE_MAGIC);
    for record in records {
        text.push('\n');
        text.push_str(&format!(
            "{} {} {} {} {} {}",
            record.name,
            record.restarts,
            record.quick_failures,
            u8::from(record.gave_up),
            record.pid,
            record.started_ms
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
    if lines.next() != Some(STATE_MAGIC) {
        return Vec::new();
    }
    lines
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let name = words.next()?.to_string();
            let restarts = words.next()?.parse().ok()?;
            let quick_failures = words.next()?.parse().ok()?;
            let gave_up = words.next()? == "1";
            let pid = words.next()?.parse().ok()?;
            let started_ms = words.next()?.parse().ok()?;
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
                pid,
                started_ms,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn plan_splits_limits_from_program_arguments() {
        let plan = parse_plan(
            "llm caps=serve+net server=10.0.2.2:8080 model=mock\n\n  spinner burst_ms=300\nhog pages=64",
        )
        .services;
        assert_eq!(plan.len(), 3);
        assert_eq!(plan[0].name, "llm");
        assert_eq!(plan[0].args, "server=10.0.2.2:8080 model=mock");
        assert_eq!(plan[0].limits.caps, cap::SERVE | cap::NET);
        assert_eq!(plan[1].limits.max_burst_ms, 300);
        assert_eq!(plan[1].limits.max_pages, DEFAULT_PAGES);
        assert_eq!(plan[2].limits.max_pages, 64);
        assert_eq!(plan[2].limits.caps, 0);
        assert_eq!(plan[2].limits.stack_pages, 0);
        assert_eq!(plan[2].policy.max_restarts, POLICY.max_restarts);
    }

    #[test]
    fn the_local_llm_line_names_the_model_capability() {
        let plan = parse_plan("llm caps=serve+net+model burst_ms=10000 pages=1331").services;
        assert_eq!(plan[0].limits.caps, cap::SERVE | cap::NET | cap::MODEL);
        assert_eq!(plan[0].limits.max_burst_ms, 10_000);
        assert_eq!(plan[0].limits.max_pages, 1331);
        assert_eq!(plan[0].args, "");
    }

    #[test]
    fn plan_can_tune_the_restart_policy_per_service() {
        let plan = parse_plan("crasher backoff_max_ms=200 max_restarts=1000 after_ms=150").services;
        assert_eq!(plan[0].policy.max_backoff_ms, 200);
        assert_eq!(plan[0].policy.max_restarts, 1000);
        assert_eq!(plan[0].policy.first_backoff_ms, POLICY.first_backoff_ms);
        assert_eq!(plan[0].args, "after_ms=150");
    }

    #[test]
    fn the_plan_the_kernel_sends_parses() {
        let text = alloc::format!(
            "llm caps=serve+net burst_ms=10000 pages=1024 host=10.0.2.2:8080 model=org/model-7b\n{}",
            kabi::plan::SHELL
        );
        let plan = parse_plan(&text);
        assert_eq!(plan.errors, Vec::<String>::new());
        let shell = &plan.services[1];
        assert_eq!(shell.name, "shell");
        assert_eq!(shell.limits.caps, cap::SERVE | cap::CALL | cap::POLICY);
        assert_eq!(shell.limits.stack_pages, 512);
        assert!(shell.limits.stack_pages < shell.limits.max_pages);
        assert_eq!(
            plan.services[0].args,
            "host=10.0.2.2:8080 model=org/model-7b"
        );
    }

    #[test]
    fn a_bad_line_skips_only_its_service() {
        let plan = parse_plan("llm caps=serve+net\nshell caps=serve+teleport\nhog pages=64");
        let names: Vec<&str> = plan.services.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["llm", "hog"]);
        assert_eq!(plan.errors.len(), 1);
        assert!(plan.errors[0].contains("teleport"));
    }

    #[test]
    fn plan_rejects_bad_lines() {
        for bad in [
            "a caps=supervise",
            "a pages=0",
            "a pages=x",
            "a stack_pages=4096",
            "bad/name",
            "a-very-long-service-name-indeed",
        ] {
            let plan = parse_plan(bad);
            assert!(plan.services.is_empty() && plan.errors.len() == 1, "{bad}");
        }
        assert_eq!(parse_plan("a\na").errors.len(), 1);
    }

    #[test]
    fn quick_failures_back_off_exponentially_up_to_the_cap() {
        let mut record = Record::new("crasher");
        let delays: Vec<Decision> = (0..8)
            .map(|i| record.on_exit(&POLICY, i * 1000, 200))
            .collect();
        let expected = [50, 100, 200, 400, 800, 1600, 1600, 1600].map(Decision::RestartAfterMs);
        assert_eq!(delays, expected);
        assert_eq!(record.restarts, 8);
    }

    #[test]
    fn a_healthy_run_resets_the_backoff() {
        let mut record = Record::new("llm");
        record.on_exit(&POLICY, 0, 10);
        record.on_exit(&POLICY, 100, 10);
        assert_eq!(
            record.on_exit(&POLICY, 20_000, 10_000),
            Decision::RestartAfterMs(0)
        );
        assert_eq!(
            record.on_exit(&POLICY, 20_100, 10),
            Decision::RestartAfterMs(50)
        );
    }

    #[test]
    fn too_many_restarts_in_the_window_gives_up_for_good() {
        let policy = Policy {
            max_restarts: 3,
            ..POLICY
        };
        let mut record = Record::new("crasher");
        for i in 0..3 {
            assert!(matches!(
                record.on_exit(&policy, i, 1),
                Decision::RestartAfterMs(_)
            ));
        }
        assert_eq!(record.on_exit(&policy, 3, 1), Decision::GiveUp);
        assert_eq!(record.on_exit(&policy, 999_999, 1), Decision::GiveUp);
    }

    #[test]
    fn old_restarts_leave_the_window() {
        let policy = Policy {
            max_restarts: 2,
            window_ms: 1000,
            ..POLICY
        };
        let mut record = Record::new("s");
        record.on_exit(&policy, 0, 1);
        record.on_exit(&policy, 10, 1);
        assert!(matches!(
            record.on_exit(&policy, 2000, 1),
            Decision::RestartAfterMs(_)
        ));
    }

    #[test]
    fn state_round_trips_and_damage_is_ignored() {
        let records = vec![
            Record {
                name: "crasher".into(),
                restarts: 7,
                quick_failures: 3,
                recent: vec![10, 20, 30],
                gave_up: false,
                pid: 42,
                started_ms: 1234,
            },
            Record {
                name: "hog".into(),
                restarts: 1,
                quick_failures: 0,
                recent: vec![],
                gave_up: true,
                pid: 0,
                started_ms: 0,
            },
        ];
        assert_eq!(decode(encode(&records).as_bytes()), records);
        assert!(decode(b"").is_empty());
        assert!(decode(b"garbage").is_empty());
        assert!(decode(&[0xFF, 0xFE]).is_empty());
        let damaged = alloc::format!("{STATE_MAGIC}\ncrasher x 1 0 0 0\nhog 2 0 0 7 99");
        // State in an older layout reads as none rather than as shifted fields.
        assert!(decode(b"supervisor-state 2\nhog 2 0 0 7").is_empty());
        assert_eq!(decode(damaged.as_bytes()).len(), 1);
    }

    #[test]
    fn program_names_the_program_and_is_not_passed_on() {
        let plan = parse_plan(kabi::plan::NOTES_TEST);
        assert!(plan.errors.is_empty(), "{:?}", plan.errors);
        let notes = &plan.services[0];
        assert_eq!(
            (notes.name.as_str(), notes.program.as_str()),
            ("notes", "comphost")
        );
        assert_eq!(notes.limits.caps, cap::SERVE | cap::COMPONENT);
        assert_eq!(notes.limits.max_pages, 8192);
        assert_eq!(notes.args, "version=1 probation_calls=3 probation_ms=0");
        let notes = &parse_plan(&kabi::plan::notes("2", None)).services[0];
        assert_eq!(notes.args, "version=2");
        // Without the key, a service runs the program of its own name.
        let shell = &parse_plan(kabi::plan::SHELL).services[0];
        assert_eq!(shell.program, "shell");
    }

    #[test]
    fn a_service_cannot_run_the_supervisor_or_a_bad_program_name() {
        let plan = parse_plan("evil program=supervisor\nodd program=a/b\nok program=comphost");
        assert_eq!(plan.services.len(), 1);
        assert_eq!(plan.errors.len(), 2, "{:?}", plan.errors);
        assert!(plan.errors[0].contains("supervisor"));
    }

    #[test]
    fn the_edit_service_and_its_test_plan_parse() {
        let line = alloc::format!(
            "{} host=10.0.2.2:5000 token=0123456789abcdef0123456789abcdef",
            kabi::plan::EDIT_BASE
        );
        let plan = parse_plan(&line);
        assert!(plan.errors.is_empty(), "{:?}", plan.errors);
        let edit = &plan.services[0];
        assert_eq!(
            (edit.name.as_str(), edit.program.as_str()),
            ("edit", "edit")
        );
        assert_eq!(edit.limits.caps, cap::SERVE | cap::PUBLISH);
        assert_eq!(
            edit.args,
            "host=10.0.2.2:5000 token=0123456789abcdef0123456789abcdef"
        );
        let notes = &parse_plan(kabi::plan::NOTES_EDIT_TEST).services[0];
        assert_eq!(notes.args, "version=2 probation_calls=3 probation_ms=0");
        assert_eq!(notes.limits.caps, cap::SERVE | cap::COMPONENT);
    }
}
