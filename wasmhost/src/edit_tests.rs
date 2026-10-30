//! `kai:edit`: the grant that links it, the queue `send` fills and the answers
//! `on-reply` takes, through a hand-written canonical-ABI guest.

extern crate std;

use crate::{CoreInstance, Grants, Limits, load_core, load_core_granted};
use alloc::string::String;
use alloc::vec::Vec;
use livecore::txn::{Inputs, Storage};
use livecore::{Fault, Instance, Reason};

const LIMITS: Limits = Limits {
    fuel_call: 5_000_000,
    fuel_life: 20_000_000,
    memory_bytes: 2 * 1024 * 1024,
};

/// A guest whose `health` sends "open notes" and returns its ticket, and whose
/// `on-reply` keeps the ticket, the result's case and the payload's length, which
/// `self-test` reads back as one number.
const GUEST: &str = r#"(module
  (import "kai:edit/edit@0.1.0" "send" (func $send (param i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 32) "open notes")
  (global $top (mut i32) (i32.const 4096))
  (global $ticket (mut i32) (i32.const 0))
  (global $case (mut i32) (i32.const 0))
  (global $size (mut i32) (i32.const 0))
  (func (export "cabi_realloc") (param i32 i32 i32 i32) (result i32)
    (local $at i32)
    global.get $top
    local.set $at
    global.get $top
    local.get 3
    i32.add
    global.set $top
    local.get $at)
  (func (export "kai:ui/input@0.1.0#on-ui") (param i32 i32 i32 i32 i32 i32)
    (drop (call $send (i32.const 32) (i32.const 10))))
  (func (export "kai:live/lifecycle@0.1.0#health") (result i32)
    i32.const 32
    i32.const 10
    call $send)
  (func (export "kai:live/lifecycle@0.1.0#self-test") (result i32)
    global.get $ticket
    i32.const 24
    i32.shl
    global.get $case
    i32.const 16
    i32.shl
    i32.or
    global.get $size
    i32.or)
  (func (export "kai:edit/replies@0.1.0#on-reply") (param i32 i32 i32 i32)
    ;; Ticket 99 is an app that traps.
    local.get 0
    i32.const 99
    i32.eq
    if
      unreachable
    end
    local.get 0
    global.set $ticket
    local.get 1
    global.set $case
    local.get 3
    global.set $size))"#;

fn guest() -> Vec<u8> {
    wat::parse_str(GUEST).unwrap()
}

fn refused(result: Result<CoreInstance, Fault>) -> Fault {
    match result {
        Err(fault) => fault,
        Ok(_) => panic!("expected a fault"),
    }
}

/// Runs `f` inside a transaction, as `Host::serve_with` wraps a call.
fn in_call<R>(app: &mut CoreInstance, f: impl FnOnce(&mut CoreInstance) -> R) -> R {
    let inputs = Inputs {
        now_ms: 0,
        seed: 0,
        first_ticket: 0,
    };
    app.begin(&Storage::new(), inputs);
    let result = f(app);
    app.finish();
    result
}

fn granted() -> Grants {
    Grants {
        edit: true,
        ..Grants::default()
    }
}

#[test]
fn an_app_without_the_grant_does_not_link() {
    let fault = refused(load_core(&guest(), LIMITS, None));
    assert_eq!(fault.reason, Reason::Link);
    assert!(fault.detail.contains("edit grant"), "{}", fault.detail);
    let fault = refused(load_core_granted(&guest(), LIMITS, None, Grants::default()));
    assert_eq!(fault.reason, Reason::Link);
    assert!(load_core_granted(&guest(), LIMITS, None, granted()).is_ok());
}

#[test]
fn send_queues_a_message_with_a_ticket_and_taking_empties_the_queue() {
    let mut app = load_core_granted(&guest(), LIMITS, None, granted()).unwrap();
    assert_eq!(
        in_call(&mut app, |a| a.health()).unwrap(),
        0,
        "the first ticket"
    );
    assert_eq!(in_call(&mut app, |a| a.health()).unwrap(), 1);
    let sent = app.take_edits();
    assert_eq!(sent.len(), 2);
    assert_eq!(
        (sent[0].ticket, sent[0].message.as_slice()),
        (0, &b"open notes"[..])
    );
    assert_eq!(sent[1].ticket, 1);
    assert!(app.take_edits().is_empty());
    // Tickets go on, so an answer is never taken for a later message's.
    assert_eq!(in_call(&mut app, |a| a.health()).unwrap(), 2);
}

