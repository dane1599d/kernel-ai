//! A window's input and the OS's events as transactions: inside `begin` and
//! `finish` (what `livecore::Host::serve_with` wraps them in), an `on-ui` or
//! `on-event` call may use storage and effects. Outside, it traps, as before.

extern crate std;

use crate::{AppEvent, Limits, load_core};
use alloc::vec::Vec;
use appabi::Guest;
use livecore::Instance;
use livecore::txn::{Effect, Inputs, Storage};
use uikit::widget::UiEvent;

const LIMITS: Limits = Limits {
    fuel_call: 5_000_000,
    fuel_life: 20_000_000,
    memory_bytes: 2 * 1024 * 1024,
};

/// Where `on-event` leaves what it was given: its five flat arguments.
const EVENT: u32 = 900;

/// An app whose `on-ui` writes `key` and queues a notify, and whose `on-event`
/// keeps its arguments where the test can read them.
const GUEST: &str = r#"(module
  (import "kai:app/effects@0.1.0" "request" (func $request (param i32 i32 i32 i32 i32) (result i32)))
  (import "kai:app/storage@0.1.0" "put" (func $put (param i32 i32 i32 i32)))
  (memory (export "memory") 1)
  (data (i32.const 100) "key")
  (data (i32.const 110) "value")
  (data (i32.const 120) "pressed")
  (global $top (mut i32) (i32.const 1024))
  (func (export "cabi_realloc") (param i32 i32 i32 i32) (result i32)
    (local $at i32)
    global.get $top
    local.get 2
    i32.add
    i32.const 1
    i32.sub
    i32.const 0
    local.get 2
    i32.sub
    i32.and
    local.tee $at
    local.get 3
    i32.add
    global.set $top
    local.get $at)
  (func (export "kai:ui/input@0.1.0#on-ui") (param i32 i32 i32 i32 i32 i32)
    (call $put (i32.const 100) (i32.const 3) (i32.const 110) (i32.const 5))
    (drop (call $request (i32.const 1) (i32.const 120) (i32.const 7) (i32.const 0) (i32.const 0))))
  (func (export "kai:app/events@0.1.0#on-event") (param i32 i32 i32 i32 i32)
    (i32.store (i32.const 900) (local.get 0))
    (i32.store (i32.const 904) (local.get 1))
    (i32.store (i32.const 908) (local.get 2))
    (i32.store (i32.const 912) (local.get 3))
    (i32.store (i32.const 916) (local.get 4))))"#;

fn guest() -> crate::CoreInstance {
    load_core(&wat::parse_str(GUEST).unwrap(), LIMITS, None).unwrap()
}

fn words(g: &mut crate::CoreInstance) -> Vec<u32> {
    let bytes = g.read(EVENT, 20).unwrap();
    bytes
        .chunks(4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}

#[test]
fn on_ui_outside_a_transaction_has_no_storage_or_effects() {
    let mut g = guest();
    assert!(g.ui_events(1, &[UiEvent::Pressed(2)]).is_err());
}

#[test]
fn on_ui_inside_one_writes_storage_and_queues_effects() {
    let mut g = guest();
    let inputs = Inputs {
        now_ms: 5,
        seed: 6,
        first_ticket: 7,
    };
    g.begin(&Storage::new(), inputs);
    g.ui_events(1, &[UiEvent::Pressed(2), UiEvent::Pressed(3)])
        .unwrap();
    let txn = g.finish();
    // Two events, one transaction: both effects, tickets from the first.
    let pressed = Effect::Notify("pressed".into());
    assert_eq!(txn.effects, [pressed.clone(), pressed]);
    assert_eq!(txn.effect_lines(7).lines().count(), 2);
    assert_eq!(txn.writes.get("key"), Some(&Some(b"value".to_vec())));
}

#[test]
fn an_event_from_the_os_reaches_on_event_as_its_wit_variant() {
    let mut g = guest();
    g.deliver_event(&AppEvent::Started).unwrap();
    assert_eq!(words(&mut g)[0], 0);

    g.deliver_event(&AppEvent::EffectDone {
        ticket: 9,
        outcome: Ok("fine".into()),
    })
    .unwrap();
    let w = words(&mut g);
    // effect-done (case 1), ticket 9, ok (case 0), then the text's place and size.
    assert_eq!((w[0], w[1], w[2], w[4]), (1, 9, 0, 4));
    assert_eq!(g.read(w[3], 4).unwrap(), b"fine");

    g.deliver_event(&AppEvent::EffectDone {
        ticket: 10,
        outcome: Err("refused".into()),
    })
    .unwrap();
    let w = words(&mut g);
    assert_eq!((w[0], w[1], w[2], w[4]), (1, 10, 1, 7));
    assert_eq!(g.read(w[3], 7).unwrap(), b"refused");
}

#[test]
fn an_app_without_events_is_told_nothing() {
    let wat = r#"(module (memory (export "memory") 1)
      (func (export "cabi_realloc") (param i32 i32 i32 i32) (result i32) i32.const 8))"#;
    let mut g = load_core(&wat::parse_str(wat).unwrap(), LIMITS, None).unwrap();
    assert!(g.deliver_event(&AppEvent::Started).is_ok());
}
