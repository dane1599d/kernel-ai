//! `app_build` in the background: the line answers at once, the rounds go on
//! between lines, `build status` and `stop building` reach it, and what came
//! of it leads the next reply.

use super::*;
use crate::ccbuild::CcBuilt;
use alloc::vec;
use llmproto::script::{Answer, AnswerError};

const SOURCE: &str = "#include <kaiapp.h>\nstruct state { I64 left; };\n";
const TIMER: &str =
    r#"{"name":"app_build","arguments":{"name":"timer","request":"a countdown timer"}}"#;

fn app() -> Result<Answer, AnswerError> {
    Ok(Answer {
        summary: String::from("A countdown timer."),
        script: String::from(SOURCE),
    })
}

fn built() -> CcBuilt {
    CcBuilt {
        app: true,
        bytes: 9_216,
        provenance: String::from(
            "built-by cc 631e318432aeaaf72d6d2dfc93085766934314a2 source 5e1f",
        ),
        path: String::from("/home/user/apps/timer.kai"),
    }
}

/// A shell on a host that asks off the line, whose model calls `app_build`
/// for the timer, then writes `cards`.
fn background(cards: Vec<Result<Answer, AnswerError>>) -> (Rc<FakeHost>, Shell) {
    let host = FakeHost::new();
    host.background.set(true);
    host.calls
        .borrow_mut()
        .push_back(Ok(llmproto::act::parse(TIMER).unwrap()));
    host.answers.borrow_mut().extend(cards);
    let shell = shell(&host);
    (host, shell)
}

/// The model answers what is in flight, and the shell goes on.
fn answer(host: &FakeHost, shell: &mut Shell) -> Option<crate::PanelResult> {
    host.answered.set(true);
    shell.app_poll()
}

#[test]
fn the_line_answers_at_once_and_the_build_s_notice_leads_the_next_reply() {
    let (host, mut shell) = background(vec![app()]);
    host.pkg_answers
        .borrow_mut()
        .push_back(Ok(String::from("0.0.1")));
    host.cc_builds.borrow_mut().push_back(Ok(built()));
    let reply = shell.eval("make me a countdown timer app");
    assert!(
        reply.ok && reply.text.starts_with("Building timer:"),
        "{reply:?}"
    );
    assert!(host.app_sources.borrow().is_empty(), "nothing written yet");
    assert_eq!(shell.eval("build status").text, "Kai is writing timer.");
    // Nothing happens until the model answers.
    assert_eq!(shell.app_poll(), None);
    assert!(host.pkg_sent.borrow().is_empty());
    // The panel's Done says it on its own, for someone who never types again.
    let shown = answer(&host, &mut shell);
    assert!(
        matches!(&shown, Some(crate::PanelResult::Done(line)) if line.starts_with("Built timer 0.0.1")),
        "{shown:?}"
    );
    assert_eq!(
        *host.pkg_sent.borrow(),
        [
            "local-next timer",
            "cc_build /home/user/apps/timer 0.0.1",
            "cc_trial timer"
        ]
    );
    assert!(shell.app_built.is_some());
    let next = shell.eval_console("build status");
    assert!(
        next.text
            .starts_with("Built timer 0.0.1 (9216 bytes): A countdown timer.")
            && next.text.ends_with("Kai is building no app."),
        "{next:?}"
    );
    // Shown once.
    assert_eq!(
        shell.eval_console("build status").text,
        "Kai is building no app."
    );
    assert!(
        shell
            .eval("log actlog")
            .text
            .contains("trial init, started")
    );
}

#[test]
fn a_repair_goes_on_in_the_background_and_one_build_runs_at_a_time() {
    let (host, mut shell) = background(vec![app(), app()]);
    for _ in 0..2 {
        host.pkg_answers
            .borrow_mut()
            .push_back(Ok(String::from("0.0.1")));
    }
    let mut builds = host.cc_builds.borrow_mut();
    builds.push_back(Err(String::from("main.c:2: error: expected ';'")));
    builds.push_back(Ok(built()));
    drop(builds);
    assert!(shell.eval("make me a countdown timer app").ok);
    host.calls
        .borrow_mut()
        .push_back(Ok(llmproto::act::parse(TIMER).unwrap()));
    let second = shell.eval("make me a countdown timer app");
    assert!(
        !second.ok && second.text.contains("Kai is already building timer"),
        "{second:?}"
    );
    assert_eq!(answer(&host, &mut shell), None, "a repair is not an end");
    assert_eq!(
        shell.eval("build status").text,
        "Kai is repairing timer: repair 1 of 3."
    );
    let asked = host.script_requests.borrow().clone();
    assert_eq!(asked.len(), 2);
    assert_eq!(
        asked[1].repair.as_ref().map(|(_, why)| why.as_str()),
        Some("cc build refused it:\nmain.c:2: error: expected ';'")
    );
    assert_eq!(asked[1].timeout_ms, crate::appjob::APP_ROUND_MS);
    answer(&host, &mut shell);
    assert!(shell.app_built.is_some());
}

