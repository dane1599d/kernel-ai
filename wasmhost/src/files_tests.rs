//! `kai:files`: the grant that links it, the queue `read` and `write` fill and the
//! answers `on-done` takes, through a hand-written canonical-ABI guest.

extern crate std;

use crate::files::{FileOp, MAX_DATA, MAX_QUEUED};
use crate::{CoreInstance, Grants, Limits, load_core, load_core_granted};
use alloc::string::String;
use alloc::vec::Vec;
use livecore::txn::{Inputs, Storage};
use livecore::{Fault, Instance, Reason};
use uikit::widget::UiEvent;

const LIMITS: Limits = Limits {
    fuel_call: 5_000_000,
    fuel_life: 20_000_000,
    memory_bytes: 2 * 1024 * 1024,
};

/// A guest whose `health` reads `/home/user/a.txt` and returns its ticket; whose
/// `on-ui` writes "hi" there with `create` (window 1), reads a relative path
/// (window 9), writes with a length over the limit (window 8) or reads a path
/// with `..` (window 7); and whose `on-done` keeps the ticket, the result's case
/// and the payload's length, which `self-test` reads back as one number.
const GUEST: &str = r#"(module
  (import "kai:files/files@0.1.0" "read" (func $read (param i32 i32) (result i32)))
  (import "kai:files/files@0.1.0" "write" (func $write (param i32 i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 32) "/home/user/a.txt")
  (data (i32.const 64) "hi")
  (data (i32.const 80) "a.txt")
  (data (i32.const 96) "/home/../etc")
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
    local.get 0
    i32.const 9
    i32.eq
    if
      (drop (call $read (i32.const 80) (i32.const 5)))
      return
    end
    local.get 0
    i32.const 7
    i32.eq
    if
      (drop (call $read (i32.const 96) (i32.const 12)))
      return
    end
    local.get 0
    i32.const 8
    i32.eq
    if
      (drop (call $write (i32.const 32) (i32.const 16) (i32.const 64) (i32.const 262145) (i32.const 0)))
      return
    end
    (drop (call $write (i32.const 32) (i32.const 16) (i32.const 64) (i32.const 2) (i32.const 1))))
  (func (export "kai:live/lifecycle@0.1.0#health") (result i32)
    i32.const 32
    i32.const 16
    call $read)
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
  (func (export "kai:files/replies@0.1.0#on-done") (param i32 i32 i32 i32)
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
        files: true,
        ..Grants::default()
    }
}

fn app() -> CoreInstance {
    load_core_granted(&guest(), LIMITS, None, granted()).unwrap()
}

const PRESS: [UiEvent; 1] = [UiEvent::Pressed(2)];

#[test]
fn an_app_without_the_grant_does_not_link() {
    let fault = refused(load_core(&guest(), LIMITS, None));
    assert_eq!(fault.reason, Reason::Link);
    assert!(fault.detail.contains("files grant"), "{}", fault.detail);
    let fault = refused(load_core_granted(&guest(), LIMITS, None, Grants::default()));
    assert_eq!(fault.reason, Reason::Link);
    // The edit grant is not the files grant.
    let edit = Grants {
        edit: true,
        ..Grants::default()
    };
    assert_eq!(
        refused(load_core_granted(&guest(), LIMITS, None, edit)).reason,
        Reason::Link
    );
    assert!(load_core_granted(&guest(), LIMITS, None, granted()).is_ok());
}

#[test]
fn read_and_write_queue_requests_with_tickets_and_taking_empties_the_queue() {
    let mut app = app();
    assert_eq!(
        in_call(&mut app, |a| a.health()).unwrap(),
        0,
        "the first ticket"
    );
    in_call(&mut app, |a| a.ui_events(1, &PRESS)).unwrap();
    let sent = app.take_files();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].ticket, 0);
    assert_eq!(
        sent[0].op,
        FileOp::Read {
            path: String::from("/home/user/a.txt")
        }
    );
    assert_eq!(sent[1].ticket, 1);
    assert_eq!(
        sent[1].op,
        FileOp::Write {
            path: String::from("/home/user/a.txt"),
            data: b"hi".to_vec(),
            create: true,
        }
    );
    assert!(app.take_files().is_empty());
    // Tickets go on, so an answer is never taken for a later request's.
    assert_eq!(in_call(&mut app, |a| a.health()).unwrap(), 2);
}

