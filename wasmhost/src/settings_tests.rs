//! `kai:settings`: the grant that links it, the queue `send` fills with each
//! request's kind and origin, the requests it refuses and the answers
//! `on-reply` takes, through a hand-written canonical-ABI guest.

extern crate std;

use crate::settings::{
    Kind, MAX_ERROR, MAX_QUEUED, MAX_REPLY, MAX_REQUEST, READS, SettingsRequest, classify,
};
use crate::{CoreInstance, Grants, Limits, SettingsKind, load_core, load_core_granted};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use livecore::{Fault, Instance, Reason};
use uikit::widget::UiEvent;

const LIMITS: Limits = Limits {
    fuel_call: 5_000_000,
    fuel_life: 20_000_000,
    memory_bytes: 2 * 1024 * 1024,
};

/// A guest whose `health` and `on-ui` send the requests in `texts` one after the
/// other (`health` returns the last ticket), and whose `on-reply` keeps the
/// ticket, the result's case and the payload's length, which `self-test` reads
/// back as one number. `trap` ends `health` in an unreachable after the sends.
fn guest_with(texts: &[&str], trap: bool) -> Vec<u8> {
    let mut data = String::new();
    let mut sends = String::new();
    for (i, text) in texts.iter().enumerate() {
        let at = 32 + i * 600;
        data.push_str(&format!("(data (i32.const {at}) \"{text}\")\n"));
        sends.push_str(&format!(
            "i32.const {at}\ni32.const {}\ncall $send\n",
            text.len()
        ));
        if i + 1 < texts.len() {
            sends.push_str("drop\n");
        }
    }
    let on_ui_sends: String = texts
        .iter()
        .enumerate()
        .map(|(i, text)| {
            format!(
                "i32.const {}\ni32.const {}\ncall $send\ndrop\n",
                32 + i * 600,
                text.len()
            )
        })
        .collect();
    let trap = if trap { "unreachable" } else { "" };
    let source = format!(
        r#"(module
  (import "kai:settings/settings@0.1.0" "send" (func $send (param i32 i32) (result i32)))
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
    {sends}
    {trap})
  (func (export "kai:ui/input@0.1.0#on-ui") (param i32 i32 i32 i32 i32 i32)
    {on_ui_sends})
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
  (func (export "kai:settings/replies@0.1.0#on-reply") (param i32 i32 i32 i32)
    local.get 0
    global.set $ticket
    local.get 1
    global.set $case
    local.get 3
    global.set $size))"#
    );
    wat::parse_str(source).unwrap()
}

fn guest() -> Vec<u8> {
    guest_with(&["rules"], false)
}

fn refused(result: Result<CoreInstance, Fault>) -> Fault {
    match result {
        Err(fault) => fault,
        Ok(_) => panic!("expected a fault"),
    }
}

fn granted() -> Grants {
    Grants {
        settings: true,
        ..Grants::default()
    }
}

fn app_sending(texts: &[&str]) -> CoreInstance {
    load_core_granted(&guest_with(texts, false), LIMITS, None, granted()).unwrap()
}

fn requests(sent: &[SettingsRequest]) -> Vec<&str> {
    sent.iter().map(|r| r.request.as_str()).collect()
}

#[test]
fn an_app_without_the_grant_does_not_link() {
    let fault = refused(load_core(&guest(), LIMITS, None));
    assert_eq!(fault.reason, Reason::Link);
    assert!(fault.detail.contains("settings grant"), "{}", fault.detail);
    let fault = refused(load_core_granted(&guest(), LIMITS, None, Grants::default()));
    assert_eq!(fault.reason, Reason::Link);
    assert!(load_core_granted(&guest(), LIMITS, None, granted()).is_ok());
}

