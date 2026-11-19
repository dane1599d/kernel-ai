//! Which new window takes the keyboard: a client's first window takes it only
//! when nothing has it, one of the client's own has it, or the person has not
//! acted for [`crate::desk::ACTIVATE_IDLE_MS`]; otherwise it opens without it.

use super::*;
use crate::desk::{ACTIVATE_IDLE_MS, Input};

/// A client's first window, committed: its window id.
fn first_window(desk: &mut Desk, pid: u64) -> (Client, crate::desk::WindowId) {
    let mut client = Client::new(pid, "app");
    client.handle(desk, &setup(), true).unwrap();
    client.pool_mapped(2);
    let needs = client.handle(desk, &show(), false).unwrap();
    let [Need::Show { window, .. }] = needs[..] else {
        panic!("{needs:?}");
    };
    (client, window)
}

#[test]
fn a_background_clients_window_does_not_take_the_keys_while_the_person_acts() {
    let mut desk = Desk::new(1280, 800);
    let (_notes, one) = first_window(&mut desk, 1);
    // Nothing had the focus: the first window takes it.
    assert_eq!(desk.focus(), Focus::Window(one));
    // The person types in it, then a second client opens a window at once.
    desk.set_now(10_000);
    desk.input(Input::Key {
        usage: 0x04,
        down: true,
    });
    desk.input(Input::Key {
        usage: 0x04,
        down: false,
    });
    desk.set_now(10_000 + ACTIVATE_IDLE_MS - 1);
    let (_other, two) = first_window(&mut desk, 2);
    assert!(desk.window(two).is_some());
    assert_eq!(desk.focus(), Focus::Window(one), "the keys stayed put");
    // The person has left the keyboard alone for the idle time: a third window
    // takes it, as a window opened by what they started does.
    desk.set_now(10_000 + ACTIVATE_IDLE_MS + 5_000);
    let (_third, three) = first_window(&mut desk, 3);
    assert_eq!(desk.focus(), Focus::Window(three));
}

#[test]
fn the_focus_holders_own_second_window_and_a_bare_desk_take_the_keys() {
    let mut desk = Desk::new(1280, 800);
    assert!(desk.may_activate(7), "nothing has the focus");
    let (_first, one) = first_window(&mut desk, 1);
    desk.set_now(50);
    desk.input(Input::Motion(crate::geom::Point { x: 5, y: 5 }));
    // Its own client's window takes it even while the person acts; another's does not.
    assert!(desk.may_activate(1));
    assert!(!desk.may_activate(2));
    assert_eq!(desk.focus(), Focus::Window(one));
}

#[test]
fn the_pane_keeps_the_keys_from_a_window_opened_while_the_person_types_in_it() {
    let mut desk = Desk::new(1280, 800);
    let mut effects = Vec::new();
    desk.set_focus(Focus::Panel, &mut effects);
    desk.set_now(1_000);
    desk.input(Input::Key {
        usage: 0x04,
        down: true,
    });
    desk.set_now(1_500);
    let (_client, window) = first_window(&mut desk, 9);
    assert!(desk.window(window).is_some());
    assert_eq!(desk.focus(), Focus::Panel);
}
