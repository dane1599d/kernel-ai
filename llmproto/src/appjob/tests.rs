use super::*;
use alloc::vec::Vec;

fn app(name: &str, repair: bool) -> String {
    ScriptRequest {
        request: format!("{name}\na countdown timer"),
        repair: repair.then(|| (String::from("void init"), String::from("line 1: error"))),
        timeout_ms: 70_000,
        lang: Lang::App,
    }
    .encode()
}

fn started(jobs: &mut Jobs, name: &str) -> u64 {
    match jobs.start(&app(name, false), true) {
        Start::Run(ticket, request) => {
            assert_eq!(request.lang, Lang::App);
            ticket
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_round_is_pending_while_it_runs_then_taken_once() {
    let mut jobs = Jobs::new(3);
    let ticket = started(&mut jobs, "timer");
    assert_eq!(ticket >> 32, 3);
    let take = format!("app-take {ticket}");
    assert_eq!(jobs.while_running(&take), (String::from("pending"), false));
    jobs.finish(ticket, String::from("ok 5 3\nA app"));
    assert_eq!(jobs.take(&ticket.to_string()), "ok 5 3\nA app");
    // Taken, it is gone.
    assert_eq!(jobs.take(&ticket.to_string()), "ended unknown ticket");
}

#[test]
fn a_stopped_round_ends_and_is_never_taken() {
    let mut jobs = Jobs::new(0);
    let ticket = started(&mut jobs, "timer");
    let (reply, stop) = jobs.while_running(&format!("app-stop {ticket}"));
    assert_eq!((reply.as_str(), stop), ("ok", true));
    jobs.finish(ticket, String::from("err cutoff"));
    assert_eq!(jobs.take(&ticket.to_string()), "ended unknown ticket");
    // A new job may start at once.
    assert!(matches!(
        jobs.start(&app("timer", false), true),
        Start::Run(..)
    ));
}

#[test]
fn one_job_at_a_time_and_other_requests_are_told_why() {
    let mut jobs = Jobs::new(0);
    let ticket = started(&mut jobs, "timer");
    let busy = |jobs: &mut Jobs, text: &str| jobs.while_running(text).0;
    assert_eq!(
        busy(&mut jobs, &format!("app-start {}", app("dice", false))),
        "busy Kai is writing timer"
    );
    assert!(busy(&mut jobs, "act 1 2 3\nxy").contains("say `stop building`"));
    let script = busy(&mut jobs, "script 1 2 0 0\nhi");
    let refused = script.strip_prefix("err ").unwrap();
    assert!(matches!(
        script::decode_error(refused),
        AnswerError::Unavailable(why) if why.contains("Kai is writing timer")
    ));
    assert!(busy(&mut jobs, "ask what time is it").starts_with("err Kai is writing timer"));
    assert_eq!(busy(&mut jobs, "warm"), "ok no");
    let health = busy(&mut jobs, "health");
    let status = crate::health::Status::of(&health).expect("a status");
    assert_eq!(status.state, crate::health::State::Busy);
    // Another ticket's take and stop do not touch this job.
    let other = ticket + 1;
    assert_eq!(
        busy(&mut jobs, &format!("app-take {other}")),
        "ended unknown ticket"
    );
    assert_eq!(
        jobs.while_running(&format!("app-stop {other}")),
        (String::from("ok"), false)
    );
    assert_eq!(busy(&mut jobs, &format!("app-take {ticket}")), "pending");
}

#[test]
fn a_repair_round_says_so() {
    let mut jobs = Jobs::new(0);
    let Start::Run(..) = jobs.start(&app("timer", true), true) else {
        panic!("not started");
    };
    assert!(jobs.while_running("ask x").0.contains("timer (a repair)"));
}

#[test]
fn a_ticket_from_another_run_of_llm_is_unknown() {
    let mut before = Jobs::new(1);
    let old = started(&mut before, "timer");
    let mut after = Jobs::new(2);
    let new = started(&mut after, "timer");
    assert_ne!(old, new);
    after.finish(new, String::from("ok x"));
    assert_eq!(after.take(&old.to_string()), "ended unknown ticket");
    assert_eq!(after.take(&new.to_string()), "ok x");
}

#[test]
fn rounds_are_inline_without_a_local_model_and_only_apps_start() {
    let mut jobs = Jobs::new(0);
    assert_eq!(
        jobs.start(&app("timer", false), false),
        Start::Reply(String::from("inline"))
    );
    let rhai = ScriptRequest {
        request: String::from("hi"),
        repair: None,
        timeout_ms: 1,
        lang: Lang::Rhai,
    };
    let refused: Vec<Start> = [rhai.encode(), String::from("nonsense")]
        .iter()
        .map(|text| jobs.start(text, true))
        .collect();
    assert!(
        refused
            .iter()
            .all(|s| matches!(s, Start::Reply(r) if r.starts_with("err ")))
    );
}

#[test]
fn a_finished_job_stopped_before_its_take_is_dropped() {
    let mut jobs = Jobs::new(0);
    let ticket = started(&mut jobs, "timer");
    jobs.finish(ticket, String::from("ok x"));
    assert_eq!(jobs.stop(&ticket.to_string()), "ok");
    assert_eq!(jobs.take(&ticket.to_string()), "ended unknown ticket");
}