#[test]
fn a_message_says_whether_a_person_was_at_the_keys() {
    use uikit::widget::UiEvent;
    let mut app = load_core_granted(&guest(), LIMITS, None, granted()).unwrap();
    let press = [UiEvent::Pressed(2)];
    // A real key or click, the agent's action or an event, and an answer's call.
    in_call(&mut app, |a| a.ui_events_user(1, &press)).unwrap();
    in_call(&mut app, |a| a.ui_events(1, &press)).unwrap();
    in_call(&mut app, |a| a.health()).unwrap();
    let sent = app.take_edits();
    let users: std::vec::Vec<bool> = sent.iter().map(|r| r.user).collect();
    assert_eq!(users, [true, false, false]);
    // The flag does not stay up after the person's call.
    in_call(&mut app, |a| a.ui_events(1, &press)).unwrap();
    assert!(!app.take_edits()[0].user);
}

#[test]
fn send_outside_a_transaction_is_a_fault() {
    // A probation's `health`, or any call a transaction does not wrap, cannot send.
    let mut app = load_core_granted(&guest(), LIMITS, None, granted()).unwrap();
    let fault = app.health().expect_err("no transaction");
    assert!(fault.detail.contains("transaction"), "{}", fault.detail);
    assert!(app.take_edits().is_empty());
}

#[test]
fn too_many_messages_before_comphost_takes_them_is_a_fault() {
    let mut app = load_core_granted(&guest(), LIMITS, None, granted()).unwrap();
    for _ in 0..crate::edit::MAX_QUEUED {
        in_call(&mut app, |a| a.health()).unwrap();
    }
    let fault = in_call(&mut app, |a| a.health()).expect_err("one past the limit traps");
    assert!(fault.detail.contains("queued"), "{}", fault.detail);
    assert_eq!(app.take_edits().len(), crate::edit::MAX_QUEUED);
}

#[test]
fn a_reply_reaches_the_app_as_a_result_with_its_ticket() {
    let mut app = load_core_granted(&guest(), LIMITS, None, granted()).unwrap();
    app.edit_reply(7, Ok(b"ok editing notes 2".to_vec()))
        .unwrap();
    // The ticket in the top byte, the result's case next (0 is ok), then the
    // payload's length.
    assert_eq!(app.self_test().unwrap(), 7 << 24 | 18);
    app.edit_reply(9, Err(String::from("the edit service is not running")))
        .unwrap();
    assert_eq!(app.self_test().unwrap(), 9 << 24 | 1 << 16 | 31);
}

#[test]
fn a_trap_in_on_reply_comes_back_as_the_calls_fault_not_swallowed() {
    // comphost runs the answer inside `Host::serve_with`, which reports and
    // reloads a version that faulted; that needs the fault here.
    let mut app = load_core_granted(&guest(), LIMITS, None, granted()).unwrap();
    let fault = in_call(&mut app, |a| a.edit_reply(99, Ok(Vec::new()))).expect_err("a trap");
    assert_ne!(fault.reason, Reason::Link);
    // The next answer still works on a fresh call.
    let mut app = load_core_granted(&guest(), LIMITS, None, granted()).unwrap();
    assert_eq!(
        in_call(&mut app, |a| a.edit_reply(5, Ok(Vec::new()))),
        Ok(())
    );
}

#[test]
fn an_app_that_exports_no_replies_is_told_nothing() {
    let no_replies = GUEST.replace(
        r#"(func (export "kai:edit/replies@0.1.0#on-reply")"#,
        r#"(func (export "other")"#,
    );
    let mut app = load_core_granted(
        &wat::parse_str(&no_replies).unwrap(),
        LIMITS,
        None,
        granted(),
    )
    .unwrap();
    assert_eq!(app.edit_reply(1, Ok(Vec::new())), Ok(()));
}
