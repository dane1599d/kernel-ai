//! `desktop`, the compositor (ADR 0005 §1, P3 task 6): it claims the screen,
//! serves `kai-surface 1` to its clients, and draws their windows with
//! `deskcore`'s rules, painted by `deskcore::paint` on `gfx2d` in Ember
//! Workbench's tokens and type.
//!
//! - A client calls the `desktop` service with a run of `surfabi` messages; the
//!   reply carries the events waiting for it. `pool.create` brings its `Shm`,
//!   mapped here read-only.
//! - Input comes from the kernel's queue (INPUT_READ); Ctrl+Alt+F12 takes the
//!   screen away (a `Screen` event) and gives it back.
//! - Each frame repaints only the desk's damage, grown by what a window's
//!   shadow reaches, into a frame of its own, then copies those rectangles to
//!   the screen and flushes them.
//! - Window events (minimize, restore, focus, close) and idle minutes go to the
//!   kernel's event box for the shell's standing instructions (P3 task 11),
//!   found by `deskcore::watch`.
//! - The top bar's model state and memory are read once a second; a screen
//!   without a hardware cursor (GOP) gets one drawn in the frame.
//! - Everything is painted in the theme the [`Appearance`] resolves to (P4
//!   task 8): checked once per frame, and on a change the whole screen and the
//!   hardware cursor are painted again. It starts from the plan line's
//!   arguments (`theme=light`, `legible=on`, ...), each an `appearance_set`
//!   setting.
//! - While the terminal window (`term`'s) has focus, the keys go to the
//!   kernel's terminal instead: a lease (INPUT_TO_CONSOLE) renewed every
//!   [`LEASE_RENEW_NS`], and ended when the focus moves (P3 task 8). The
//!   kernel, not `desktop`, decides which client is the terminal: it lends the
//!   keys only for the window of a process holding TERM.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use deskcore::askcard::Card;
use deskcore::compose::{Pixels, Source};
use deskcore::desk::{Button, Desk, Effect, Focus, Input, WindowId};
use deskcore::effects;
use deskcore::geom::{Point, Rect};
use deskcore::paint::{self, Bar, Painter, Scene};
use deskcore::paint_panel;
use deskcore::panel::Panel;
use deskcore::panel_wire;
use deskcore::server::{Buffer, Client, Need};
use deskcore::watch::Watch;
use gfx2d::{Canvas, Fonts, Image};
use kabi::display::{DisplayInfo, FlushRect};
use kabi::handle::{Handle, RecvInfo};
use kabi::input::{InputEvent, Kind};
use look::Palette;
use look::appearance::{self, Appearance, Shipped};
use surfabi::Format;
use user::{println, sys};

user::entry!(main);

/// The fonts ADR 0005 §8 names, built in until P2's files carry them.
const FONTS: Fonts = Fonts {
    sans: include_bytes!("../../../assets/fonts/InterVariable.ttf"),
    mono: include_bytes!("../../../assets/fonts/JetBrainsMono-Variable.ttf"),
};

/// How often the top bar's model state and memory are read.
const BAR_EVERY_NS: u64 = 1_000_000_000;

/// Waits for input or a client, in turn, so neither starves.
const INPUT_WAIT_MS: u64 = 2;
const CLIENT_WAIT_MS: u64 = 2;
/// How often the lease is renewed while that window has focus: well inside the
/// kernel's one second.
const LEASE_RENEW_NS: u64 = 250_000_000;

struct Pool {
    pid: u64,
    pool: u32,
    address: u64,
    size: u32,
    /// Mapped writable, so its header page's event ring takes the client's
    /// input (ADR 0005 §3); a client that gave read-only access gets none.
    writable: bool,
}

struct Screen {
    info: DisplayInfo,
    address: u64,
}

