//! The trusted card's input: what answers it, and the keys, presses and
//! pointers that never can.

use super::*;

fn centre(r: Rect) -> (i32, i32) {
    (r.x + r.w / 2, r.y + r.h / 2)
}

/// Kai Desk PR 3: a card that comes up while the panel (and the Apps sheet
/// on it) has the keys takes the focus, which `desktop` reads as the sheet's
/// close, and from then on no key reaches the panel or the sheet.
#[test]
fn a_card_closes_the_sheet_and_takes_its_keys() {
    let (mut desk, _) = desk_with(1);
    key(&mut desk, usage::LEFT_CTRL, true);
    assert_eq!(
        key(&mut desk, usage::SPACE, true),
        [Effect::Focus(Focus::Panel)]
    );
    key(&mut desk, usage::SPACE, false);
    key(&mut desk, usage::LEFT_CTRL, false);
    // The sheet's keys: Down, Enter, Esc.
    let sheet_keys = [0x51, 0x28, 0x29];
    assert!(
        key(&mut desk, sheet_keys[0], true).contains(&Effect::PanelKey {
            usage: sheet_keys[0],
            down: true
        })
    );
    key(&mut desk, sheet_keys[0], false);
    assert_eq!(desk.show_ask(9, 3), [Effect::Focus(Focus::Ask(9))]);
    desk.card_drawn(0);
    for code in sheet_keys {
        for down in [true, false] {
            let effects = key(&mut desk, code, down);
            assert!(
                !effects
                    .iter()
                    .any(|e| matches!(e, Effect::PanelKey { .. } | Effect::Focus(_))),
                "{code:#x}: {effects:?}"
            );
        }
    }
    assert_eq!(desk.focus(), Focus::Ask(9));
}

#[test]
fn an_ask_card_takes_only_y_n_or_a_click_and_drops_keys_held_before_it() {
    let (mut desk, ids) = desk_with(2);
    // `a` is held down when the card appears.
    key(&mut desk, 0x04, true);
    assert_eq!(desk.show_ask(7, 3), [Effect::Focus(Focus::Ask(7))]);
    desk.card_drawn(0);
    assert!(desk.take_damage().iter().any(|d| {
        let card = crate::trusted::card(W, H, &desk.ask().unwrap());
        d.intersect(&card) == Some(card)
    }));
    // Its release, Enter, Esc and other keys go nowhere; no window hears them.
    for code in [0x04, 0x28, 0x29, 0x05] {
        assert!(key(&mut desk, code, true).is_empty() || code == 0x04);
        assert!(key(&mut desk, code, false).is_empty());
    }
    // A click on a window neither raises it nor reaches it.
    let lower = desk.window(ids[0]).unwrap().content();
    assert!(click(&mut desk, (lower.x + 3, lower.y + 3)).is_empty());
    assert_eq!(desk.windows().last().unwrap().id, ids[1]);
    // Shortcuts cannot take focus from it.
    key(&mut desk, usage::LEFT_CTRL, true);
    assert!(key(&mut desk, usage::SPACE, true).is_empty());
    key(&mut desk, usage::SPACE, false);
    key(&mut desk, usage::LEFT_CTRL, false);
    assert_eq!(desk.focus(), Focus::Ask(7));
    // Once settled, a first y arms the card, and a second answers yes; focus
    // goes back to the window.
    desk.set_now(SETTLED);
    assert!(key(&mut desk, usage::Y, true).is_empty());
    key(&mut desk, usage::Y, false);
    assert!(desk.ask().unwrap().armed);
    assert_eq!(
        key(&mut desk, usage::Y, true),
        [
            Effect::Answer { id: 7, yes: true },
            Effect::Focus(Focus::Window(ids[1]))
        ]
    );
    assert_eq!(desk.ask(), None);
}

#[test]
fn a_y_held_before_the_card_cannot_answer_it_and_buttons_can() {
    let (mut desk, _) = desk_with(1);
    key(&mut desk, usage::Y, true);
    desk.show_ask(9, 1);
    desk.card_drawn(0);
    // The y still held repeats or comes up: neither answers.
    assert!(key(&mut desk, usage::Y, true).is_empty());
    assert!(key(&mut desk, usage::Y, false).is_empty());
    let card = crate::trusted::card(W, H, &desk.ask().unwrap());
    let (_, no) = crate::trusted::buttons(card);
    desk.set_now(SETTLED);
    let effects = click(&mut desk, centre(no));
    assert!(effects.contains(&Effect::Answer { id: 9, yes: false }));
    // A card that times out goes without an answer.
    desk.show_ask(10, 1);
    assert_eq!(desk.hide_ask(11), []);
    assert!(!desk.hide_ask(10).is_empty());
    assert_eq!(desk.ask(), None);
}

