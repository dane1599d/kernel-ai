//! What a stop, start or restart names: code, not the model, decides whether the
//! name is a supervised service or a plain process, and so which row runs. The
//! model may say `process_stop("indexer")` or `service_stop("build-3")`; the
//! door looks the name up and runs the row that fits, or asks.

use agentabi::Action;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// A service the supervisor runs, as its `services` request lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Service {
    pub name: String,
    /// The running instance's pid, or 0 between restarts.
    pub pid: u64,
    /// Stopped by a stop request, until started.
    pub stopped: bool,
}

/// What the user wants done to the name, whichever row the model picked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verb {
    Stop,
    Start,
    Restart,
}

impl Verb {
    /// The verb of a stop, start or restart row; `None` for any other row.
    pub fn of(row: &Action) -> Option<Self> {
        match row.name {
            "process_stop" | "service_stop" => Some(Verb::Stop),
            "service_start" => Some(Verb::Start),
            "service_restart" => Some(Verb::Restart),
            _ => None,
        }
    }
}

/// The row that fits, and how the menu names its target.
#[derive(Clone, Debug)]
pub struct Found {
    pub row: &'static Action,
    /// "process build-3 (pid 7)" or "service indexer".
    pub subject: String,
}

/// Why no row fits: a question back to the user (the next line joins the
/// request, as `clarify`'s does), or a plain answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unresolved {
    Clarify(String),
    Say(String),
}

/// The row for `verb` on `name`, given the supervisor's `services` and the
/// running processes as (pid, name). A running service is also a process with
/// its name: only a process with another pid makes the name both.
pub fn resolve(
    verb: Verb,
    name: &str,
    services: &[Service],
    processes: &[(u64, String)],
) -> Result<Found, Unresolved> {
    let service = services.iter().find(|s| s.name == name);
    let pids: Vec<u64> = processes
        .iter()
        .filter(|(pid, process)| process == name && service.is_none_or(|s| s.pid != *pid))
        .map(|(pid, _)| *pid)
        .collect();
    match (service, pids.as_slice(), verb) {
        (Some(_), [_, ..], _) => Err(Unresolved::Clarify(format!(
            "{name} is both a service and a separate process ({}). Which one?",
            pid_list(&pids)
        ))),
        (Some(s), [], Verb::Stop) if s.stopped => Err(Unresolved::Say(format!(
            "Service {name} is already stopped."
        ))),
        (Some(_), [], Verb::Stop) => Ok(service_row(&agentabi::SERVICE_STOP, name)),
        // Restarting a stopped service is starting it.
        (Some(s), [], Verb::Start | Verb::Restart) if s.stopped => {
            Ok(service_row(&agentabi::SERVICE_START, name))
        }
        (Some(_), [], Verb::Start) => Err(Unresolved::Say(format!(
            "Service {name} is already running."
        ))),
        (Some(_), [], Verb::Restart) => Ok(service_row(&agentabi::SERVICE_RESTART, name)),
        (None, [_, ..], Verb::Stop) => Ok(Found {
            row: &agentabi::PROCESS_STOP,
            subject: format!("process {name} ({})", pid_list(&pids)),
        }),
        (None, [_, ..], _) => Err(Unresolved::Clarify(format!(
            "{name} is a process ({}), not a service, so it cannot be started again. Stop it instead?",
            pid_list(&pids)
        ))),
        (None, [], _) => Err(Unresolved::Clarify(format!(
            "No service or process named {name} is running. Which one did you mean?"
        ))),
    }
}

impl Found {
    /// How the menu's question starts: "Stop process build-3 (pid 7)".
    pub fn question(&self) -> String {
        let verb = match self.row.name {
            "service_start" => "Start",
            "service_restart" => "Restart",
            _ => "Stop",
        };
        format!("{verb} {}", self.subject)
    }
}

fn service_row(row: &'static Action, name: &str) -> Found {
    let subject = format!("service {name}");
    Found { row, subject }
}

/// "pid 7", or "pids 7, 9".
fn pid_list(pids: &[u64]) -> String {
    let joined: Vec<String> = pids.iter().map(|pid| format!("{pid}")).collect();
    match pids.len() {
        1 => format!("pid {}", joined[0]),
        _ => format!("pids {}", joined.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    fn services() -> Vec<Service> {
        let service = |name: &str, pid, stopped| Service {
            name: name.to_string(),
            pid,
            stopped,
        };
        vec![
            service("indexer", 5, false),
            service("notes", 0, true),
            service("cache", 6, false),
        ]
    }

    fn processes() -> Vec<(u64, String)> {
        [(5, "indexer"), (6, "cache"), (7, "build-3"), (8, "cache")]
            .into_iter()
            .map(|(pid, name)| (pid, name.to_string()))
            .collect()
    }

    fn run(verb: Verb, name: &str) -> Result<(&'static str, String), Unresolved> {
        resolve(verb, name, &services(), &processes()).map(|f| (f.row.name, f.subject))
    }

    #[test]
    fn a_plain_process_is_a_process_stop_named_with_its_pid() {
        assert_eq!(
            run(Verb::Stop, "build-3"),
            Ok(("process_stop", "process build-3 (pid 7)".to_string()))
        );
    }

    #[test]
    fn a_service_is_a_service_stop_or_restart_whatever_row_was_picked() {
        assert_eq!(
            run(Verb::Stop, "indexer"),
            Ok(("service_stop", "service indexer".to_string()))
        );
        assert_eq!(
            run(Verb::Restart, "indexer"),
            Ok(("service_restart", "service indexer".to_string()))
        );
        assert_eq!(
            Verb::of(&agentabi::PROCESS_STOP),
            Verb::of(&agentabi::SERVICE_STOP)
        );
    }

    #[test]
    fn starting_or_restarting_a_stopped_service_starts_it() {
        for verb in [Verb::Start, Verb::Restart] {
            assert_eq!(
                run(verb, "notes"),
                Ok(("service_start", "service notes".to_string()))
            );
        }
    }

    #[test]
    fn both_or_neither_asks() {
        let both = run(Verb::Stop, "cache");
        assert!(
            matches!(&both, Err(Unresolved::Clarify(q)) if q.contains("both") && q.contains("pid 8")),
            "{both:?}"
        );
        let neither = run(Verb::Stop, "nope");
        assert!(
            matches!(neither, Err(Unresolved::Clarify(_))),
            "{neither:?}"
        );
        let restart = run(Verb::Restart, "build-3");
        assert!(
            matches!(&restart, Err(Unresolved::Clarify(q)) if q.contains("not a service")),
            "{restart:?}"
        );
    }

    #[test]
    fn what_is_already_so_is_said() {
        assert_eq!(
            run(Verb::Stop, "notes"),
            Err(Unresolved::Say(
                "Service notes is already stopped.".to_string()
            ))
        );
        assert_eq!(
            run(Verb::Start, "indexer"),
            Err(Unresolved::Say(
                "Service indexer is already running.".to_string()
            ))
        );
    }
}