#[test]
fn send_queues_a_request_with_a_ticket_and_a_kind_and_taking_empties_the_queue() {
    let mut app = app_sending(&["rules-remove S3"]);
    assert_eq!(app.health().unwrap(), 0, "the first ticket");
    assert_eq!(app.health().unwrap(), 1);
    let sent = app.take_settings();
    assert_eq!(sent.len(), 2);
    assert_eq!(
        (sent[0].ticket, sent[0].request.as_str(), sent[0].kind),
        (0, "rules-remove S3", SettingsKind::Change)
    );
    assert_eq!(sent[1].ticket, 1);
    assert!(app.take_settings().is_empty());
    // Tickets go on, so an answer is never taken for a later request's.
    assert_eq!(app.health().unwrap(), 2);

    let mut read = app_sending(&["file appearance"]);
    read.health().unwrap();
    assert_eq!(read.take_settings()[0].kind, SettingsKind::Read);
}

#[test]
fn a_read_and_a_change_may_share_a_call_but_two_changes_may_not() {
    let mut app = app_sending(&["rules", "grants-revoke notes notes_add"]);
    app.health().unwrap();
    assert_eq!(
        requests(&app.take_settings()),
        ["rules", "grants-revoke notes notes_add"]
    );

    let mut app = app_sending(&[
        "grants-revoke notes notes_add",
        "grants-restore notes notes_add",
    ]);
    let fault = app.health().expect_err("the second change traps");
    assert!(fault.detail.contains("one change"), "{}", fault.detail);
    assert!(
        app.take_settings().is_empty(),
        "a call that faulted leaves nothing queued"
    );
    // A later call has its own allowance.
    let mut app = app_sending(&["grants-revoke notes notes_add"]);
    app.health().unwrap();
    app.health().unwrap();
    assert_eq!(app.take_settings().len(), 2);
}

#[test]
fn a_call_that_faults_takes_back_what_it_queued_and_only_that() {
    let mut app =
        load_core_granted(&guest_with(&["rules"], true), LIMITS, None, granted()).unwrap();
    app.health().expect_err("it ends in unreachable");
    assert!(app.take_settings().is_empty());
    // What an earlier call queued stays.
    let mut app = app_sending(&["rules"]);
    app.health().unwrap();
    let mut other =
        load_core_granted(&guest_with(&["about"], true), LIMITS, None, granted()).unwrap();
    other.health().unwrap_err();
    assert_eq!(app.take_settings().len(), 1);
}

#[test]
fn a_change_is_the_persons_only_when_a_real_key_or_click_caused_the_call() {
    let press = [UiEvent::Pressed(1)];
    let mut app = app_sending(&["profile-set quiet"]);
    app.ui_events_user(1, &press).unwrap();
    let sent = app.take_settings();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].kind, SettingsKind::Change);
    assert!(sent[0].user, "queued while a person's input was handled");

    // The same event from anything else, such as the agent's `ui_act`, is not.
    app.ui_events(1, &press).unwrap();
    assert!(!app.take_settings()[0].user);

    // And the mark does not outlast the input.
    app.ui_events_user(1, &press).unwrap();
    app.take_settings();
    app.health().unwrap();
    assert!(!app.take_settings()[0].user);

    // Nor does a fault in the input's call leave it set.
    let mut trapping = load_core_granted(
        &guest_with(&["profile-set quiet"], false),
        LIMITS,
        None,
        granted(),
    )
    .unwrap();
    trapping.ui_events_user(1, &press).unwrap();
    trapping.take_settings();
    trapping.health().unwrap();
    assert!(!trapping.take_settings()[0].user);
}

