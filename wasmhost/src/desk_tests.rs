//! comphost's requests served by `deskcore`'s own connection code, as `desktop`
//! serves them, and `desktop`'s events read back.

use crate::desk::{self, Desk, Told};
use alloc::string::String;
use alloc::vec::Vec;
use deskcore::desk::{Desk as Screen, Effect, Focus};
use deskcore::server::{Client, Need};
use surfabi::proto::{Interface, Request};
use uikit::layout::FixedAdvance;
use uikit::widget::{Layout, ROOT, Widget};
use uikit::{Frame, Window};

fn frame(title: &str, w: u16, h: u16, label: &str) -> Frame {
    let mut win = Window::new(title, w, h);
    let button = Widget::Button {
        text: label.into(),
        action: None,
    };
    win.put(1, ROOT, Layout::FIT, button).unwrap();
    win.commit(&FixedAdvance)
}

/// Makes `calls` on `client`, the first with the pool's handle when `pool`.
fn serve(client: &mut Client, screen: &mut Screen, calls: &[Vec<u8>], pool: bool) -> Vec<Need> {
    let mut needs = Vec::new();
    for (n, call) in calls.iter().enumerate() {
        assert!(call.len() <= kabi::MAX_MESSAGE);
        let got = client.handle(screen, call, pool && n == 0).unwrap();
        needs.extend(got);
    }
    needs
}

#[test]
fn a_window_opens_shows_new_pictures_and_closes() {
    let mut screen = Screen::new(1280, 800);
    let mut client = Client::new(7, "notes-window");
    let mut d = Desk::new();
    // The registry's globals come first: a greeting, not a restart.
    let hello = client.take_events(8192);
    assert_eq!(d.told(&hello), []);
    let Err(open) = d.frame(1, &frame("Notes", 320, 200, "Go")) else {
        panic!("a first frame opens a window");
    };
    assert_eq!(open.pool_bytes, 4096 + 2 * 320 * 200 * 4);
    assert_eq!(open.target.offset, 4096);
    let needs = serve(&mut client, &mut screen, &open.calls, true);
    let [Need::MapPool { pool, size }, Need::Show { window, buffer }] = needs[..] else {
        panic!("{needs:?}");
    };
    assert_eq!(
        (size, buffer.offset, buffer.width),
        (open.pool_bytes, 4096, 320)
    );
    client.pool_mapped(pool);
    assert_eq!(screen.window(window).unwrap().title, "Notes");
    // The next frame goes to the other buffer, with its damage.
    let Ok(show) = d.frame(1, &frame("Notes", 320, 200, "Go on")) else {
        panic!("an open window takes new pictures");
    };
    assert_eq!(show.target.offset, 4096 + 320 * 200 * 4);
    assert!(!show.damage.is_empty());
    let needs = serve(&mut client, &mut screen, &show.calls, false);
    assert!(
        matches!(needs[..], [Need::Show { buffer, .. }] if buffer.offset == show.target.offset)
    );
    // A click that focuses it reaches the app as focus (app nap's use).
    client.take_events(8192);
    client.apply(&[Effect::Focus(Focus::Window(window))]);
    assert_eq!(d.told(&client.take_events(8192)), [Told::Focus(1)]);
    // The user asks to close it; the app's close ends it on the desk.
    client.apply(&[Effect::Close { window }]);
    assert_eq!(d.told(&client.take_events(8192)), [Told::Close(1)]);
    let needs = serve(&mut client, &mut screen, &[d.closed(1)], false);
    assert_eq!(needs, [Need::Closed { window }]);
    assert!(d.windows().is_empty() && screen.window(window).is_none());
    // The desktop's theme reaches the app by name, and is no new greeting.
    client.theme("light");
    assert_eq!(
        d.told(&client.take_events(8192)),
        [Told::Theme(String::from("light"))]
    );
    // Another desktop's greeting is a restart.
    let again = Client::new(8, "notes-window").take_events(8192);
    assert_eq!(d.told(&again), [Told::Restarted]);
}

