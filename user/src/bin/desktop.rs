//! `desktop`, the compositor (ADR 0005 §1, P3 task 6): it claims the screen,
//! serves `kai-surface 1` to its clients, and draws their windows with
//! `deskcore`'s rules. This first version draws chrome as solid colours and no
//! text; lane G-render's `gfx2d` and `look` bring type and the Ember Workbench
//! tokens.
//!
//! - A client calls the `desktop` service with a run of `surfabi` messages; the
//!   reply carries the events waiting for it. `pool.create` brings its `Shm`,
//!   mapped here read-only.
//! - Input comes from the kernel's queue (INPUT_READ); Ctrl+Alt+F12 takes the
//!   screen away (a `Screen` event) and gives it back.
//! - Each frame redraws only the desk's damage into a frame of its own, then
//!   copies those rectangles to the screen and flushes them.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use deskcore::compose::{Pixels, Source};
use deskcore::desk::{Button, Desk, Effect, Focus, Input, WindowId};
use deskcore::geom::{Point, Rect};
use deskcore::server::{Buffer, Client, Need};
use deskcore::{chrome, trusted};
use kabi::display::{DisplayInfo, FlushRect};
use kabi::handle::{Handle, RecvInfo};
use kabi::input::{InputEvent, Kind};
use surfabi::Format;
use user::{println, sys};

user::entry!(main);

/// Ember Workbench's dark tokens (ADR 0005 §8), opaque.
mod color {
    use look::DARK;

    const fn opaque(rgb: u32) -> u32 {
        0xFF00_0000 | rgb
    }

    pub const DESK: u32 = opaque(DARK.desk);
    pub const WINDOW: u32 = opaque(DARK.window);
    pub const LINE: u32 = opaque(DARK.line);
    pub const LINE_STRONG: u32 = opaque(DARK.line_strong);
    pub const RAISED: u32 = opaque(DARK.raised);
    pub const AI: u32 = opaque(DARK.ai);
    pub const ASK: u32 = opaque(DARK.ask);
    /// The controls until they get their icons.
    pub const CONTROL: u32 = opaque(DARK.line);
}

/// Waits for input or a client, in turn, so neither starves.
const INPUT_WAIT_MS: u64 = 2;
const CLIENT_WAIT_MS: u64 = 2;

struct Pool {
    pid: u64,
    pool: u32,
    address: u64,
    size: u32,
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
    frame: Vec<u32>,
    width: i32,
    height: i32,
    /// Frames as last logged, to report moves and closes once.
    known: Vec<(WindowId, Rect)>,
    focus: Focus,
}

fn main() -> i32 {
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
            frame: vec![0; (width * height) as usize],
            width,
            height,
            known: Vec::new(),
            focus: Focus::Nothing,
        };
        println!("desktop: claimed {width}x{height}");
        if !run(&mut state, &screen) {
            return 1;
        }
        let _ = sys::shm_unmap(screen.address);
        println!("desktop: lost the screen to the console");
        if !wait_for_screen() {
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
        let _ = sys::display_cursor(&arrow(), (0, 0));
        let _ = sys::display_cursor_move(info.width / 2, info.height / 2);
    }
    Some(Screen { info, address })
}

/// Serves until the screen is taken: `true` then, `false` on an error.
fn run(state: &mut State, screen: &Screen) -> bool {
    let mut events = [InputEvent {
        time_ns: 0,
        kind: Kind::Screen,
        code: 0,
        value: 0,
    }; 32];
    let mut request = vec![0u8; kabi::MAX_MESSAGE];
    loop {
        let Ok(n) = sys::input_read(&mut events, INPUT_WAIT_MS) else {
            return false;
        };
        for event in &events[..n] {
            if event.kind == Kind::Screen && event.value == 0 {
                return true;
            }
            input(state, screen, event);
        }
        let mut info = RecvInfo::default();
        if let Ok(len) = sys::recv_h(&mut request, CLIENT_WAIT_MS, &mut info) {
            serve(state, &request[..len.min(request.len())], &info);
        }
        report(state);
        draw(state, screen);
    }
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
    let effects = state.desk.input(input);
    if let Input::Motion(_) = input
        && screen.info.cursor != 0
    {
        let at = state.desk.pointer();
        let _ = sys::display_cursor_move(at.x as u32, at.y as u32);
    }
    deliver(state, &effects);
}