struct State {
    desk: Desk,
    clients: Vec<Client>,
    pools: Vec<Pool>,
    shown: Vec<(WindowId, u64, Buffer)>,
    frame: Image,
    width: i32,
    height: i32,
    painter: Painter,
    bar: Bar,
    bar_at: u64,
    /// Damage of desktop's own: the top bar's stats, the drawn cursor.
    extra: Vec<Rect>,
    /// The screen has no hardware cursor, so the frame shows one.
    draws_cursor: bool,
    /// The first frame is on the screen.
    drawn: bool,
    /// The AI panel's line and the state the shell reports.
    panel: Panel,
    /// Apps' effects waiting for the shell, and what came of them.
    effects: effects::Queue,
    /// When the panel's "thinking 4s" was last redrawn.
    ticked_ms: u64,
    /// The ask card up now, as the kernel posted it.
    card: Option<Card>,
    /// When the ask box was last read.
    asked_ms: u64,
    /// Frames as last logged, to report moves and closes once.
    known: Vec<(WindowId, Rect)>,
    focus: Focus,
    /// What standing instructions hear about: window events and idle.
    watch: Watch,
    /// When the keys were last lent to the kernel's terminal, while they are.
    leased_at: Option<u64>,
    /// The focused window, and whether the kernel lent the keys for it.
    asked: Option<(WindowId, bool)>,
    /// What the user chose, and what the painter paints in: kept with the
    /// painter across a lost screen, and the default (what `Painter::new`
    /// paints) at first, so the first frame does no theme work for it.
    appearance: Appearance,
    painted_in: Appearance,
}

fn main() -> i32 {
    let Some(mut painter) = Painter::new(FONTS) else {
        println!("desktop: the built-in fonts do not parse");
        return 1;
    };
    let appearance = starting_appearance();
    let mut painted_in = Appearance::default();
    loop {
        let Some(screen) = claim() else {
            if !wait_for_screen() {
                return 1;
            }
            continue;
        };
        let width = screen.info.width as i32;
        let height = screen.info.height as i32;
        let mut state = State {
            desk: Desk::new(width, height),
            clients: Vec::new(),
            pools: Vec::new(),
            shown: Vec::new(),
            frame: Image::new(width, height),
            width,
            height,
            painter,
            bar: Bar::default(),
            bar_at: 0,
            extra: Vec::new(),
            draws_cursor: screen.info.cursor == 0,
            drawn: false,
            panel: Panel::new(),
            effects: effects::Queue::default(),
            ticked_ms: 0,
            card: None,
            asked_ms: 0,
            known: Vec::new(),
            focus: Focus::Nothing,
            watch: Watch::new(now_ms()),
            leased_at: None,
            asked: None,
            appearance: appearance.clone(),
            painted_in,
        };
        println!("desktop: claimed {width}x{height}");
        let result = run(&mut state, &screen);
        painter = state.painter;
        painted_in = state.painted_in;
        let Some(back) = result else {
            return 1;
        };
        let _ = sys::shm_unmap(screen.address);
        println!("desktop: lost the screen to the console");
        if !back && !wait_for_screen() {
            return 1;
        }
        println!("desktop: the screen is back");
    }
}

fn claim() -> Option<Screen> {
    let mut info = DisplayInfo::default();
    let shm = sys::display_claim(&mut info).ok()?;
    let address = sys::shm_map(shm, true).ok()?;
    if info.cursor != 0 {
        let _ = sys::display_cursor(&arrow(&look::DARK), (0, 0));
        let _ = sys::display_cursor_move(info.width / 2, info.height / 2);
    }
    Some(Screen { info, address })
}

/// Serves until the screen is taken: `Some(back)` then, `back` when the same
/// read already gave it back (both Ctrl+Alt+F12 presses at once); `None` on
/// an error.
fn run(state: &mut State, screen: &Screen) -> Option<bool> {
    let mut events = [InputEvent {
        time_ns: 0,
        kind: Kind::Screen,
        code: 0,
        value: 0,
    }; 32];
    let mut request = vec![0u8; kabi::MAX_MESSAGE];
    loop {
        let Ok(n) = sys::input_read(&mut events, INPUT_WAIT_MS) else {
            return None;
        };
        for (at, event) in events[..n].iter().enumerate() {
            if event.kind == Kind::Screen && event.value == 0 {
                let back = events[at + 1..n]
                    .iter()
                    .any(|e| e.kind == Kind::Screen && e.value == 1);
                return Some(back);
            }
            input(state, screen, event);
        }
        let mut info = RecvInfo::default();
        if let Ok(len) = sys::recv_h(&mut request, CLIENT_WAIT_MS, &mut info) {
            serve(state, &request[..len.min(request.len())], &info);
        }
        report(state);
        post_events(state);
        refresh_bar(state);
        tick_panel(state);
        read_asks(state);
        lease(state);
        theme(state, screen);
        draw(state, screen);
    }
}