#[test]
fn a_key_whose_release_went_to_the_terminal_can_answer_the_next_card() {
    let (mut desk, _) = desk_with(1);
    desk.show_ask(1, 1);
    desk.card_drawn(0);
    desk.set_now(SETTLED);
    key(&mut desk, usage::Y, true);
    key(&mut desk, usage::Y, false);
    assert!(key(&mut desk, usage::Y, true).contains(&Effect::Answer { id: 1, yes: true }));
    // Focus is back on the terminal window: desktop lends the keys, and the
    // y's release goes to the console, never to the desk.
    desk.keys_lent();
    desk.show_ask(2, 1);
    desk.card_drawn(SETTLED);
    desk.set_now(2 * SETTLED);
    key(&mut desk, usage::Y, true);
    key(&mut desk, usage::Y, false);
    assert!(key(&mut desk, usage::Y, true).contains(&Effect::Answer { id: 2, yes: true }));
}

/// Past a card's settle time.
const SETTLED: u64 = termcore::menu::SETTLE_MS;

/// Presses and lets go of `code` at `at_ms`, by the event's own time.
fn press_at(desk: &mut Desk, at_ms: u64, code: u16) -> Vec<Effect> {
    desk.set_now(at_ms);
    let effects = key(desk, code, true);
    key(desk, code, false);
    effects
}

/// #864's reviews: the card takes nothing until it has been drawn and the
/// keys quiet for the settle time; `y` takes two steps; a repeat is not one.
#[test]
fn a_card_settles_before_it_answers_and_y_takes_two_steps() {
    let (mut desk, _) = desk_with(1);
    desk.set_now(1_000);
    desk.show_ask(5, 1);
    // Not drawn yet: nothing counts, however long after.
    assert!(press_at(&mut desk, 5_000, usage::Y).is_empty());
    desk.card_drawn(5_000);
    // Within the quiet time y, n and a click on Run do nothing, and each
    // starts it again.
    for code in [usage::Y, usage::N] {
        assert!(press_at(&mut desk, 5_000 + SETTLED - 1, code).is_empty());
    }
    let card = crate::trusted::card(W, H, &desk.ask().unwrap());
    let (yes, _) = crate::trusted::buttons(card);
    desk.set_now(5_000 + SETTLED);
    assert!(
        click(&mut desk, centre(yes)).is_empty(),
        "the n restarted the quiet"
    );
    assert!(!desk.ask().unwrap().armed);
    // Quiet at last: the first y arms it, with the card redrawn; the second
    // runs it.
    let quiet = 5_000 + 2 * SETTLED;
    desk.set_now(quiet);
    desk.take_damage();
    assert!(key(&mut desk, usage::Y, true).is_empty());
    assert!(!desk.take_damage().is_empty(), "the armed card redraws");
    // Its key repeat is not a second step: only a fresh key-down is.
    assert!(key(&mut desk, usage::Y, true).is_empty());
    assert!(desk.ask().is_some());
    key(&mut desk, usage::Y, false);
    assert!(key(&mut desk, usage::Y, true).contains(&Effect::Answer { id: 5, yes: true }));
    assert_eq!(desk.ask(), None);
}

/// Typing straight through the card's arrival ("copy my notes", a key every
/// 120 ms) never answers it: each key starts the quiet time again.
#[test]
fn typing_straight_through_a_card_never_answers_it() {
    let (mut desk, _) = desk_with(1);
    desk.show_ask(3, 1);
    desk.card_drawn(0);
    // c o p y _ m y _ n o t e s, then on and on.
    let codes = [
        0x06, 0x12, 0x13, 0x1C, 0x2C, 0x10, 0x1C, 0x2C, 0x11, 0x12, 0x17, 0x08, 0x16,
    ];
    for (n, code) in codes.iter().cycle().take(80).enumerate() {
        let effects = press_at(&mut desk, 100 + 120 * n as u64, *code);
        assert!(effects.is_empty(), "key {n} answered: {effects:?}");
    }
    assert!(desk.ask().is_some_and(|a| !a.armed));
}

/// `y`, another key, `y`: the other key disarms the card, and the second `y`
/// only arms it again once the keys are quiet.
#[test]
fn y_then_another_key_then_y_does_not_run_it() {
    let (mut desk, _) = desk_with(1);
    desk.show_ask(4, 1);
    desk.card_drawn(0);
    assert!(press_at(&mut desk, SETTLED, usage::Y).is_empty());
    assert!(desk.ask().unwrap().armed);
    assert!(press_at(&mut desk, SETTLED + 100, 0x04).is_empty());
    assert!(!desk.ask().unwrap().armed);
    assert!(press_at(&mut desk, SETTLED + 200, usage::Y).is_empty());
    assert!(!desk.ask().unwrap().armed, "too soon after the other key");
    // Quiet again: arm, then run.
    assert!(press_at(&mut desk, 2 * SETTLED + 300, usage::Y).is_empty());
    assert!(
        press_at(&mut desk, 2 * SETTLED + 400, usage::Y)
            .contains(&Effect::Answer { id: 4, yes: true })
    );
}