/// Hands the desk's effects to the clients they concern.
fn deliver(state: &mut State, effects: &[Effect]) {
    for client in &mut state.clients {
        client.apply(effects);
    }
}

fn serve(state: &mut State, bytes: &[u8], info: &RecvInfo) {
    let pid = info.caller_pid;
    if !state.clients.iter().any(|c| c.pid == pid) {
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

fn apply_need(state: &mut State, at: usize, pid: u64, need: Need, handle: Option<Handle>) {
    match need {
        Need::MapPool { pool, size } => {
            let Some(handle) = handle else { return };
            match sys::shm_map(handle, false) {
                Ok(address) => {
                    state.pools.push(Pool {
                        pid,
                        pool,
                        address,
                        size,
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

/// The name a process runs under, which labels its windows.
fn name_of(pid: u64) -> alloc::string::String {
    let mut buf = vec![0u8; 4096];
    let Ok(len) = sys::list(&mut buf) else {
        return alloc::format!("pid {pid}");
    };
    alloc::string::String::from_utf8_lossy(&buf[..len.min(buf.len())])
        .lines()
        .find_map(|line| {
            let (p, name) = line.split_once(' ')?;
            (p.parse() == Ok(pid)).then(|| alloc::string::String::from(name))
        })
        .unwrap_or_else(|| alloc::format!("pid {pid}"))
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

/// Redraws the desk's damage into the frame, copies it to the screen and
/// flushes it.
fn draw(state: &mut State, screen: &Screen) {
    let damage = state.desk.take_damage();
    if damage.is_empty() {
        return;
    }
    let (width, height) = (state.width, state.height);
    let mut frame = Pixels {
        data: &mut state.frame,
        width,
        height,
        stride: width as usize,
    };
    for &clip in &damage {
        frame.fill(clip, clip, color::DESK);
        let desk = &state.desk;
        for window in desk.windows().iter().filter(|w| !w.minimized) {
            let focused = desk.focus() == Focus::Window(window.id);
            draw_chrome(&mut frame, window.frame, focused, clip);
            if desk.agent() == Some(window.id) {
                outline(&mut frame, trusted::outline(window.frame), color::AI, clip);
            }
            let content = window.content();
            if let Some((_, pid, buffer)) = state.shown.iter().find(|(w, ..)| *w == window.id)
                && let Some(source) = source(&state.pools, *pid, buffer)
            {
                frame.draw(
                    &source,
                    (content.x, content.y),
                    content.intersect(&clip).unwrap_or_default(),
                );
            } else {
                frame.fill(content, clip, color::WINDOW);
            }
        }
        frame.fill(desk.top_bar(), clip, color::WINDOW);
        frame.fill(desk.panel(), clip, color::WINDOW);
        if let Some(ask) = desk.ask() {
            let card = trusted::card(width, height, &ask);
            frame.fill(card, clip, color::RAISED);
            frame.fill(Rect::new(card.x, card.y, card.w, 3), clip, color::ASK);
        }
    }
    copy_to_screen(&state.frame, width, screen, &damage);
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
}

fn draw_chrome(frame: &mut Pixels, f: Rect, focused: bool, clip: Rect) {
    let line = if focused {
        color::LINE_STRONG
    } else {
        color::LINE
    };
    frame.fill(f, clip, line);
    frame.fill(chrome::title_bar(f), clip, color::WINDOW);
    for (_, control) in chrome::controls(f) {
        frame.fill(control, clip, color::CONTROL);
    }
}

fn outline(frame: &mut Pixels, r: Rect, color: u32, clip: Rect) {
    let t = trusted::OUTLINE;
    for edge in [
        Rect::new(r.x, r.y, r.w, t),
        Rect::new(r.x, r.bottom() - t, r.w, t),
        Rect::new(r.x, r.y, t, r.h),
        Rect::new(r.right() - t, r.y, t, r.h),
    ] {
        frame.fill(edge, clip, color);
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

/// A plain arrow for the hardware cursor: `text` inside a `desk` edge.
fn arrow() -> [u32; 64 * 64] {
    let mut image = [0u32; 64 * 64];
    for y in 0..18usize {
        for x in 0..=y.min(11) {
            let edge = x == 0 || x == y.min(11) || y == 17;
            image[y * 64 + x] = match edge {
                true => color::DESK,
                false => 0xFF00_0000 | look::DARK.text,
            };
        }
    }
    image
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
