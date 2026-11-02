//! `kai:models`: the grant that links it, the queue `read` fills, and the lists
//! `on-listing` takes, through a hand-written canonical-ABI guest.

extern crate std;

use crate::models::{MAX_QUEUED, MAX_REPLY};
use crate::{CoreInstance, Grants, Limits, load_core, load_core_granted};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use livecore::{Fault, Instance, Reason};

const LIMITS: Limits = Limits {
    fuel_call: 5_000_000,
    fuel_life: 20_000_000,
    memory_bytes: 2 * 1024 * 1024,
};

/// A guest whose `health` makes `reads` reads (and returns the last ticket, or 0
/// for none), and whose `on-listing` keeps the ticket, the result's case and the
/// text's length, which `self-test` reads back as one number. `trap` ends
/// `health` in an unreachable after the reads.
fn guest_with(reads: usize, trap: bool, exports_listing: bool) -> Vec<u8> {
    let mut body = String::new();
    for i in 0..reads {
        body.push_str("call $read\n");
        if i + 1 < reads {
            body.push_str("drop\n");
        }
    }
    if reads == 0 {
        body.push_str("i32.const 0\n");
    }
    let trap = if trap { "unreachable" } else { "" };
    let on_listing = if exports_listing {
        r#"(func (export "kai:models/listing@0.1.0#on-listing") (param i32 i32 i32 i32)
    local.get 0
    global.set $ticket
    local.get 1
    global.set $case
    local.get 3
    global.set $size)"#
    } else {
        ""
    };
    let source = format!(
        r#"(module
  (import "kai:models/models@0.1.0" "read" (func $read (result i32)))
  (memory (export "memory") 2)
  (global $top (mut i32) (i32.const 40000))
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
  (func (export "kai:live/lifecycle@0.1.0#health") (result i32)
    {body}
    {trap})
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
  {on_listing})"#
    );
    wat::parse_str(source).unwrap()
}

fn granted() -> Grants {
    Grants {
        models: true,
        ..Grants::default()
    }
}

fn app_reading(reads: usize) -> CoreInstance {
    load_core_granted(&guest_with(reads, false, true), LIMITS, None, granted()).unwrap()
}

fn refused(result: Result<CoreInstance, Fault>) -> Fault {
    match result {
        Err(fault) => fault,
        Ok(_) => panic!("expected a fault"),
    }
}

#[test]
fn an_app_without_the_grant_does_not_link() {
    let guest = guest_with(1, false, true);
    let fault = refused(load_core(&guest, LIMITS, None));
    assert_eq!(fault.reason, Reason::Link);
    assert!(fault.detail.contains("models grant"), "{}", fault.detail);
    // No other grant is the models grant: the System monitor's least of all.
    for other in [
        Grants {
            system: true,
            ..Grants::default()
        },
        Grants {
            settings: true,
            ..Grants::default()
        },
    ] {
        let fault = refused(load_core_granted(&guest, LIMITS, None, other));
        assert_eq!(fault.reason, Reason::Link);
    }
    assert!(load_core_granted(&guest, LIMITS, None, granted()).is_ok());
}

#[test]
fn the_models_grant_opens_no_other_interface() {
    for (import, grant) in [
        (
            r#"(import "kai:system/system@0.1.0" "read" (func (result i32)))"#,
            "system grant",
        ),
        (
            r#"(import "kai:settings/settings@0.1.0" "send" (func (param i32 i32) (result i32)))"#,
            "settings grant",
        ),
    ] {
        let bytes =
            wat::parse_str(format!(r#"(module {import} (memory (export "memory") 1))"#)).unwrap();
        let fault = refused(load_core_granted(&bytes, LIMITS, None, granted()));
        assert_eq!(fault.reason, Reason::Link);
        assert!(fault.detail.contains(grant), "{}", fault.detail);
    }
}

#[test]
fn read_queues_a_ticket_and_taking_empties_the_queue() {
    let mut app = app_reading(1);
    assert!(!app.models_waiting());
    assert_eq!(app.health().unwrap(), 0, "the first ticket");
    assert!(app.models_waiting());
    assert_eq!(app.health().unwrap(), 1);
    assert_eq!(app.take_models(), vec![0, 1]);
    assert!(app.take_models().is_empty());
    assert!(!app.models_waiting());
    // Tickets go on counting after the queue was taken.
    assert_eq!(app.health().unwrap(), 2);
    assert_eq!(app.take_models(), vec![2]);
}

#[test]
fn the_models_queue_is_its_own() {
    let mut app = app_reading(1);
    app.health().unwrap();
    assert!(app.take_system().is_empty());
    assert_eq!(app.take_models(), vec![0]);
}

#[test]
fn a_call_may_make_the_most_reads_and_the_next_one_traps() {
    let mut app = app_reading(MAX_QUEUED);
    assert_eq!(app.health().unwrap(), MAX_QUEUED as u32 - 1);
    let fault = app.health().expect_err("a full queue refuses another read");
    assert!(fault.detail.contains("waiting"), "{}", fault.detail);
    assert_eq!(
        app.take_models(),
        (0..MAX_QUEUED as u32).collect::<Vec<_>>()
    );
}

#[test]
fn a_call_that_faults_leaves_nothing_queued() {
    let mut app = load_core_granted(&guest_with(2, true, true), LIMITS, None, granted()).unwrap();
    app.health().expect_err("it ends in unreachable");
    assert!(app.take_models().is_empty());
}

#[test]
fn a_new_instance_starts_again_from_ticket_zero_with_nothing_waiting() {
    let mut old = app_reading(1);
    old.health().unwrap();
    let mut new = app_reading(1);
    assert!(new.take_models().is_empty());
    assert_eq!(new.health().unwrap(), 0);
    assert_eq!(new.take_models(), vec![0]);
    assert_eq!(old.take_models(), vec![0]);
}

#[test]
fn a_list_reaches_the_app_as_a_result_with_its_ticket() {
    let mut app = app_reading(1);
    app.models_listing(7, Ok("kai-models 1\n".to_string()))
        .unwrap();
    // The ticket in the top byte, the result's case next (0 is ok), then the
    // text's length.
    assert_eq!(app.self_test().unwrap(), 7 << 24 | 13);
    app.models_listing(9, Err("there is no model store".to_string()))
        .unwrap();
    assert_eq!(app.self_test().unwrap(), 9 << 24 | 1 << 16 | 23);
}

#[test]
fn a_big_list_and_a_long_or_odd_error_are_cut_down_to_what_the_app_can_take() {
    let mut app = app_reading(1);
    app.models_listing(1, Ok("m".repeat(MAX_REPLY))).unwrap();
    assert_eq!(app.self_test().unwrap(), 1 << 24 | MAX_REPLY as u32);
    app.models_listing(2, Ok("m".repeat(MAX_REPLY + 1)))
        .unwrap();
    // An error: case 1, the text "the list is too large" (21 bytes).
    assert_eq!(app.self_test().unwrap(), 2 << 24 | 1 << 16 | 21);
    app.models_listing(3, Err("e".repeat(1000))).unwrap();
    assert_eq!(app.self_test().unwrap(), 3 << 24 | 1 << 16 | 200);
}

#[test]
fn an_app_that_exports_no_listing_is_told_nothing() {
    let mut app = load_core_granted(&guest_with(1, false, false), LIMITS, None, granted()).unwrap();
    assert_eq!(app.models_listing(1, Ok(String::new())), Ok(()));
}