#[test]
fn a_late_modifiers_message_never_changes_the_keys_already_in_the_ring() {
    let mut screen = Screen::new(1280, 800);
    let mut client = Client::new(7, "editor");
    let mut d = Desk::new();
    d.told(&client.take_events(8192));
    let Err(open) = d.frame(1, &frame("Editor", 320, 200, "Go")) else {
        panic!("a first frame opens a window");
    };
    let needs = serve(&mut client, &mut screen, &open.calls, true);
    let [Need::MapPool { pool, .. }, Need::Show { window, .. }] = needs[..] else {
        panic!("{needs:?}");
    };
    client.pool_mapped(pool);
    client.take_events(8192);
    // Ctrl-B, as `desktop` hands it over: each key into the ring, and Ctrl's
    // press and release as `seat.modifiers` messages too.
    let key = |usage, down| Effect::Key {
        window,
        usage,
        down,
    };
    client.apply(&[
        key(0xE0, true),
        key(0x05, true),
        key(0x05, false),
        key(0xE0, false),
    ]);
    let ring: Vec<_> = client.take_posts().into_iter().map(|(_, e)| e).collect();
    assert_eq!(ring.len(), 4);
    // The app reads Ctrl's press from the ring, then is busy while the rest
    // come, and reads both messages, the release last, before the ring's B.
    d.seat_state.input(&ring[0]);
    d.told(&client.take_events(8192));
    let b = d.seat_state.input(&ring[1]);
    let Some(uikit::input::Input::Key { mods, down, .. }) = b else {
        panic!("{b:?}");
    };
    assert!(down && mods.0 & uikit::input::Mods::CTRL != 0, "{mods:?}");
}

#[test]
fn a_new_size_is_a_new_window_and_a_restart_forgets_them() {
    let mut d = Desk::new();
    assert!(d.frame(1, &frame("a", 100, 100, "x")).is_err());
    assert!(d.frame(1, &frame("a", 100, 100, "y")).is_ok());
    let Err(bigger) = d.frame(1, &frame("a", 200, 100, "y")) else {
        panic!("a new size reopens");
    };
    let mut client = Client::new(7, "notes-window");
    client.take_events(8192);
    // The reopen first ends the old window, then makes the new one.
    assert!(
        client
            .handle(&mut Screen::new(1280, 800), &bigger.calls[0], true)
            .is_err(),
        "the old objects are not this client's"
    );
    d.reset();
    assert!(d.windows().is_empty());
    assert!(d.frame(1, &frame("a", 100, 100, "x")).is_err());
}

#[test]
fn requests_split_into_calls_at_message_boundaries() {
    let mut d = Desk::new();
    // A big tree: many buttons.
    let mut win = Window::new("t", 600, 400);
    for n in 1..200 {
        let b = Widget::Button {
            text: alloc::format!("button {n}"),
            action: None,
        };
        win.put(n, ROOT, Layout::FIT, b).unwrap();
    }
    let Err(open) = d.frame(1, &win.commit(&FixedAdvance)) else {
        panic!()
    };
    let all: Vec<u8> = open.calls.concat();
    assert!(open.calls.iter().all(|c| c.len() <= kabi::MAX_MESSAGE));
    assert_eq!(desk::calls(&all).concat(), all);
}

/// A window whose one button runs `action`, holding unsaved changes or not.
fn bound(win: &mut Window, action: Option<&str>, unsaved: bool) -> Frame {
    let button = Widget::Button {
        text: "Save".into(),
        action: action.map(Into::into),
    };
    win.put(1, ROOT, Layout::FIT, button).unwrap();
    win.set_unsaved(unsaved);
    win.commit(&FixedAdvance)
}

