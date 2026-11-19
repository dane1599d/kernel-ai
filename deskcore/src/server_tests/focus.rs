//! Which new window takes the keyboard: a client's window takes it only when
//! nothing has it or one of the client's own has it; any other opens without
//! it, however still the person is. What the person asked for is given the
//! focus by the shell's `ui focus`.

use super::*;
use crate::desk::Input;

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
fn a_background_clients_window_never_takes_the_keys_whatever_the_person_does() {
    let mut desk = Desk::new(1280, 800);
    let (_notes, one) = first_window(&mut desk, 1);
    // Nothing had the focus: the first window takes it.
    assert_eq!(desk.focus(), Focus::Window(one));
    // Typing, however long ago or lent to the Terminal (where the desk sees no
    // key at all), makes no difference: a second client's window does not take it.
    for at in [10_000u64, 10_001, 500_000] {
        desk.set_now(at);
        desk.input(Input::Key {
            usage: 0x04,
            down: true,
        });
        desk.input(Input::Key {
            usage: 0x04,
            down: false,
        });
        desk.set_now(at + 1_000_000);
        let (_other, two) = first_window(&mut desk, 2);
        assert!(desk.window(two).is_some());
        assert_eq!(desk.focus(), Focus::Window(one), "the keys stayed put");
    }
}

#[test]
fn the_focus_holders_own_second_window_and_a_bare_desk_take_the_keys() {
    let mut desk = Desk::new(1280, 800);
    assert!(desk.may_activate(7), "nothing has the focus");
    let (_first, one) = first_window(&mut desk, 1);
    assert!(desk.may_activate(1));
    assert!(!desk.may_activate(2));
    assert_eq!(desk.focus(), Focus::Window(one));
    // After a click on the empty desk nothing has it again, and a window may.
    let mut effects = Vec::new();
    desk.set_focus(Focus::Nothing, &mut effects);
    assert!(desk.may_activate(2));
}

#[test]
fn the_pane_keeps_the_keys_from_any_window_opened_meanwhile() {
    let mut desk = Desk::new(1280, 800);
    let mut effects = Vec::new();
    desk.set_focus(Focus::Panel, &mut effects);
    let (_client, window) = first_window(&mut desk, 9);
    assert!(desk.window(window).is_some());
    assert_eq!(desk.focus(), Focus::Panel);
    // What the person asked for takes the focus by the shell's `ui focus`.
    let mut effects = Vec::new();
    desk.set_focus(Focus::Window(window), &mut effects);
    assert_eq!(desk.focus(), Focus::Window(window));
}