/// Lends the keys to the kernel's terminal while the terminal window has
/// focus, renewing the lease; ends it once the focus moves. The kernel says
/// which window that is: it refuses the lease for any other client.
fn lease(state: &mut State) {
    let focused = match state.desk.focus() {
        Focus::Window(id) => state
            .desk
            .window(id)
            .filter(|w| !w.minimized)
            .map(|w| (id, u64::from(w.client))),
        _ => None,
    };
    let now = sys::time_ns();
    let Some((id, pid)) = focused else {
        if state.leased_at.take().is_some() {
            let _ = sys::input_to_console(false, 0);
            println!("desktop: the keys are the desktop's again");
        }
        state.asked = None;
        return;
    };
    let fresh = state
        .leased_at
        .is_some_and(|at| now.saturating_sub(at) < LEASE_RENEW_NS);
    match state.asked {
        // Not the terminal: asked once for this focus.
        Some((asked, false)) if asked == id => return,
        Some((asked, true)) if asked == id && fresh => return,
        _ => {}
    }
    let lent = sys::input_to_console(true, pid).is_ok();
    match (lent, state.leased_at.is_some()) {
        (true, false) => println!("desktop: the terminal window has the keys"),
        (false, true) => println!("desktop: the keys are the desktop's again"),
        _ => {}
    }
    state.asked = Some((id, lent));
    state.leased_at = lent.then_some(now);
}

/// One kernel input event, in logical px at scale 1.
fn input(state: &mut State, screen: &Screen, event: &InputEvent) {
    let p = state.desk.pointer();
    let input = match event.kind {
        Kind::Rel => Some(Input::Motion(match event.code {
            0 => Point {
                x: p.x + event.value,
                y: p.y,
            },
            _ => Point {
                x: p.x,
                y: p.y + event.value,
            },
        })),
        Kind::Abs => {
            let scale = |v: i32, max: i32| (i64::from(v) * i64::from(max) / 32768) as i32;
            Some(Input::Motion(match event.code {
                0 => Point {
                    x: scale(event.value, state.width),
                    y: p.y,
                },
                _ => Point {
                    x: p.x,
                    y: scale(event.value, state.height),
                },
            }))
        }
        Kind::Button => {
            let button = match event.code {
                1 => Button::Left,
                2 => Button::Right,
                _ => Button::Middle,
            };
            Some(Input::Button {
                button,
                down: event.value != 0,
            })
        }
        Kind::Key => {
            println!("desktop: key {:#x} {}", event.code, event.value);
            Some(Input::Key {
                usage: event.code,
                down: event.value != 0,
            })
        }
        _ => None,
    };
    let Some(input) = input else {
        return;
    };
    state.watch.input(now_ms());
    let before = state.desk.pointer();
    let effects = state.desk.input(input);
    let at = state.desk.pointer();
    if at != before {
        if state.draws_cursor {
            state.extra.push(paint::cursor_box(before));
            state.extra.push(paint::cursor_box(at));
        } else if screen.info.cursor != 0 {
            let _ = sys::display_cursor_move(at.x as u32, at.y as u32);
        }
    }
    deliver(state, &effects);
}

/// Hands the desk's effects to the clients they concern: messages, and input
/// posted to their pools' event rings.
fn deliver(state: &mut State, effects: &[Effect]) {
    for client in &mut state.clients {
        client.apply(effects);
        for (pool, event) in client.take_posts() {
            post(&state.pools, client.pid, pool, &event);
        }
    }
    let now = now_ms();
    for effect in effects {
        if let Effect::Answer { id, yes } = *effect {
            answer_ask(state, id, yes);
            continue;
        }
        let changed = match *effect {
            Effect::PanelKey { usage, down } => {
                let before = state.panel.waiting();
                let changed = state.panel.key(usage, down, now);
                if state.panel.waiting() > before {
                    // Its length only: a panel line may hold anything typed.
                    let len = match state.panel.newest() {
                        Some(deskcore::panel::Request::Line(text)) => text.chars().count(),
                        _ => 0,
                    };
                    println!("desktop: the panel queued a request ({len} characters)");
                    wake_shell();
                }
                changed
            }
            Effect::Focus(focus) => state.panel.set_focus(focus == Focus::Panel),
            _ => false,
        };
        if changed {
            state.extra.push(state.desk.panel());
        }
    }
}