/// `y`, Super, `y` with no lease (ADR 0005 §7): Super reaches the desk only
/// as a press `desktop` reports, which disarms the card as any key but `y`
/// does, so the second `y` does not run it.
#[test]
fn y_then_super_then_y_does_not_run_it() {
    use crate::desk::usage::LEFT_GUI;
    use crate::lease::{Lease, on_key};
    let (mut desk, _) = desk_with(1);
    let mut lease = Lease::new();
    desk.show_ask(4, 1);
    desk.card_drawn(0);
    assert!(press_at(&mut desk, SETTLED, usage::Y).is_empty());
    assert!(desk.ask().unwrap().armed);
    // desktop's order: no set_now of its own before the press, which comes
    // long after the `y`; the quiet time runs from the press.
    let super_at = SETTLED + 5_000;
    let step = on_key(&mut lease, LEFT_GUI, 1);
    assert!(step.super_press && step.keys.is_empty());
    desk.super_pressed(super_at);
    assert!(!desk.ask().unwrap().armed, "Super did not disarm the card");
    assert!(press_at(&mut desk, super_at + 20, usage::Y).is_empty());
    assert!(press_at(&mut desk, super_at + 40, usage::Y).is_empty());
    assert!(on_key(&mut lease, LEFT_GUI, 0).keys.is_empty());
    assert!(desk.ask().is_some_and(|a| !a.armed));
    assert!(desk.start().is_none());
}

#[test]
fn a_click_on_run_answers_at_once_once_quiet() {
    let (mut desk, _) = desk_with(1);
    desk.show_ask(6, 1);
    desk.card_drawn(0);
    let card = crate::trusted::card(W, H, &desk.ask().unwrap());
    let (yes, _) = crate::trusted::buttons(card);
    desk.set_now(SETTLED - 1);
    assert!(click(&mut desk, centre(yes)).is_empty());
    desk.set_now(2 * SETTLED);
    assert!(click(&mut desk, centre(yes)).contains(&Effect::Answer { id: 6, yes: true }));
}

#[test]
fn a_key_or_click_in_the_agents_window_takes_over() {
    let (mut desk, ids) = desk_with(2);
    desk.set_agent(Some(ids[1]));
    desk.take_damage();
    // A key while another window has focus is not taking over.
    let other = desk.window(ids[0]).unwrap().content();
    click(&mut desk, (other.x + 2, other.y + 2));
    assert_eq!(desk.agent(), Some(ids[1]));
    // A click on the agent's window is, where the other window does not cover it.
    let agent = desk.window(ids[1]).unwrap().content();
    let effects = click(&mut desk, (agent.right() - 3, agent.bottom() - 3));
    assert!(effects.contains(&Effect::TookOver { window: ids[1] }));
    assert_eq!(desk.agent(), None);
    let outline = crate::trusted::outline(desk.window(ids[1]).unwrap().frame);
    assert!(
        desk.take_damage()
            .iter()
            .any(|d| d.intersect(&outline) == Some(outline))
    );
    // A key in the focused agent window is too.
    desk.set_agent(Some(ids[1]));
    assert!(key(&mut desk, 0x04, true).contains(&Effect::TookOver { window: ids[1] }));
}

/// Backlog item 9: Caps Lock is announced to every client and goes to no
/// window; with Num Lock off the keypad's 4 is Left, and its 5 nothing.
#[test]
fn the_locks_toggle_and_the_keypad_follows_num_lock() {
    let (mut desk, ids) = desk_with(1);
    let window = ids[0];
    assert_eq!(key(&mut desk, 0x39, true), [Effect::Locks { caps: true }]);
    assert!(key(&mut desk, 0x39, false).is_empty());
    assert!(desk.caps());
    // Num Lock on (as at start): keypad 4 is itself, a digit.
    assert_eq!(
        key(&mut desk, 0x5C, true),
        [Effect::Key {
            window,
            usage: 0x5C,
            down: true
        }]
    );
    key(&mut desk, 0x5C, false);
    // Off: it is Left.
    assert!(key(&mut desk, 0x53, true).is_empty());
    key(&mut desk, 0x53, false);
    assert_eq!(
        key(&mut desk, 0x5C, true),
        [Effect::Key {
            window,
            usage: 0x50,
            down: true
        }]
    );
    assert!(key(&mut desk, 0x5D, true).is_empty(), "keypad 5 is nothing");
}

