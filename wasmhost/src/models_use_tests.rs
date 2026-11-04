//! `kai:models` `use-model` (#556): the switch it queues, as a change marked
//! with whether a person's key or click made it, what it traps on, and the
//! answers `on-used` takes, through a hand-written canonical-ABI guest.

extern crate std;

use crate::models::{MAX_REPLY, MAX_USES, SHA256_HEX, SWITCH, is_sha256_hex};
use crate::settings::{Kind, SettingsRequest};
use crate::{CoreInstance, Grants, Limits, load_core_granted};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use livecore::{Instance, Reason};
use uikit::widget::UiEvent;

const LIMITS: Limits = Limits {
    fuel_call: 5_000_000,
    fuel_life: 20_000_000,
    memory_bytes: 2 * 1024 * 1024,
};

/// A SHA-256 as the window sends it.
const SHA: &str = "3605803b982cb64aead44f6c1b2ae36e3acdb41d8e46c8a94c6533bc4c67e597";

/// A guest whose `health` and `on-ui` each switch to the models in `texts`, one
/// after the other (`health` returns the last ticket), and whose `on-used`
/// keeps the ticket, the result's case and the text's length, which
/// `self-test` reads back as one number. `trap` ends `health` in an
/// unreachable after the switches.
fn guest_with(texts: &[&str], trap: bool, exports_used: bool) -> Vec<u8> {
    let mut data = String::new();
    let mut calls = String::new();
    for (i, text) in texts.iter().enumerate() {
        let at = 32 + i * 100;
        data.push_str(&format!("(data (i32.const {at}) \"{text}\")\n"));
        calls.push_str(&format!(
            "i32.const {at}\ni32.const {}\ncall $use\n",
            text.len()
        ));
        if i + 1 < texts.len() {
            calls.push_str("drop\n");
        }
    }
    let on_ui = calls.replace("call $use\n", "call $use\ndrop\n");
    let on_ui = on_ui.replace("drop\ndrop\n", "drop\n");
    let trap = if trap { "unreachable" } else { "" };
    let on_used = if exports_used {
        r#"(func (export "kai:models/listing@0.1.1#on-used") (param i32 i32 i32 i32)
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
  (import "kai:models/models@0.1.1" "use-model" (func $use (param i32 i32) (result i32)))
  (memory (export "memory") 2)
  {data}
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
    {calls}
    {trap})
  (func (export "kai:ui/input@0.1.0#on-ui") (param i32 i32 i32 i32 i32 i32)
    {on_ui})
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
  {on_used})"#
    );
    wat::parse_str(source).unwrap()
}

fn granted() -> Grants {
    Grants {
        models: true,
        ..Grants::default()
    }
}

fn app_using(texts: &[&str]) -> CoreInstance {
    load_core_granted(&guest_with(texts, false, true), LIMITS, None, granted()).unwrap()
}

fn requests(sent: &[SettingsRequest]) -> Vec<&str> {
    sent.iter().map(|r| r.request.as_str()).collect()
}

#[test]
fn a_sha256_is_64_lowercase_hex_digits_and_nothing_else() {
    assert!(is_sha256_hex(SHA));
    assert_eq!(SHA.len(), SHA256_HEX);
    for bad in [
        "",
        &SHA[..63],
        &format!("{SHA}0"),
        &SHA.to_ascii_uppercase(),
        &format!("{}g", &SHA[..63]),
        &format!("{} ", &SHA[..63]),
    ] {
        assert!(!is_sha256_hex(bad), "{bad:?}");
    }
}

#[test]
fn a_switch_queues_a_change_naming_the_model_and_taking_empties_the_queue() {
    let mut app = app_using(&[SHA]);
    assert_eq!(app.health().unwrap(), 0, "the first ticket");
    assert_eq!(app.health().unwrap(), 1);
    let sent = app.take_model_uses();
    assert_eq!(
        requests(&sent),
        [format!("{SWITCH}{SHA}"), format!("{SWITCH}{SHA}")]
    );
    assert!(sent.iter().all(|r| r.kind == Kind::Change));
    assert_eq!((sent[0].ticket, sent[1].ticket), (0, 1));
    assert!(app.take_model_uses().is_empty());
    // Tickets go on, so an answer is never taken for a later switch's.
    assert_eq!(app.health().unwrap(), 2);
}