/// How often the ask box is read: often enough that the kernel sees the
/// trusted layer alive, and a posted ask shows within a frame or two.
const ASK_EVERY_MS: u64 = 100;

/// Reads the ask box: a new ask shows as the card; one the kernel withdrew
/// goes away.
fn read_asks(state: &mut State) {
    let now = now_ms();
    if now.saturating_sub(state.asked_ms) < ASK_EVERY_MS {
        return;
    }
    state.asked_ms = now;
    let mut buf = [0u8; 2048];
    let Ok(len) = sys::ask_read(&mut buf) else {
        return;
    };
    let text = core::str::from_utf8(&buf[..len.min(buf.len())]).unwrap_or("");
    let mut parts = text.splitn(3, '\n');
    let posted = match (parts.next(), parts.next(), parts.next()) {
        (Some(id), Some(call), Some(question)) => id.parse().ok().map(|id| (id, call, question)),
        _ => None,
    };
    match (posted, state.card.as_ref()) {
        (Some((id, _, _)), Some(card)) if card.id == id => {}
        (Some((id, call, question)), _) => {
            let card = Card {
                id,
                call: call.into(),
                reason: question.into(),
                shown_ms: now,
            };
            let lines = state.painter.card_lines(state.width, &card);
            if let Some(old) = state.card.take() {
                let effects = state.desk.hide_ask(old.id);
                deliver(state, &effects);
            }
            println!("desktop: ask {id} shows on the card");
            let effects = state.desk.show_ask(id, lines);
            state.card = Some(card);
            deliver(state, &effects);
        }
        (None, Some(card)) => {
            // Withdrawn by the kernel: it asks on the console now.
            let id = card.id;
            state.card = None;
            let effects = state.desk.hide_ask(id);
            deliver(state, &effects);
        }
        (None, None) => {}
    }
}

/// The card's answer goes to the kernel with the digest of the call it showed.
fn answer_ask(state: &mut State, id: u64, yes: bool) {
    let Some(card) = state.card.take().filter(|c| c.id == id) else {
        return;
    };
    let digest = termcore::once::digest_call(id, &card.call);
    let word = if yes { "yes" } else { "no" };
    match sys::ask_answer(id, yes, &digest) {
        Ok(_) => println!("desktop: ask {id} answered {word}"),
        Err(error) => println!("desktop: the kernel refused ask {id}'s answer ({error:?})"),
    }
}

/// Posts `event` to the ring in the header page of `pid`'s pool `pool`; a full
/// ring drops it and counts the drop there.
fn post(pools: &[Pool], pid: u64, pool: u32, event: &surfabi::header::Event) {
    let Some(p) = pools
        .iter()
        .find(|p| p.pid == pid && p.pool == pool && p.writable)
    else {
        return;
    };
    if (p.size as usize) < surfabi::header::PAGE {
        return;
    }
    // SAFETY: the pool is mapped read-write at `address` for `size` bytes, at
    // least a page; its first page is the header the client set up, and only
    // its ring's slots and head are written.
    let page = unsafe { &mut *(p.address as *mut [u8; surfabi::header::PAGE]) };
    surfabi::header::post(page, event);
}

fn now_ms() -> u64 {
    sys::time_ns() / 1_000_000
}

/// Redraws the panel's state once a second while it counts ("thinking 4s").
fn tick_panel(state: &mut State) {
    let now = now_ms();
    if paint_panel::ticks(&state.panel) && now.saturating_sub(state.ticked_ms) >= 1_000 {
        state.ticked_ms = now;
        state
            .extra
            .push(paint_panel::state_area(state.desk.panel()));
    }
}