#[test]
fn a_request_says_whether_a_person_was_at_the_keys() {
    let mut app = app();
    // A real key or click, the agent's action or an event, and an answer's call.
    in_call(&mut app, |a| a.ui_events_user(1, &PRESS)).unwrap();
    in_call(&mut app, |a| a.ui_events(1, &PRESS)).unwrap();
    let users: Vec<bool> = app.take_files().iter().map(|r| r.user).collect();
    assert_eq!(users, [true, false]);
    // The flag does not stay up after the person's call.
    in_call(&mut app, |a| a.ui_events(1, &PRESS)).unwrap();
    assert!(!app.take_files()[0].user);
}

#[test]
fn a_request_outside_a_transaction_is_a_fault() {
    // A probation's `health`, or any call a transaction does not wrap, cannot ask.
    let mut app = app();
    let fault = app.health().expect_err("no transaction");
    assert!(fault.detail.contains("transaction"), "{}", fault.detail);
    assert!(app.take_files().is_empty());
}

#[test]
fn too_many_requests_before_comphost_takes_them_is_a_fault() {
    let mut app = app();
    for _ in 0..MAX_QUEUED {
        in_call(&mut app, |a| a.health()).unwrap();
    }
    let fault = in_call(&mut app, |a| a.health()).expect_err("over the queue");
    assert!(fault.detail.contains("queued"), "{}", fault.detail);
    assert_eq!(app.take_files().len(), MAX_QUEUED);
    // Taken, there is room again.
    in_call(&mut app, |a| a.health()).unwrap();
}

#[test]
fn a_path_that_is_not_plain_and_absolute_or_a_write_too_big_is_a_fault() {
    for (window, why) in [(9, "absolute"), (7, ".. segments"), (8, "at most")] {
        let mut app = app();
        let fault = in_call(&mut app, |a| a.ui_events(window, &PRESS)).expect_err(why);
        assert!(fault.detail.contains(why), "{window}: {}", fault.detail);
        assert!(app.take_files().is_empty());
    }
    assert_eq!(MAX_DATA, 262_144, "the guest's 262,145 is one over");
}

#[test]
fn an_answer_reaches_the_app_as_a_result_with_the_bytes_or_the_reason() {
    let mut app = app();
    // The bytes of a read: ticket 3, the ok case (0), 5 bytes.
    app.files_reply(3, Ok(b"hello".to_vec())).unwrap();
    assert_eq!(app.self_test().unwrap(), (3 << 24) | 5);
    // A reason: the error case (1), as long as the words.
    app.files_reply(4, Err(String::from("missing: no such file")))
        .unwrap();
    assert_eq!(app.self_test().unwrap(), (4 << 24) | (1 << 16) | 21);
    // An app that traps while it is told is a fault for the host to see.
    assert!(app.files_reply(99, Ok(Vec::new())).is_err());
}

#[test]
fn an_app_with_no_replies_export_is_told_nothing() {
    let bare = r#"(module
      (import "kai:files/files@0.1.0" "read" (func (param i32 i32) (result i32)))
      (import "kai:files/files@0.1.0" "write" (func (param i32 i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (func (export "cabi_realloc") (param i32 i32 i32 i32) (result i32) i32.const 4096))"#;
    let bytes = wat::parse_str(bare).unwrap();
    let mut app = load_core_granted(&bytes, LIMITS, None, granted()).unwrap();
    app.files_reply(0, Ok(Vec::new())).unwrap();
    assert_eq!(app.take_files().len(), 0);
}