#[test]
fn stop_building_is_logged_keeps_the_source_and_drops_the_late_answer() {
    let (host, mut shell) = background(vec![app(), app()]);
    host.pkg_answers
        .borrow_mut()
        .push_back(Ok(String::from("0.0.1")));
    host.cc_builds
        .borrow_mut()
        .push_back(Err(String::from("main.c:2: error: expected ';'")));
    assert!(shell.eval("make me a countdown timer app").ok);
    answer(&host, &mut shell);
    assert_eq!(host.app_sources.borrow().len(), 1, "round 1 wrote main.c");
    let stopped = shell.eval("stop building");
    assert!(
        stopped.ok
            && stopped
                .text
                .contains("What Kai wrote so far stays in /home/user/apps/timer/"),
        "{stopped:?}"
    );
    let actlog = shell.eval("log actlog").text;
    assert!(actlog.contains("stopped by the person"), "{actlog}");
    assert_eq!(*host.stops.borrow(), [2], "the round in flight is stopped");
    // The model is still answering the stopped round: a new build waits.
    host.calls
        .borrow_mut()
        .push_back(Ok(llmproto::act::parse(TIMER).unwrap()));
    let again = shell.eval("make me a countdown timer app");
    assert!(
        !again.ok && again.text.contains("Kai cannot start building timer yet"),
        "{again:?}"
    );
    // Its late answer is dropped: nothing more is built.
    answer(&host, &mut shell);
    assert_eq!(host.app_sources.borrow().len(), 1);
    assert!(shell.app_built.is_none());
    assert_eq!(shell.eval("stop building").text, "Kai is building no app");
}

#[test]
fn a_host_that_cannot_ask_off_the_line_builds_within_the_line() {
    let (host, mut shell) = background(vec![app()]);
    host.background.set(false);
    host.pkg_answers
        .borrow_mut()
        .push_back(Ok(String::from("0.0.1")));
    host.cc_builds.borrow_mut().push_back(Ok(built()));
    let reply = shell.eval("make me a countdown timer app");
    assert!(
        reply.ok && reply.text.starts_with("Built timer"),
        "{reply:?}"
    );
}

/// The manager's condition: an llm that stopped or restarted ends the job
/// with why, so it never shows as going on forever.
#[test]
fn a_job_the_llm_dropped_ends_with_why() {
    let gone = AnswerError::Unavailable(String::from("the llm service restarted"));
    let (host, mut shell) = background(vec![Err(gone)]);
    assert!(shell.eval("make me a countdown timer app").ok);
    let shown = answer(&host, &mut shell);
    assert!(
        matches!(&shown, Some(crate::PanelResult::Refused(why)) if why.contains("restarted")),
        "{shown:?}"
    );
    let after = shell.eval_console("build status").text;
    assert!(after.contains("restarted"), "{after}");
    assert_eq!(after.lines().last(), Some("Kai is building no app."));
}

/// C on #621: a panel line after a build ended keeps its own result, and the
/// notice waits for the terminal.
#[test]
fn a_panel_line_keeps_its_result_and_the_notice_waits_for_the_terminal() {
    let (host, mut shell) = background(vec![app()]);
    host.pkg_answers
        .borrow_mut()
        .push_back(Ok(String::from("0.0.1")));
    host.cc_builds.borrow_mut().push_back(Ok(built()));
    assert!(shell.eval_console("make me a countdown timer app").ok);
    answer(&host, &mut shell);
    assert_eq!(
        shell.eval_panel("1 + 1"),
        crate::PanelResult::Done(String::from("2"))
    );
    let next = shell.eval_console("build status").text;
    assert!(next.starts_with("Built timer 0.0.1"), "{next}");
}

/// C on #621: between rounds the person may edit main.c; the next round
/// never overwrites it.
#[test]
fn a_main_c_edited_mid_build_is_not_overwritten_by_the_repair() {
    let (host, mut shell) = background(vec![app(), app()]);
    host.pkg_answers
        .borrow_mut()
        .push_back(Ok(String::from("0.0.1")));
    host.cc_builds
        .borrow_mut()
        .push_back(Err(String::from("main.c:2: error: expected ';'")));
    assert!(shell.eval_console("make me a countdown timer app").ok);
    answer(&host, &mut shell);
    host.app_sources.borrow_mut()[0].1 = String::from("/* mine */\n");
    let shown = answer(&host, &mut shell);
    assert!(
        matches!(&shown, Some(crate::PanelResult::Refused(why)) if why.contains("was changed since Kai wrote it")),
        "{shown:?}"
    );
    assert_eq!(
        *host.app_sources.borrow(),
        [(String::from("timer"), String::from("/* mine */\n"))]
    );
}

#[test]
fn esc_on_the_panel_stops_a_build_and_nothing_else() {
    let (_host, mut shell) = background(vec![app()]);
    assert!(shell.stop_app_build().is_none());
    assert!(shell.eval_console("make me a countdown timer app").ok);
    let stopped = shell.stop_app_build().expect("a build runs");
    assert!(
        stopped.text.starts_with("Stopped building timer"),
        "{stopped:?}"
    );
    assert!(shell.stop_app_build().is_none());
}