fn serve(state: &mut State, bytes: &[u8], info: &RecvInfo) {
    let pid = info.caller_pid;
    if let Some(reply) = panel_request(state, bytes, pid) {
        let _ = sys::reply_h(reply.as_bytes(), &[]);
        return;
    }
    if let Some(reply) = effects_request(state, bytes, pid) {
        let _ = sys::reply_h(reply.as_bytes(), &[]);
        return;
    }
    let new = !state.clients.iter().any(|c| c.pid == pid);
    if new {
        let label = name_of(pid);
        state.clients.push(Client::new(pid, &label));
    }
    let Some(at) = state.clients.iter().position(|c| c.pid == pid) else {
        return;
    };
    let handle = (info.handle_count > 0).then(|| Handle(info.handles[0]));
    let result = state.clients[at].handle(&mut state.desk, bytes, handle.is_some());
    match result {
        Ok(needs) => {
            for need in needs {
                apply_need(state, at, pid, need, handle);
            }
            let reply = state.clients[at].take_events(kabi::MAX_MESSAGE);
            let _ = sys::reply_h(&reply, &[]);
        }
        // A client new to this desktop that names objects it never made was
        // talking to the desktop before a restart: it gets the greeting (the
        // registry's globals), not an empty reply, so it opens its windows
        // again (`term` committed to its old surface each time the console
        // changed, and never heard the greeting).
        Err(surfabi::wire::Error::UnknownObject) if new => {
            println!("desktop: pid {pid} spoke to an earlier desktop; greeting it");
            // Windows the message opened before it failed go with the old
            // connection, not left on the desk.
            let effects = state.clients[at].close_all(&mut state.desk);
            deliver(state, &effects);
            let label = state.clients[at].label.clone();
            state.clients[at] = Client::new(pid, &label);
            let reply = state.clients[at].take_events(kabi::MAX_MESSAGE);
            let _ = sys::reply_h(&reply, &[]);
        }
        Err(error) => {
            println!("desktop: pid {pid} sent a bad message ({error:?}); closing its windows");
            let mut client = state.clients.remove(at);
            let effects = client.close_all(&mut state.desk);
            deliver(state, &effects);
            state.shown.retain(|(_, owner, _)| *owner != pid);
            for pool in state.pools.iter().filter(|p| p.pid == pid) {
                let _ = sys::shm_unmap(pool.address);
            }
            state.pools.retain(|p| p.pid != pid);
            let _ = sys::reply_h(b"", &[]);
        }
    }
}

/// The shell's `panel take` and `panel state` (P3 task 7,
/// `deskcore::panel_wire`): its answer, or `None` for a window client's
/// message. Only the shell may take the panel's lines or set its state.
fn panel_request(state: &mut State, bytes: &[u8], pid: u64) -> Option<String> {
    let text = core::str::from_utf8(bytes).ok()?;
    if !text.starts_with("panel ") {
        return None;
    }
    if name_of(pid) != "shell" {
        return Some(String::from("err only the shell talks to the panel"));
    }
    if text == panel_wire::TAKE {
        let request = state.panel.take();
        if request.is_some() {
            println!("desktop: the shell took the panel's request");
            // From the Enter to the take: the wake's latency (under 100 ms).
            if let deskcore::panel::State::Thinking { since_ms } = *state.panel.state() {
                let ms = now_ms().saturating_sub(since_ms);
                let within = if ms < 100 { "within" } else { "over" };
                println!(
                    "desktop: the panel's request was taken {ms} ms after Enter ({within} 100 ms)"
                );
            }
        }
        return Some(panel_wire::encode_take(request.as_ref()));
    }
    let Some(shown) = panel_wire::parse_state(text) else {
        return Some(String::from("err not a panel request"));
    };
    let word = match &shown {
        deskcore::panel::State::Idle => "idle",
        deskcore::panel::State::Thinking { .. } => "thinking",
        deskcore::panel::State::Acting { .. } => "acting",
        deskcore::panel::State::Done { .. } => "done",
        deskcore::panel::State::Refused { .. } => "refused",
    };
    if state.panel.set_state(shown) {
        // Its kind only: the line is the shell's result, which may name a file.
        println!("desktop: the panel shows {word}");
        state.extra.push(state.desk.panel());
    }
    Some(String::from("ok"))
}