#[test]
fn switches_queue_apart_from_reads() {
    let mut app = app_using(&[SHA]);
    app.health().unwrap();
    assert!(app.take_models().is_empty());
    assert!(
        !app.models_waiting(),
        "a switch is not a read waiting for a list"
    );
    assert_eq!(app.take_model_uses().len(), 1);
}

#[test]
fn a_switch_is_the_persons_only_when_a_real_key_or_click_caused_the_call() {
    let press = [UiEvent::Pressed(5)];
    let mut app = app_using(&[SHA]);
    app.ui_events_user(1, &press).unwrap();
    let sent = app.take_model_uses();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].user, "queued while a person's input was handled");

    // The same event from anything else, such as the agent's `ui_act`, is not.
    app.ui_events(1, &press).unwrap();
    assert!(!app.take_model_uses()[0].user);

    // And the mark does not outlast the input.
    app.ui_events_user(1, &press).unwrap();
    app.take_model_uses();
    app.health().unwrap();
    assert!(!app.take_model_uses()[0].user);
}

#[test]
fn a_call_may_make_one_switch() {
    let mut app = app_using(&[SHA, SHA]);
    let fault = app.health().expect_err("the second switch traps");
    assert!(fault.detail.contains("one switch"), "{}", fault.detail);
    assert!(
        app.take_model_uses().is_empty(),
        "a call that faulted leaves nothing queued"
    );
}

#[test]
fn a_full_queue_refuses_another_switch() {
    let mut app = app_using(&[SHA]);
    for ticket in 0..MAX_USES as u32 {
        assert_eq!(app.health().unwrap(), ticket);
    }
    let fault = app
        .health()
        .expect_err("a full queue refuses another switch");
    assert!(fault.detail.contains("waiting"), "{}", fault.detail);
    assert_eq!(app.take_model_uses().len(), MAX_USES);
}

#[test]
fn a_call_that_faults_takes_back_its_switch_and_only_that() {
    let mut app =
        load_core_granted(&guest_with(&[SHA], true, true), LIMITS, None, granted()).unwrap();
    app.health().expect_err("it ends in unreachable");
    assert!(app.take_model_uses().is_empty());
}

#[test]
fn a_sha256_out_of_shape_traps_the_call() {
    for bad in [&SHA[..63], &SHA.to_ascii_uppercase(), "no model"] {
        let mut app = app_using(&[bad]);
        let fault = app.health().expect_err("a bad SHA-256 traps");
        assert!(fault.detail.contains("use-model"), "{}", fault.detail);
        assert!(app.take_model_uses().is_empty());
    }
}

#[test]
fn an_answer_reaches_the_app_with_its_ticket_and_is_cut_down_when_it_must_be() {
    let mut app = app_using(&[SHA]);
    app.models_used(7, Ok("Done".to_string())).unwrap();
    // The ticket in the top byte, the result's case next (0 is ok), then the
    // text's length.
    assert_eq!(app.self_test().unwrap(), 7 << 24 | 4);
    app.models_used(8, Err("a change needs your key or click".to_string()))
        .unwrap();
    assert_eq!(app.self_test().unwrap(), 8 << 24 | 1 << 16 | 32);
    app.models_used(9, Ok("m".repeat(MAX_REPLY + 1))).unwrap();
    // "the answer is too large" (23 bytes), as an error.
    assert_eq!(app.self_test().unwrap(), 9 << 24 | 1 << 16 | 23);
    app.models_used(10, Err("e".repeat(1000))).unwrap();
    assert_eq!(app.self_test().unwrap(), 10 << 24 | 1 << 16 | 200);
}

#[test]
fn an_app_that_exports_no_on_used_is_told_nothing() {
    let mut app =
        load_core_granted(&guest_with(&[SHA], false, false), LIMITS, None, granted()).unwrap();
    assert_eq!(app.models_used(1, Ok(String::new())), Ok(()));
}

#[test]
fn use_model_needs_the_models_grant() {
    let guest = guest_with(&[SHA], false, true);
    for other in [
        Grants::default(),
        Grants {
            settings: true,
            ..Grants::default()
        },
        Grants {
            system: true,
            ..Grants::default()
        },
    ] {
        let fault = match load_core_granted(&guest, LIMITS, None, other) {
            Err(fault) => fault,
            Ok(_) => panic!("linked without the models grant"),
        };
        assert_eq!(fault.reason, Reason::Link);
        assert!(fault.detail.contains("models grant"), "{}", fault.detail);
    }
}
