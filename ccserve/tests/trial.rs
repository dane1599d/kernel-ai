//! `trial <component>` through the service (#583's piece 3b): an app `cc
//! build` wrote into the `Shm` runs on stub imports, and each way it can fail
//! is named for the repair loop. Hostile requests are refused.

mod common;

use ccserve::build::shm;
use common::Cc;
use promptcore::wire::parse_reply;

/// An app that keeps a schema-1 state and queues a notice when it starts.
const APP: &str = "#include <app.h>
#include <stdlib.h>
// SPDX-License-Identifier: MIT
uint32_t exports_kai_live_lifecycle_schema(void) { return 1; }
bool exports_kai_live_lifecycle_init(exports_kai_live_lifecycle_start_error_t *err) {
    return true;
}
bool exports_kai_live_lifecycle_migrate(app_list_u8_t *state, uint32_t from_schema,
                                        exports_kai_live_lifecycle_start_error_t *err) { return true; }
bool exports_kai_live_lifecycle_save(uint32_t to_schema, app_list_u8_t *ret) {
    uint8_t *state = malloc(3);
    if (!state) {
        return false;
    }
    state[0] = 0x82;
    state[1] = 0x01;
    state[2] = 0x60;
    ret->ptr = state;
    ret->len = 3;
    return true;
}
uint32_t exports_kai_live_lifecycle_health(void) { return 0; }
uint32_t exports_kai_live_lifecycle_self_test(void) { return 0; }
void exports_kai_app_events_on_event(exports_kai_app_events_event_t *e) {
    kai_app_effects_effect_t notice;
    notice.tag = KAI_APP_EFFECTS_EFFECT_NOTIFY;
    app_string_set(&notice.val.notify, \"started\");
    kai_app_effects_request(&notice);
    exports_kai_app_events_event_free(e);
}
";

/// `message` with `shm`: the reply's records as strings.
fn send(cc: &mut Cc, message: &str, shm: Option<&mut [u8]>) -> Vec<String> {
    let reply = cc.service.handle_with(message.as_bytes(), shm, &cc.clock);
    parse_reply(&reply)
        .unwrap()
        .iter()
        .map(common::show)
        .collect()
}

/// An `Shm` that holds `source` built as `name` 0.0.1, as `build` leaves it.
fn built(cc: &mut Cc, name: &str, source: &str) -> Vec<u8> {
    let mut shm = vec![0u8; shm::BYTES];
    let bundle = livecore::artifact::bundle(&[("main.c", source.as_bytes())]);
    shm::put(&mut shm, &bundle).unwrap();
    let reply = send(cc, &format!("build {name} 0.0.1"), Some(&mut shm));
    assert!(reply[0].starts_with("text "), "{reply:?}");
    shm
}

/// `source`'s trial as `name`: the one record of the reply.
fn trial(source: &str) -> String {
    let mut cc = Cc::open();
    let mut shm = built(&mut cc, "timer", source);
    let reply = send(&mut cc, "trial timer", Some(&mut shm));
    let [record] = &reply[..] else {
        panic!("{reply:?}")
    };
    record.clone()
}

#[test]
fn a_good_app_passes_and_what_it_queues_is_not_run() {
    assert_eq!(
        trial(APP),
        "text trial ok: init, started; 0 window(s), 0 frame(s); 1 effect(s) queued, none run"
    );
}

#[test]
fn a_state_without_its_schema_fails_after_init() {
    let empty = APP.replace(
        "ret->ptr = state;\n    ret->len = 3;",
        "free(state);\n    ret->ptr = 0;\n    ret->len = 0;",
    );
    assert_eq!(
        trial(&empty),
        "err 0 after init, its state says schema None, not 1"
    );
}

#[test]
fn an_init_that_fails_or_never_ends_is_named() {
    let refused = APP.replace(
        "start_error_t *err) {\n    return true;",
        "start_error_t *err) {\n    return false;",
    );
    assert_eq!(trial(&refused), "err 0 init failed: it returned 1");
    let endless = APP.replace(
        "start_error_t *err) {\n    return true;",
        "start_error_t *err) {\n    for (volatile int i = 0; ; i++) {}\n    return true;",
    );
    assert_eq!(
        trial(&endless),
        "err 0 init failed (fuel): all fuel consumed by WebAssembly"
    );
}

#[test]
fn a_trap_in_an_event_is_named_with_its_step() {
    let trapping = APP.replace(
        "    kai_app_effects_effect_t notice;",
        "    volatile int zero = 0;\n    volatile int boom = 1 / zero;\n    kai_app_effects_effect_t notice;",
    );
    assert_eq!(
        trial(&trapping),
        "err 0 started failed (trap): integer divide by zero"
    );
}

/// A host fault in a call is the program's to repair, not `internal`: here a
/// notice whose text is not UTF-8.
#[test]
fn a_bad_string_the_app_passes_is_its_own_fault() {
    let bad = APP.replace(
        "app_string_set(&notice.val.notify, \"started\");",
        "notice.val.notify.ptr = (uint8_t *)\"\\xff\\xfe\";\n    notice.val.notify.len = 2;",
    );
    let got = trial(&bad);
    assert!(got.starts_with("err 0 started failed (host): "), "{got}");
}

#[test]
fn a_trial_takes_only_the_apps_own_artifact() {
    let mut cc = Cc::open();
    assert_eq!(
        send(&mut cc, "trial timer", None),
        ["err 0 internal: trial brings the artifact in an Shm"]
    );
    // Built as another app.
    let mut shm = built(&mut cc, "clock", APP);
    let got = send(&mut cc, "trial timer", Some(&mut shm));
    assert_eq!(
        got,
        ["err 0 internal: the artifact is app:clock/clock@0.0.1, not timer's"]
    );
    // Not an artifact, a length past the end, and one over the largest.
    let mut junk = vec![0u8; shm::BYTES];
    shm::put(&mut junk, b"\0asm not really").unwrap();
    let got = send(&mut cc, "trial timer", Some(&mut junk));
    assert!(
        got[0].starts_with("err 0 internal: the trial's artifact does not load"),
        "{got:?}"
    );
    junk[..8].copy_from_slice(&u64::MAX.to_le_bytes());
    let got = send(&mut cc, "trial timer", Some(&mut junk));
    assert!(
        got[0].starts_with("err 0 internal: the trial's artifact: "),
        "{got:?}"
    );
    let mut short = vec![0u8; 4];
    let got = send(&mut cc, "trial timer", Some(&mut short));
    assert!(
        got[0].starts_with("err 0 internal: the trial's artifact: "),
        "{got:?}"
    );
    // A bad name never reaches the trial.
    let got = send(&mut cc, "trial ../timer", Some(&mut shm));
    assert_eq!(got, ["err 0 not a name"]);
}

#[test]
fn a_trial_stops_at_ctrl_c_and_past_a_line_budget() {
    let mut cc = Cc::open();
    let mut shm = built(&mut cc, "timer", APP);
    cc.clock.interrupted.set(true);
    let got = send(&mut cc, "trial timer", Some(&mut shm));
    assert_eq!(got, ["err 0 internal: the trial was stopped before init"]);
    cc.clock.interrupted.set(false);
    // Each clock read moves on by more than a line's budget.
    cc.clock.step_ns.set(3 * ccserve::LINE_NS);
    let got = send(&mut cc, "trial timer", Some(&mut shm));
    assert!(got[0].starts_with("err 0 init took "), "{got:?}");
    // The session is untouched.
    cc.clock.step_ns.set(0);
    assert_eq!(cc.value("1 + 1"), "i64 2");
}