/// An app's `effects put` and `effects results`, and the shell's `effects
/// take` and `effects done` (P4, `deskcore::effects`): its answer, or `None`
/// for a window client's message. The app is the kernel's name for the
/// caller, never anything it sent.
fn effects_request(state: &mut State, bytes: &[u8], pid: u64) -> Option<String> {
    let text = core::str::from_utf8(bytes).ok()?;
    if !text.starts_with("effects ") {
        return None;
    }
    let caller = name_of(pid);
    let shell = caller == "shell";
    if let Some(lines) = text.strip_prefix(effects::PUT) {
        if shell || caller.starts_with("pid ") {
            return Some(String::from("err only an app queues effects"));
        }
        let refused = state.effects.put(&caller, lines);
        println!("desktop: {caller} queued effects ({refused} refused)");
        wake_shell();
        return Some(String::from("ok"));
    }
    if text == effects::RESULTS {
        return Some(effects::encode_results(&state.effects.results(&caller)));
    }
    if !shell {
        return Some(String::from("err only the shell runs effects"));
    }
    if text == effects::TAKE {
        let taken = state.effects.take(kabi::MAX_MESSAGE - 64);
        if let Some((app, _)) = &taken {
            println!("desktop: the shell took {app}'s effects");
        }
        return Some(effects::encode_take(taken.as_ref()));
    }
    let Some((app, results)) = effects::parse_done(text) else {
        return Some(String::from("err not an effects request"));
    };
    for done in results {
        state.effects.finish(app, done);
    }
    Some(String::from("ok"))
}

fn apply_need(state: &mut State, at: usize, pid: u64, need: Need, handle: Option<Handle>) {
    match need {
        Need::MapPool { pool, size } => {
            let Some(handle) = handle else { return };
            // Writable when the client allowed it, for its input ring; read-only
            // otherwise, as before.
            let mapped = sys::shm_map(handle, true)
                .map(|address| (address, true))
                .or_else(|_| sys::shm_map(handle, false).map(|address| (address, false)));
            match mapped {
                Ok((address, writable)) => {
                    state.pools.push(Pool {
                        pid,
                        pool,
                        address,
                        size,
                        writable,
                    });
                    state.clients[at].pool_mapped(pool);
                }
                Err(error) => println!("desktop: cannot map pid {pid}'s pool: {error:?}"),
            }
        }
        Need::Show { window, buffer } => {
            state.shown.retain(|(w, ..)| *w != window);
            state.shown.push((window, pid, buffer));
        }
        Need::Closed { window } => state.shown.retain(|(w, ..)| *w != window),
    }
}

/// The name a process runs under, which labels its windows and names its app
/// in standing instructions' events. From the metrics' process list: LIST is
/// the supervisor's alone.
fn name_of(pid: u64) -> alloc::string::String {
    let named = sys::metrics().ok().and_then(|m| {
        let listed = &m.processes[..(m.process_count as usize).min(m.processes.len())];
        listed
            .iter()
            .find(|p| p.pid == pid)
            .map(|p| alloc::string::String::from(p.name()))
    });
    named.unwrap_or_else(|| alloc::format!("pid {pid}"))
}

/// Logs windows that opened, moved or closed, and focus changes, once each.
fn report(state: &mut State) {
    for window in state.desk.windows() {
        match state.known.iter_mut().find(|(id, _)| *id == window.id) {
            None => {
                let f = window.frame;
                println!(
                    "desktop: window {} \"{}\" opened at {},{} size {}x{}",
                    window.id.0, window.title, f.x, f.y, f.w, f.h
                );
                state.known.push((window.id, f));
            }
            Some((_, frame)) if *frame != window.frame => {
                let f = window.frame;
                println!(
                    "desktop: window {} moved to {},{} size {}x{}",
                    window.id.0, f.x, f.y, f.w, f.h
                );
                *frame = f;
            }
            Some(_) => {}
        }
    }
    let desk = &state.desk;
    state.known.retain(|(id, _)| {
        let open = desk.window(*id).is_some();
        if !open {
            println!("desktop: window {} closed", id.0);
        }
        open
    });
    let focus = state.desk.focus();
    if focus != state.focus {
        match focus {
            Focus::Window(id) => println!("desktop: window {} has focus", id.0),
            other => println!("desktop: focus {other:?}"),
        }
        state.focus = focus;
    }
}

