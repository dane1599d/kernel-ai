//! A held send's card and rules (ADR 0002 question 3): it always asks, its
//! card says how long it waits for `undo`, and no "Always allow" is offered
//! or honoured for it. On a test row: the real send row comes with its
//! transport.

use super::*;
use agentabi::APP_SUSPEND;
use agentabi::Inverse;

static NO_UNDO: Action = Action {
    inverse: Inverse::None("sent"),
    ..APP_SUSPEND
};
static SEND: Action = Action {
    inverse: Inverse::Hold { ms: 5_000 },
    ..APP_SUSPEND
};

fn args() -> Arguments {
    Arguments {
        values: alloc::vec![("app", String::from("notes"))],
        rhai: Vec::new(),
    }
}

#[test]
fn a_held_send_is_offered_no_standing_rule() {
    // The same row with no undo is offered one, for its action or target.
    assert!(offer(&NO_UNDO, &args(), Why::NoUndo).is_some());
    assert!(offer(&SEND, &args(), Why::NoUndo).is_none());
    assert!(offer(&SEND, &args(), Why::Ungrounded).is_none());
}

#[test]
fn a_held_sends_card_says_it_waits_for_undo_not_that_it_has_none() {
    assert_eq!(
        hold_reason(&SEND).as_deref(),
        Some("after you allow, it waits 5 s so you can undo")
    );
    assert_eq!(hold_reason(&NO_UNDO), None);
    assert!(held(&SEND) && !held(&NO_UNDO));
    assert_eq!(reason_of(Why::NoUndo), "it has no undo");
}
