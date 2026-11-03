//! `app-call <app>\n<request>`: the agent core's call to an app's actions
//! (P4, the agent-to-app route). The shell has no route to an app, so the
//! supervisor forwards the call through the kernel (APP_CALL), which names the
//! app by its own label for the running process. A v0.1 bridge: ADR 0001's
//! badged per-app routes replace it once routes can be made at run time.
//!
//! Only the shell may ask, only an app (a `component` service of the plan)
//! may be called, and only `kai-call 1`, `kai-undo 1` and `kai-actions 1` (the
//! app's rows) go through, at most
//! [`MAX_REQUEST`] bytes. An app that is not running is started, once, and
//! the call tried again.

use crate::ServiceSpec;
use alloc::format;
use alloc::string::String;
use kabi::cap;

pub const APP_CALL: &str = "app-call ";
/// The requests forwarded, by their first line.
pub const HEADERS: &[&str] = &["kai-call 1\n", "kai-undo 1\n"];
/// The request forwarded whole: the app's rows (`livecore::rows`).
pub const ROWS: &str = "kai-actions 1";
/// The longest request forwarded: one message, less the `app-call` line.
pub const MAX_REQUEST: usize = kabi::MAX_MESSAGE - 64;
/// Who may ask.
pub const CALLER: &str = "shell";
/// The longest the supervisor waits on one `app-call`, a start included: it
/// serves one request at a time, so a slow or hung app holds every other
/// request, restarts too, no longer than this.
pub const WAIT_MS: u64 = 5_000;
/// The answer when the app did not answer in [`WAIT_MS`]; the shell may try
/// once more.
pub const TIMEOUT: &str = "err timeout";
/// Before the app's name, the answer when the kernel refused to start it for
/// memory (the admission rule, free / 2): no wait, and nothing to retry.
pub const NO_MEMORY: &str = "err no memory to start ";

/// [`NO_MEMORY`] for `app`.
pub fn no_memory(app: &str) -> alloc::string::String {
    alloc::format!("{NO_MEMORY}{app}")
}

/// The app an answer says there was no memory to start, if it says that.
pub fn no_memory_for(answer: &str) -> Option<&str> {
    answer.strip_prefix(NO_MEMORY).filter(|app| !app.is_empty())
}
/// The supervisor's request buffer: one message, so a full `app-call` fits.
pub const MAX_SERVED: usize = kabi::MAX_MESSAGE;

/// `app-call <app>\n<request>` read back: the app and its request.
pub fn parse(text: &str) -> Option<(&str, &str)> {
    let (app, request) = text.strip_prefix(APP_CALL)?.split_once('\n')?;
    Some((app, request))
}

/// The request for app `app`.
pub fn encode(app: &str, request: &str) -> String {
    format!("{APP_CALL}{app}\n{request}")
}

/// Whether `caller` may send `request` to `app`: the plan's index of the app,
/// or why not.
pub fn check(
    caller: &str,
    app: &str,
    request: &str,
    specs: &[&ServiceSpec],
) -> Result<usize, &'static str> {
    if caller != CALLER {
        return Err("err only the shell calls apps");
    }
    let rows = request.strip_suffix('\n').unwrap_or(request) == ROWS;
    if !rows && !HEADERS.iter().any(|header| request.starts_with(header)) {
        return Err("err only kai-call 1, kai-undo 1 and kai-actions 1 go to an app");
    }
    if request.len() > MAX_REQUEST {
        return Err("err the call is too long");
    }
    specs
        .iter()
        .position(|spec| spec.name == app && spec.limits.caps & cap::COMPONENT != 0)
        .ok_or("err no such app")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_plan;

    const PLAN: &str = "llm caps=serve\nnotes program=comphost caps=serve+component";

    fn specs() -> alloc::vec::Vec<ServiceSpec> {
        parse_plan(PLAN).services
    }

    #[test]
    fn a_call_reads_back() {
        let call = "kai-call 1\nfunc app:notes/actions@1.0.0#count\nargs {}\n";
        assert_eq!(parse(&encode("notes", call)), Some(("notes", call)));
        assert_eq!(parse("app-call notes"), None);
        assert_eq!(parse("stop notes"), None);
    }

    #[test]
    fn only_the_shell_calls_only_an_app_with_only_kai_call_or_undo() {
        let specs = specs();
        let specs: alloc::vec::Vec<&ServiceSpec> = specs.iter().collect();
        let call = "kai-call 1\nfunc app:notes/actions@1.0.0#count\nargs {}\n";
        assert_eq!(check("shell", "notes", call, &specs), Ok(1));
        assert!(check("edit", "notes", call, &specs).is_err());
        assert!(check("shell", "llm", call, &specs).is_err());
        assert!(check("shell", "nobody", call, &specs).is_err());
        assert!(check("shell", "notes", "ctl suspend", &specs).is_err());
        assert!(check("shell", "notes", "kai-undo 1\nid 3\n", &specs).is_ok());
        assert!(check("shell", "notes", "kai-actions 1", &specs).is_ok());
        assert!(check("shell", "notes", "kai-actions 1 x", &specs).is_err());
        let long = format!("kai-call 1\n{}", "x".repeat(MAX_REQUEST));
        assert!(check("shell", "notes", &long, &specs).is_err());
        // The longest call allowed fits the supervisor's buffer whole.
        let full = format!("kai-call 1\n{}", "x".repeat(MAX_REQUEST - 11));
        assert!(check("shell", "notes", &full, &specs).is_ok());
        assert!(encode("notes-window-long-name", &full).len() <= MAX_SERVED);
    }

    #[test]
    fn no_memory_names_its_app_and_nothing_else_reads_as_it() {
        let answer = no_memory("notes-window");
        assert_eq!(answer, "err no memory to start notes-window");
        assert_eq!(no_memory_for(&answer), Some("notes-window"));
        for other in [
            TIMEOUT,
            "err no memory to start ",
            "ok",
            "err notes is stopped",
        ] {
            assert_eq!(no_memory_for(other), None, "{other:?}");
        }
    }
}