#[test]
fn every_read_is_an_exact_line_and_nothing_else_is_a_read() {
    for read in READS {
        assert_eq!(classify(read), Ok(Kind::Read), "{read}");
    }
    // One app's grants, as the shell lists them.
    for read in ["grants notes", "grants files-window", "grants app_2"] {
        assert_eq!(classify(read), Ok(Kind::Read), "{read}");
    }
    let not_reads = [
        "grants ",
        "grants  notes",
        "grants notes ",
        "grants Notes",
        "grants notes x",
        "grants ../notes",
        "grants abcdefghijklmnopqrstuvwxy",
        "rules x",
        "rules ",
        " rules",
        "Rules",
        "rules rules-remove 1",
        "file",
        "file ",
        "file ../x",
        "file appearance ",
        "file appearance x",
        "file Appearance",
        "file /system/settings/appearance",
        "file settings",
        "grants\u{2028}",
        "rules\u{a0}",
        "rules\u{202e}",
    ];
    for text in not_reads {
        assert!(
            classify(text) != Ok(Kind::Read),
            "{text:?} was taken for a read"
        );
    }
}

#[test]
fn every_change_takes_its_own_arguments_and_nothing_looser() {
    let changes = [
        "appearance-set theme light",
        "appearance-set legible on",
        "appearance-set reduced_motion off",
        "appearance-set scale 150",
        "appearance-set token muted #aabbcc",
        "profile-set quiet",
        "profile-set deep focus",
        "rules-remove 3",
        "rules-remove S12",
        "grants-revoke notes notes_add",
        "grants-restore files-window files_open2",
        "keep-running on notes",
        "keep-running off my app",
        "restart-window 120 300",
        "restart-window 1380 60",
        "restart-window off",
    ];
    for text in changes {
        assert_eq!(classify(text), Ok(Kind::Change), "{text}");
    }
    let verbs: Vec<&str> = changes
        .iter()
        .map(|c| c.split(' ').next().unwrap())
        .collect();
    for verb in crate::settings::CHANGES {
        assert!(verbs.contains(verb), "no example for {verb}");
    }
    let refused = [
        "appearance-set",
        "appearance-set theme",
        "appearance-set theme ",
        "appearance-set colour red",
        "appearance-set theme light dark  mode",
        "profile-set",
        "profile-set ",
        "profile-set  quiet",
        "profile-set quiet ",
        "profile-set a  b",
        "profile-set abcdefghijklmnopqrstuvwxy",
        "rules-remove",
        "rules-remove 0x3",
        "rules-remove -1",
        "rules-remove +1",
        "rules-remove 3 4",
        "rules-remove S",
        "rules-remove s3",
        "rules-remove 12345",
        "rules-remove #3",
        "grants-revoke",
        "grants-revoke 7",
        "grants-revoke notes",
        "grants-revoke notes ",
        "grants-revoke  notes notes_add",
        "grants-revoke notes notes_add extra",
        "grants-revoke Notes notes_add",
        "grants-revoke notes notes-add",
        "grants-revoke notes _notes",
        "grants-revoke notes notes_",
        "grants-revoke notes NOTES_ADD",
        "grants-revoke abcdefghijklmnopqrstuvwxy notes_add",
        "grants-restore",
        "grants-restore notes",
        "keep-running",
        "keep-running on",
        "keep-running on ",
        "keep-running yes notes",
        "keep-running notes on",
        "restart-window",
        "restart-window 5",
        "restart-window 5 5",
        "restart-window 1440 5",
        "restart-window 5 1440",
        "restart-window 1 2 3",
        "restart-window OFF",
        "restart-window off now",
        "Rules-remove 3",
        "rules_remove 3",
        "format-disk",
    ];
    for text in refused {
        assert!(classify(text).is_err(), "{text:?}");
    }
}

#[test]
fn only_printable_ascii_gets_through() {
    for text in [
        "profile-set caf\u{e9}",
        "profile-set a\u{200b}b",
        "profile-set a\u{202e}b",
        "profile-set a\u{ff0d}b",
        "profile-set a\tb",
        "profile-set a\nrules-remove 1",
        "profile-set a\u{0}b",
        "profile-set a\u{7f}b",
        "keep-running on no\u{a0}tes",
    ] {
        assert!(classify(text).is_err(), "{text:?}");
    }
    assert!(classify(&"r".repeat(MAX_REQUEST + 1)).is_err());
    let name = "n".repeat(24);
    assert_eq!(classify(&format!("profile-set {name}")), Ok(Kind::Change));
    let value = "v".repeat(crate::settings::MAX_VALUE);
    assert_eq!(
        classify(&format!("appearance-set theme {value}")),
        Ok(Kind::Change)
    );
    assert!(classify(&format!("appearance-set theme {value}v")).is_err());
}