#[test]
fn desktop_learns_a_buttons_action_and_the_windows_unsaved_changes() {
    let mut screen = Screen::new(1280, 800);
    let mut client = Client::new(7, "notes-window");
    let mut d = Desk::new();
    client.take_events(8192);
    let mut win = Window::new("Notes", 320, 200);
    let Err(open) = d.frame(1, &bound(&mut win, Some("notes_save"), false)) else {
        panic!("a first frame opens a window");
    };
    let needs = serve(&mut client, &mut screen, &open.calls, true);
    let [Need::MapPool { pool, .. }, Need::Show { window, .. }] = needs[..] else {
        panic!("{needs:?}");
    };
    client.pool_mapped(pool);
    let button = crate::surface::wire_id(1);
    assert_eq!(client.bound_action(window, button), Some("notes_save"));
    assert!(!client.unsaved(window));
    // An edit: the app says it holds changes; the binding is not sent again.
    let Ok(show) = d.frame(1, &bound(&mut win, Some("notes_save"), true)) else {
        panic!("an open window takes new pictures");
    };
    serve(&mut client, &mut screen, &show.calls, false);
    assert!(client.unsaved(window));
    assert_eq!(client.bound_action(window, button), Some("notes_save"));
    // Saved, and the button runs nothing declared any more.
    let Ok(show) = d.frame(1, &bound(&mut win, None, false)) else {
        panic!("an open window takes new pictures");
    };
    serve(&mut client, &mut screen, &show.calls, false);
    assert!(!client.unsaved(window));
    assert_eq!(client.bound_action(window, button), None);
}

#[test]
fn desktop_learns_a_private_window_before_its_contents() {
    let mut screen = Screen::new(1280, 800);
    let mut client = Client::new(7, "settings-window");
    let mut d = Desk::new();
    client.take_events(8192);
    let mut win = Window::new("Settings", 320, 200);
    win.set_agent_private(true);
    let first = bound(&mut win, None, false);
    let Err(open) = d.frame(1, &first) else {
        panic!("a first frame opens a window");
    };
    // The flag comes before the tree: no message holds the contents first.
    let all: Vec<u8> = open.calls.concat();
    let tree = crate::surface::tree_update(&first).encode();
    let flag = position(&all, |m| m == &Request::SetAgentPrivate(true)).expect("the flag");
    let tree = position(&all, |m| m == &Request::TreeUpdate(&tree)).expect("the tree");
    assert!(flag < tree, "flag is message {flag}, tree {tree}");
    let needs = serve(&mut client, &mut screen, &open.calls, true);
    let [Need::MapPool { pool, .. }, Need::Show { window, .. }] = needs[..] else {
        panic!("{needs:?}");
    };
    client.pool_mapped(pool);
    assert!(client.agent_private(window));
    // Shared again: told once, not on every frame.
    win.set_agent_private(false);
    let Ok(show) = d.frame(1, &bound(&mut win, None, false)) else {
        panic!("an open window takes new pictures");
    };
    serve(&mut client, &mut screen, &show.calls, false);
    assert!(!client.agent_private(window));
    let Ok(show) = d.frame(1, &bound(&mut win, None, false)) else {
        panic!("an open window takes new pictures");
    };
    let shared = |m: &Request<'_>| m == &Request::SetAgentPrivate(false);
    assert_eq!(position(&show.calls.concat(), shared), None);
}

/// The index of the first message in `calls` that `is` picks, as `desktop`
/// would read it on whichever object it went to.
fn position(calls: &[u8], is: impl Fn(&Request<'_>) -> bool) -> Option<usize> {
    let mut rest = calls;
    let mut index = 0;
    while !rest.is_empty() {
        let (header, args, after) = surfabi::wire::split(rest).ok()?;
        let picked = INTERFACES.iter().any(|&interface| {
            Request::decode(interface, header.opcode, args).is_ok_and(|m| {
                m.encode(header.object)
                    .is_ok_and(|bytes| bytes.len() == header.size as usize)
                    && is(&m)
            })
        });
        if picked {
            return Some(index);
        }
        (rest, index) = (after, index + 1);
    }
    None
}

const INTERFACES: [Interface; 2] = [Interface::Toplevel, Interface::A11y];