/// Posts `work` to the kernel's event box, which wakes the shell to take what
/// was queued for it now, not at its next look.
fn wake_shell() {
    if let Err(error) = sys::event_post("work") {
        println!("desktop: cannot wake the shell: {error:?}");
    }
}

/// Posts the window events and idle minutes since last round to the kernel's
/// event box, for the shell's standing instructions.
fn post_events(state: &mut State) {
    let mut events = state
        .watch
        .windows(state.desk.windows(), state.desk.focus());
    events.extend(state.watch.idle(now_ms()));
    for event in events {
        let mut line = alloc::string::String::new();
        let _ = event.text(&mut line);
        match sys::event_post(&line) {
            Ok(_) => println!("desktop: event {line}"),
            Err(error) => println!("desktop: cannot post {line}: {error:?}"),
        }
    }
}

/// Reads the model state and memory once a second; a change damages the top
/// bar's stats.
fn refresh_bar(state: &mut State) {
    let now = sys::time_ns();
    if state.bar_at != 0 && now.saturating_sub(state.bar_at) < BAR_EVERY_NS {
        return;
    }
    state.bar_at = now.max(1);
    let Ok(m) = sys::metrics() else {
        return;
    };
    let listed = &m.processes[..(m.process_count as usize).min(m.processes.len())];
    let mib = |frames: u64| frames * 4096 / (1024 * 1024);
    let bar = Bar {
        model: listed.iter().any(|p| p.name() == "llm"),
        mem_used_mib: mib(m.total_frames - m.free_frames),
        mem_total_mib: mib(m.total_frames),
    };
    if bar != state.bar {
        state.bar = bar;
        state.extra.push(paint::stats_area(state.width));
        // The idle panel shows the model's dot too.
        state
            .extra
            .push(paint_panel::state_area(state.desk.panel()));
    }
}

/// Repaints the damage into the frame, copies it to the screen and flushes it.
fn draw(state: &mut State, screen: &Screen) {
    let mut damage = state.desk.take_damage();
    damage.append(&mut state.extra);
    if damage.is_empty() {
        return;
    }
    let bounds = Rect::new(0, 0, state.width, state.height);
    let damage: Vec<Rect> = damage
        .iter()
        .filter_map(|r| paint::reach(*r).intersect(&bounds))
        .collect();
    let mut frame = state.frame.canvas();
    let (shown, pools) = (&state.shown, &state.pools);
    let scene = Scene {
        desk: &state.desk,
        bar: &state.bar,
        panel: &state.panel,
        ask: state.card.as_ref(),
        now_ms: now_ms(),
    };
    let mut content = |id: WindowId, layer: &mut Canvas<'_>, at: Rect| {
        let Some((_, pid, buffer)) = shown.iter().find(|(w, ..)| *w == id) else {
            return false;
        };
        let Some(source) = source(pools, *pid, buffer) else {
            return false;
        };
        let mut pixels = Pixels {
            width: layer.width as i32,
            height: layer.height as i32,
            stride: layer.stride_px as usize,
            data: layer.pixels,
        };
        pixels.fill(at, at, (0xFF << 24) | look::DARK.window);
        pixels.draw(&source, (at.x, at.y), at);
        true
    };
    for &clip in &damage {
        state.painter.paint(&mut frame, clip, &scene, &mut content);
        if state.draws_cursor {
            state.painter.cursor(&mut frame, clip, state.desk.pointer());
        }
    }
    copy_to_screen(&state.frame.px, state.width, screen, &damage);
    let rects: Vec<FlushRect> = damage
        .iter()
        .map(|r| FlushRect {
            x: r.x as u32,
            y: r.y as u32,
            w: r.w as u32,
            h: r.h as u32,
        })
        .collect();
    for chunk in rects.chunks(kabi::display::MAX_FLUSH) {
        let _ = sys::display_flush(chunk);
    }
    if !state.drawn {
        state.drawn = true;
        println!("desktop: drew the desk at {}x{}", state.width, state.height);
    }
}

