//! `ctl suspend` and `ctl resume` (P3 task 11): the state is saved and the
//! instance dropped, and it comes back holding the same state.

use super::fake::LIVE;
use super::{V1, call, host, kernel, ledger};

fn live() -> usize {
    LIVE.with(|live| live.get())
}

#[test]
fn a_suspended_app_drops_its_instance_and_resumes_with_its_state() {
    let kernel = kernel(&[V1]);
    let mut host = host(&kernel);
    call(&mut host, "add");
    call(&mut host, "add");
    let before = live();
    assert!(!host.suspended());
    assert_eq!(call(&mut host, "ctl suspend"), "ok suspended counter 1");
    assert!(host.suspended());
    assert_eq!(live(), before - 1, "the instance is gone");
    assert_eq!(
        call(&mut host, "ctl status"),
        "ok counter 1 schema 1 suspended"
    );
    assert_eq!(call(&mut host, "ctl suspend"), "err counter is suspended");
    assert_eq!(
        call(&mut host, "ctl swap 1"),
        "err counter is suspended; ctl resume brings it back"
    );
    assert_eq!(call(&mut host, "ctl resume"), "ok resumed counter 1");
    assert!(!host.suspended());
    assert_eq!(live(), before);
    assert_eq!(call(&mut host, "get"), "ok 2 schema 1");
    assert_eq!(
        call(&mut host, "ctl resume"),
        "err counter is not suspended"
    );
}

#[test]
fn a_request_wakes_a_suspended_app() {
    let kernel = kernel(&[V1]);
    let mut host = host(&kernel);
    call(&mut host, "add");
    call(&mut host, "ctl suspend");
    assert_eq!(call(&mut host, "add"), "ok 2");
    assert_eq!(call(&mut host, "ctl status"), "ok counter 1 schema 1");
    // The ledger has what the app holds; a suspend sent no extra checkpoint.
    assert_eq!(ledger(&kernel).counters.checkpoints, 2);
}

#[test]
fn only_an_app_serving_alone_suspends() {
    let kernel = kernel(&[V1, super::V2]);
    let mut host = host(&kernel);
    call(&mut host, "ctl swap 2");
    let reply = call(&mut host, "ctl suspend");
    assert!(reply.contains("finish its probation first"), "{reply}");
}

#[test]
fn wake_resumes_only_a_suspended_app() {
    let kernel = kernel(&[V1]);
    let mut host = host(&kernel);
    call(&mut host, "add");
    assert_eq!(host.wake().unwrap(), None, "a serving app is left alone");
    call(&mut host, "ctl suspend");
    let before = live();
    let reply = host.wake().unwrap().unwrap();
    assert_eq!(reply, b"ok resumed counter 1");
    assert!(!host.suspended());
    assert_eq!(live(), before + 1, "the instance is back");
    assert_eq!(call(&mut host, "get"), "ok 1 schema 1");
    assert_eq!(host.wake().unwrap(), None);
}
