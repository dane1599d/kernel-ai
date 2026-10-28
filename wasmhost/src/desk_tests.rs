//! comphost's requests served by `deskcore`'s own connection code, as `desktop`
//! serves them, and `desktop`'s events read back.

use crate::desk::{self, Desk, Told};
use alloc::vec::Vec;
use deskcore::desk::{Desk as Screen, Effect, Focus};
use deskcore::server::{Client, Need};
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
    // Another desktop's greeting is a restart.
    let again = Client::new(8, "notes-window").take_events(8192);
    assert_eq!(d.told(&again), [Told::Restarted]);
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