/// #864's third review: a press queued while `desktop` was busy, stamped
/// before the card was drawn, never opens it early. Drawn at 550, a press
/// stamped 100 and one at 600: the card is still quiet until 1,000.
#[test]
fn a_press_stamped_before_the_card_was_drawn_never_opens_it() {
    let (mut desk, _) = desk_with(1);
    desk.show_ask(8, 1);
    desk.card_drawn(550);
    assert!(press_at(&mut desk, 100, usage::Y).is_empty());
    assert!(press_at(&mut desk, 600, usage::Y).is_empty());
    assert!(!desk.ask().unwrap().armed, "600 is inside the quiet time");
    // 1,000 is a settle after the press at 600: open now.
    assert!(press_at(&mut desk, 600 + SETTLED, usage::Y).is_empty());
    assert!(desk.ask().unwrap().armed);
}

/// A pointer parked where Run appears clicks nothing: it must be brought
/// there after the card was drawn. A right or middle press restarts the
/// quiet time like any other.
#[test]
fn a_parked_pointer_cannot_click_run_and_other_buttons_restart_the_quiet() {
    let (mut desk, _) = desk_with(1);
    desk.show_ask(9, 1);
    let card = crate::trusted::card(W, H, &desk.ask().unwrap());
    let (yes, _) = crate::trusted::buttons(card);
    // The pointer is on Run before the card is drawn.
    desk.set_now(0);
    desk.input(at(centre(yes)));
    desk.card_drawn(10);
    desk.set_now(10 + 3 * SETTLED);
    for down in [true, false] {
        let press = Input::Button {
            button: Button::Left,
            down,
        };
        assert!(desk.input(press).is_empty(), "a parked click ran the card");
    }
    // A right press restarts the quiet: a click just after it is too soon.
    desk.set_now(20 + 4 * SETTLED);
    desk.input(Input::Button {
        button: Button::Right,
        down: true,
    });
    desk.set_now(20 + 4 * SETTLED + 1);
    assert!(click(&mut desk, centre(yes)).is_empty());
    // Quiet again, and brought onto Run: it runs.
    desk.set_now(20 + 6 * SETTLED);
    let away = (centre(yes).0 - 200, centre(yes).1);
    desk.input(at(away));
    assert!(click(&mut desk, centre(yes)).contains(&Effect::Answer { id: 9, yes: true }));
}

/// #864's fourth review: a USB tablet sends the pointer's place with every
/// report, the button's own included. A pointer parked on Run before the
/// card was drawn, reported again and again, is no entry: its click runs
/// nothing. Moved off and back on after the draw, it is.
#[test]
fn a_tablets_repeated_reports_are_no_entry_onto_run() {
    let (mut desk, _) = desk_with(1);
    desk.show_ask(11, 1);
    let card = crate::trusted::card(W, H, &desk.ask().unwrap());
    let (yes, _) = crate::trusted::buttons(card);
    desk.set_now(0);
    desk.input(at(centre(yes)));
    desk.card_drawn(10);
    // Reports at the same place after the draw, then the press with its own.
    for t in [100, 3 * SETTLED, 3 * SETTLED + 1] {
        desk.set_now(t);
        desk.input(at(centre(yes)));
    }
    let press = Input::Button {
        button: Button::Left,
        down: true,
    };
    assert!(desk.input(press).is_empty(), "a parked click ran the card");
    // Off Run and back on, after a quiet time: now it runs.
    desk.set_now(5 * SETTLED);
    desk.input(at((centre(yes).0 - 200, centre(yes).1)));
    desk.set_now(5 * SETTLED + 1);
    assert!(click(&mut desk, centre(yes)).contains(&Effect::Answer { id: 11, yes: true }));
}

/// The fourth review's form of the queued-press case: the card shown at
/// 1,000 on desktop's clock, drawn at 550 by the presses' clock, presses
/// stamped 100 and 600: still quiet.
#[test]
fn presses_at_100_and_600_with_the_card_drawn_at_550_do_not_open_it() {
    let (mut desk, _) = desk_with(1);
    desk.set_now(1_000);
    desk.show_ask(12, 1);
    desk.card_drawn(550);
    assert!(press_at(&mut desk, 100, usage::Y).is_empty());
    assert!(press_at(&mut desk, 600, usage::Y).is_empty());
    assert!(!desk.ask().unwrap().armed);
    let card = crate::trusted::card(W, H, &desk.ask().unwrap());
    let (yes, _) = crate::trusted::buttons(card);
    desk.set_now(601);
    assert!(click(&mut desk, centre(yes)).is_empty());
}