/// A client's buffer, read where its pool is mapped. `None` past the pool.
fn source<'a>(pools: &'a [Pool], pid: u64, buffer: &Buffer) -> Option<Source<'a>> {
    let pool = pools
        .iter()
        .find(|p| p.pid == pid && p.pool == buffer.pool)?;
    let words = buffer.stride as usize / 4;
    let len = words * buffer.height as usize;
    let end = buffer.offset as usize + len * 4;
    if end > pool.size as usize {
        return None;
    }
    // SAFETY: the pool is mapped read-only at `address` for `size` bytes, and the
    // buffer lies inside it (checked above and at `pool.buffer`). The client may
    // write it while it is read: the frame may tear, nothing worse.
    let data = unsafe {
        core::slice::from_raw_parts((pool.address + u64::from(buffer.offset)) as *const u32, len)
    };
    Some(Source {
        data,
        width: buffer.width as i32,
        height: buffer.height as i32,
        stride: words,
        alpha: buffer.format == Format::Argb8888,
    })
}

fn copy_to_screen(frame: &[u32], width: i32, screen: &Screen, damage: &[Rect]) {
    let words = (screen.info.stride / 4) as usize;
    // SAFETY: the kernel mapped the back buffer's `pages` pages at `address`.
    let pixels = unsafe {
        core::slice::from_raw_parts_mut(
            screen.address as *mut u32,
            screen.info.pages as usize * 1024,
        )
    };
    for r in damage {
        for y in r.y..r.bottom() {
            let from = (y * width + r.x) as usize;
            let to = y as usize * words + r.x as usize;
            let w = r.w as usize;
            if let (Some(src), Some(dst)) = (frame.get(from..from + w), pixels.get_mut(to..to + w))
            {
                dst.copy_from_slice(src);
            }
        }
    }
}

/// The hardware cursor's image: the arrow the frame draws on GOP.
fn arrow(palette: &Palette) -> [u32; 64 * 64] {
    let mut image = [0u32; 64 * 64];
    for (i, pixel) in image.iter_mut().enumerate() {
        let (x, y) = ((i % 64) as i32, (i / 64) as i32);
        *pixel = paint::arrow(x, y, palette).unwrap_or(0);
    }
    image
}

/// The appearance the plan line's arguments set, one `appearance_set` setting
/// each; one that is refused is left out, and said.
fn starting_appearance() -> Appearance {
    let args = user::args();
    let mut chosen = Appearance::default();
    for setting in appearance::SETTINGS {
        let Some(value) = user::arg(&args, setting) else {
            continue;
        };
        match chosen.set(setting, value, &Shipped) {
            Ok(next) => chosen = next,
            Err(why) => println!("desktop: {setting}={value} is refused: {}", why.0),
        }
    }
    chosen
}

/// Paints in the appearance's theme from this frame on, when it changed: the
/// whole screen again, and the hardware cursor in its colours. Nothing while the
/// painter already paints in it, so a first frame in the default theme costs
/// what it did before themes (display-claim reads a key typed during it).
fn theme(state: &mut State, screen: &Screen) {
    if state.painted_in == state.appearance {
        return;
    }
    let theme = match state.appearance.resolve(&Shipped) {
        Ok(theme) => theme,
        Err(why) => {
            println!("desktop: the appearance is refused ({}); dark it is", why.0);
            state.appearance = Appearance::default();
            return;
        }
    };
    state.painter.set_theme(&theme);
    state.extra.push(Rect::new(0, 0, state.width, state.height));
    if screen.info.cursor != 0 {
        let _ = sys::display_cursor(&arrow(state.painter.palette()), (0, 0));
    }
    println!("desktop: painting in the {} theme", theme.name);
    state.painted_in = state.appearance.clone();
}

/// Waits for the screen to come back: `false` on an error.
fn wait_for_screen() -> bool {
    let mut events = [InputEvent {
        time_ns: 0,
        kind: Kind::Screen,
        code: 0,
        value: 0,
    }; 8];
    loop {
        let Ok(n) = sys::input_read(&mut events, 1000) else {
            return false;
        };
        if events[..n]
            .iter()
            .any(|e| e.kind == Kind::Screen && e.value == 1)
        {
            return true;
        }
    }
}