#[test]
fn a_request_that_is_not_one_traps_and_queues_nothing() {
    for text in ["format-disk", "Rules", "a  b", "rules_remove", "rules x"] {
        let mut app = app_sending(&[text]);
        let fault = app.health().expect_err(text);
        assert!(fault.detail.contains("send"), "{}", fault.detail);
        assert!(app.take_settings().is_empty(), "{text}");
    }
}

#[test]
fn too_many_requests_before_comphost_takes_them_is_a_fault() {
    let mut app = app_sending(&["about"]);
    for _ in 0..MAX_QUEUED {
        app.health().unwrap();
    }
    let fault = app.health().expect_err("one more traps");
    assert!(fault.detail.contains("waiting"), "{}", fault.detail);
    assert_eq!(app.take_settings().len(), MAX_QUEUED);
    // The start's eight reads leave room for more.
    assert!(READS.len() * 2 <= MAX_QUEUED);
}

#[test]
fn a_reply_reaches_the_app_as_a_result_with_its_ticket() {
    let mut app = load_core_granted(&guest(), LIMITS, None, granted()).unwrap();
    app.settings_reply(7, Ok(b"1\trule one".to_vec())).unwrap();
    // The ticket in the top byte, the result's case next (0 is ok), then the
    // payload's length.
    assert_eq!(app.self_test().unwrap(), 7 << 24 | 10);
    app.settings_reply(9, Err(String::from("not from a key or a click")))
        .unwrap();
    assert_eq!(app.self_test().unwrap(), 9 << 24 | 1 << 16 | 25);
}

#[test]
fn a_big_answer_and_a_long_or_odd_error_are_cut_down_to_what_the_app_can_take() {
    let mut app = load_core_granted(&guest(), LIMITS, None, granted()).unwrap();
    app.settings_reply(1, Ok(vec![b'x'; MAX_REPLY])).unwrap();
    assert_eq!(app.self_test().unwrap(), 1 << 24 | MAX_REPLY as u32);
    app.settings_reply(2, Ok(vec![b'x'; MAX_REPLY + 1]))
        .unwrap();
    // An error: case 1, the text "the answer is too large" (23 bytes).
    assert_eq!(app.self_test().unwrap(), 2 << 24 | 1 << 16 | 23);
    let long = "e".repeat(MAX_ERROR * 4);
    app.settings_reply(3, Err(long)).unwrap();
    assert_eq!(
        app.self_test().unwrap(),
        3 << 24 | 1 << 16 | MAX_ERROR as u32
    );
    // Control and bidirectional characters become one `?` each.
    app.settings_reply(4, Err("a\u{1b}[31m\u{202e}b".to_string()))
        .unwrap();
    assert_eq!(app.self_test().unwrap(), 4 << 24 | 1 << 16 | 8);
}

#[test]
fn an_app_that_exports_no_replies_is_told_nothing() {
    let bytes = wat::parse_str(
        r#"(module
  (import "kai:settings/settings@0.1.0" "send" (func $send (param i32 i32) (result i32)))
  (memory (export "memory") 1)
  (func (export "other")))"#,
    )
    .unwrap();
    let mut app = load_core_granted(&bytes, LIMITS, None, granted()).unwrap();
    assert_eq!(app.settings_reply(1, Ok(Vec::new())), Ok(()));
}

/// An app can be given an `Instance` too, through the trait the host uses.
#[test]
fn the_instance_trait_reaches_the_same_queue() {
    let mut app = app_sending(&["profiles"]);
    assert!(Instance::health(&mut app).is_ok());
    assert_eq!(requests(&app.take_settings()), ["profiles"]);
}
