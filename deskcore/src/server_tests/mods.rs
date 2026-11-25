//! The modifiers held, told to every client whatever had the focus when they
//! changed, so none stays held in an app after Ctrl+Space or a card.

use super::*;
use crate::desk::Input;

fn key(desk: &mut Desk, usage: u16, down: bool) -> Vec<Effect> {
    desk.input(Input::Key { usage, down })
}

fn mods_of(effects: &[Effect]) -> Vec<u32> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::Modifiers(m) => Some(*m),
            _ => None,
        })
        .collect()
}

#[test]
fn ctrl_space_leaves_no_ctrl_held_for_the_app_it_came_from() {
    let mut desk = Desk::new(1280, 800);
    let mut notes = Client::new(1, "notes");
    notes.handle(&mut desk, &setup(), true).unwrap();
    notes.pool_mapped(2);
    notes.handle(&mut desk, &show(), false).unwrap();
    // Ctrl down in Notes, Space to the pane, Ctrl up: the release never reaches
    // a window, but it is told to every client.
    let down = key(&mut desk, 0xE0, true);
    assert_eq!(mods_of(&down), [2]);
    notes.apply(&down);
    let space = key(&mut desk, 0x2C, true);
    notes.apply(&space);
    let up = key(&mut desk, 0xE0, false);
    assert_eq!(mods_of(&up), [0]);
    notes.apply(&up);
    // Back in Notes, a letter carries no Ctrl and types its text.
    let mut effects = Vec::new();
    let window = desk.windows()[0].id;
    desk.set_focus(crate::desk::Focus::Window(window), &mut effects);
    notes.apply(&effects);
    let letter = key(&mut desk, 0x04, true);
    notes.apply(&letter);
    let posts = notes.take_posts();
    let last = posts.last().expect("the letter");
    let (x, len) = match &last.1 {
        surfabi::header::Event::Input { x, len, .. } => (*x, *len),
        other => panic!("{other:?}"),
    };
    assert_eq!((x, len), (0, 1), "no modifier held, and its text");
}

#[test]
fn a_modifier_let_go_under_a_card_is_let_go() {
    let mut desk = Desk::new(1280, 800);
    key(&mut desk, 0xE0, true);
    desk.show_ask(3, 1);
    // The card swallows the key, not the desk's count of it.
    let up = key(&mut desk, 0xE0, false);
    assert_eq!(mods_of(&up), [0]);
    assert!(!desk.holds_for_test(2));
}

#[test]
fn only_a_change_is_told() {
    let mut desk = Desk::new(1280, 800);
    assert_eq!(mods_of(&key(&mut desk, 0xE1, true)), [1]);
    // The other Shift and a repeat change nothing.
    assert!(mods_of(&key(&mut desk, 0xE5, true)).is_empty());
    assert!(mods_of(&key(&mut desk, 0xE1, true)).is_empty());
    assert!(mods_of(&key(&mut desk, 0xE1, false)).is_empty());
    assert_eq!(mods_of(&key(&mut desk, 0xE5, false)), [0]);
}
