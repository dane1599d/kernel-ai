//! `HostState::user_input`: the flag a granted import reads to tell a person's
//! key or click from the agent's `ui_act`. It is true only inside
//! `ui_events_user` (and so `ui_input`), is never left set after a call, a fault
//! or an error, and every other source of a call leaves it false.

extern crate std;

use crate::{CoreInstance, Limits, load_core};
use alloc::vec::Vec;
use livecore::Instance;
use uikit::widget::UiEvent;

const LIMITS: Limits = Limits {
    fuel_call: 5_000_000,
    fuel_life: 20_000_000,
    memory_bytes: 2 * 1024 * 1024,
};

/// A guest with `health` and `on-ui` and nothing else; `on-ui` ends in an
/// unreachable when `trap` is set.
fn guest(trap: bool) -> Vec<u8> {
    let trap = if trap { "unreachable" } else { "" };
    let source = alloc::format!(
        r#"(module
  (memory (export "memory") 1)
  (func (export "cabi_realloc") (param i32 i32 i32 i32) (result i32) i32.const 4096)
  (func (export "kai:live/lifecycle@0.1.0#health") (result i32) i32.const 0)
  (func (export "kai:ui/input@0.1.0#on-ui") (param i32 i32 i32 i32 i32 i32) {trap}))"#
    );
    wat::parse_str(source).unwrap()
}

fn app(trap: bool) -> CoreInstance {
    load_core(&guest(trap), LIMITS, None).unwrap()
}

const PRESS: [UiEvent; 1] = [UiEvent::Pressed(1)];

#[test]
fn a_call_is_a_persons_only_inside_ui_events_user() {
    let mut g = app(false);
    assert!(!g.last_call_by_user(), "nothing has run yet");
    g.ui_events_user(1, &PRESS).unwrap();
    assert!(g.last_call_by_user());
    // The same event from anything else, such as the agent's `ui_act`.
    g.ui_events(1, &PRESS).unwrap();
    assert!(!g.last_call_by_user());
    // Each event of a person's input is a call of its own, all of them theirs.
    g.ui_events_user(1, &[UiEvent::Pressed(1), UiEvent::Pressed(2)])
        .unwrap();
    assert!(g.last_call_by_user());
}

#[test]
fn the_mark_does_not_outlast_the_input() {
    let mut g = app(false);
    g.ui_events_user(1, &PRESS).unwrap();
    assert!(g.last_call_by_user());
    // A lifecycle call or an event after it is nobody's input.
    assert_eq!(g.health().unwrap(), 0);
    assert!(!g.last_call_by_user());
    assert!(!g.store.data().user_input);
}

#[test]
fn a_fault_in_the_input_s_call_does_not_leave_the_mark_set() {
    let mut g = app(true);
    g.ui_events_user(1, &PRESS).expect_err("on-ui traps");
    assert!(!g.store.data().user_input);
    assert_eq!(g.health().unwrap(), 0);
    assert!(!g.last_call_by_user());
    // And a trap in an ordinary events call never had it.
    g.ui_events(1, &PRESS).expect_err("on-ui traps");
    assert!(!g.last_call_by_user());
}

#[test]
fn an_app_with_no_on_ui_is_given_nothing_and_stays_unmarked() {
    let bytes = wat::parse_str(
        r#"(module
  (memory (export "memory") 1)
  (func (export "cabi_realloc") (param i32 i32 i32 i32) (result i32) i32.const 4096)
  (func (export "kai:live/lifecycle@0.1.0#health") (result i32) i32.const 0))"#,
    )
    .unwrap();
    let mut g = load_core(&bytes, LIMITS, None).unwrap();
    g.ui_events_user(1, &PRESS).unwrap();
    assert!(!g.last_call_by_user());
    assert!(!g.store.data().user_input);
}
