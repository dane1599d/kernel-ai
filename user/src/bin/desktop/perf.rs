//! `perf=on` on the plan line (`test=desk-perf`, P3 close): S05-1's budgets
//! (P3 plan section 5) timed on this desktop rather than the spike. Only the
//! kernel (pid 0) may ask, and only with `perf=on`; the reply is the scenes'
//! lines, each logged too:
//! - `perf scenes <frames>`: (a) the whole screen composited `frames` times;
//!   (b) the top window dragged 200 px right and back, a px a frame, through
//!   the same input path as the pointer's, render and flush timed apart; (c) a
//!   160x45 page of terminal text on `gfx2d`'s terminal path, warm `frames`
//!   times and cold (an empty atlas) [`COLD_PAGES`] times.
//! - `perf forget`: drops the keys timed so far.
//! - `perf keys`: (e), each key since `perf forget` from the kernel's input
//!   event to the flush that showed it.
//! - `perf drag`: (b) again, for (f) under the load the kernel starts.
//!
//! S05-1's (d) is the sheet's spring, and this desktop has none: a card shows
//! in one frame. Between timed frames it sleeps 1 ms, outside the timing, so
//! the kernel never takes it for a runaway.

use super::{FONTS, Screen, State};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use deskcore::chrome;
use deskcore::geom::Rect;
use gfx2d::term::{self, Layout};
use gfx2d::{Image, Text};
use kabi::input::{InputEvent, Kind};
use termcore::style::Kind as Row;
use user::{println, sys};

/// Cold text pages: each starts from an empty atlas, as S05-1's did.
const COLD_PAGES: usize = 10;
/// How far (b) drags: right, then back.
const DRAG_PX: i32 = 200;
/// The terminal page S05-1 timed.
const COLS: usize = 160;
const ROWS: usize = 45;
/// Most frames one request may ask for, and most keys kept.
const MAX_FRAMES: usize = 1_000;
const MAX_KEYS: usize = 1_000;
/// Where (b) takes hold of the window: this far into its title bar.
const GRIP: (i32, i32) = (100, chrome::TITLE_BAR / 2);

const NS_PER_MS: u64 = 1_000_000;
/// S05-1's budgets at 1920x1080, in ns.
const FULL_P99: u64 = 8 * NS_PER_MS;
const DRAG_RENDER_P99: u64 = 8 * NS_PER_MS;
const DRAG_FLUSH_P99: u64 = 4 * NS_PER_MS;
const WARM_MAX: u64 = 4 * NS_PER_MS;
const COLD_MAX: u64 = 30 * NS_PER_MS;
const KEY_P99: u64 = 8 * NS_PER_MS;
const LOADED_P99: u64 = 16_700_000;

#[derive(Default)]
pub struct Perf {
    /// The last frame's render and flush times, from `draw`.
    last: Option<(u64, u64)>,
    /// Key presses not yet on the screen, by their input event's time.
    pending: Vec<u64>,
    /// Key press to flush, for (e).
    keys: Vec<u64>,
}

/// A `Perf` when the plan line has `perf=on`.
pub fn from_args() -> Option<Perf> {
    let args = user::args();
    (user::arg(&args, "perf") == Some("on")).then(Perf::default)
}

/// A key press, at the kernel's time `at`.
pub fn key(state: &mut State, at: u64) {
    if let Some(perf) = &mut state.perf
        && perf.keys.len() + perf.pending.len() < MAX_KEYS
    {
        perf.pending.push(at);
    }
}

/// A frame drawn: `render` and `flush` long, flushed at `done`.
pub fn drew(state: &mut State, render: u64, flush: u64, done: u64) {
    let Some(perf) = &mut state.perf else {
        return;
    };
    perf.last = Some((render, flush));
    for at in perf.pending.drain(..) {
        perf.keys.push(done.saturating_sub(at));
    }
}

/// The kernel's `perf` requests; `None` for anything else.
pub fn request(state: &mut State, screen: &Screen, bytes: &[u8], pid: u64) -> Option<String> {
    let text = core::str::from_utf8(bytes).ok()?;
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.first() != Some(&"perf") {
        return None;
    }
    if pid != 0 || state.perf.is_none() {
        return Some(String::from("err perf is the kernel's, with perf=on"));
    }
    let lines = match words[1..] {
        ["scenes", frames] => match frames.parse::<usize>() {
            Ok(n) if (1..=MAX_FRAMES).contains(&n) => scenes(state, screen, n),
            _ => return Some(String::from("err perf scenes takes 1 to 1000 frames")),
        },
        ["keys"] => {
            let keys = state.perf.as_mut().map(|p| core::mem::take(&mut p.keys));
            Vec::from([line(
                "(e) key to flush",
                &keys.unwrap_or_default(),
                KEY_P99,
                Pick::P99,
            )])
        }
        ["forget"] => {
            if let Some(perf) = &mut state.perf {
                perf.keys.clear();
                perf.pending.clear();
            }
            return Some(String::from("ok"));
        }
        ["drag"] => drag(state, screen, "(f) drag under load, render+flush", true),
        _ => return Some(String::from("err unknown perf request")),
    };
    for l in &lines {
        println!("desk-perf: {l}");
    }
    Some(lines.join("\n"))
}

fn scenes(state: &mut State, screen: &Screen, frames: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut full = Vec::new();
    for _ in 0..frames {
        state.extra.push(Rect::new(0, 0, state.width, state.height));
        if let Some((render, flush)) = frame(state, screen) {
            full.push(render + flush);
        }
    }
    let size = format!("{}x{}", state.width, state.height);
    lines.push(line(
        &format!("(a) full composite at {size}"),
        &full,
        FULL_P99,
        Pick::P99,
    ));
    lines.extend(drag(state, screen, "(b) drag", false));
    lines.extend(text_page(frames));
    lines
}

/// One frame of whatever is damaged, then the pause outside the timing.
fn frame(state: &mut State, screen: &Screen) -> Option<(u64, u64)> {
    if let Some(perf) = &mut state.perf {
        perf.last = None;
    }
    super::draw(state, screen);
    sys::sleep_ms(1);
    state.perf.as_ref().and_then(|p| p.last)
}

/// A pointer event through `input`, as the kernel's queue would give it.
fn pointer(state: &mut State, screen: &Screen, kind: Kind, code: u16, value: i32) {
    let event = InputEvent {
        time_ns: sys::time_ns(),
        kind,
        code,
        value,
    };
    super::input(state, screen, &event);
}

/// The top window dragged right and back; the flush apart from the render,
/// unless `total`, as (f) is judged on both.
fn drag(state: &mut State, screen: &Screen, name: &str, total: bool) -> Vec<String> {
    let Some(frame_at) = state.desk.windows().last().map(|w| w.frame) else {
        return Vec::from([format!("{name}: no window to drag")]);
    };
    let grip = (frame_at.x + GRIP.0, frame_at.y + GRIP.1);
    let at = state.desk.pointer();
    pointer(state, screen, Kind::Rel, 0, grip.0 - at.x);
    pointer(state, screen, Kind::Rel, 1, grip.1 - at.y);
    pointer(state, screen, Kind::Button, 1, 1);
    let _ = frame(state, screen);
    let (mut render, mut flush, mut both) = (Vec::new(), Vec::new(), Vec::new());
    for step in [1, -1] {
        for _ in 0..DRAG_PX {
            pointer(state, screen, Kind::Rel, 0, step);
            if let Some((r, f)) = frame(state, screen) {
                render.push(r);
                flush.push(f);
                both.push(r + f);
            }
        }
    }
    pointer(state, screen, Kind::Button, 1, 0);
    let _ = frame(state, screen);
    if total {
        return Vec::from([line(name, &both, LOADED_P99, Pick::P99)]);
    }
    Vec::from([
        line(
            &format!("{name} render"),
            &render,
            DRAG_RENDER_P99,
            Pick::P99,
        ),
        line(&format!("{name} flush"), &flush, DRAG_FLUSH_P99, Pick::P99),
    ])
}

/// (c): the page painted as the terminal window paints its rows.
fn text_page(frames: usize) -> Vec<String> {
    let Some(mut text) = Text::new(FONTS, true) else {
        return Vec::from([String::from("(c) text: the fonts do not parse")]);
    };
    let (cell_w, cell_h, _) = text.cell(1.0);
    let w = (COLS as f32 * cell_w) as i32 + 2 * gfx2d::px(12.0, 1.0) + 1;
    let h = ROWS as i32 * cell_h;
    let mut image = Image::new(w, h);
    let layout = Layout::new(&text, gfx2d::Rect::new(0, 0, w, h), 1.0);
    let lines: Vec<Vec<char>> = (0..ROWS)
        .map(|n| terminal_line(n).chars().collect())
        .collect();
    let mut page = |text: &mut Text| {
        let start = sys::time_ns();
        let mut canvas = image.canvas();
        for (row, cells) in lines.iter().enumerate() {
            let kind = Row::Log { tag: 0 };
            term::paint_row(&mut canvas, text, &layout, row, cells, kind, None);
        }
        let took = sys::time_ns() - start;
        sys::sleep_ms(1);
        took
    };
    let cold: Vec<u64> = (0..COLD_PAGES)
        .map(|_| {
            text.clear();
            page(&mut text)
        })
        .collect();
    let warm: Vec<u64> = (0..frames).map(|_| page(&mut text)).collect();
    let fits = format!("{}x{}", layout.cols, layout.rows);
    Vec::from([
        line(
            &format!("(c) text page {fits} warm"),
            &warm,
            WARM_MAX,
            Pick::Max,
        ),
        line(
            &format!("(c) text page {fits} cold"),
            &cold,
            COLD_MAX,
            Pick::Max,
        ),
    ])
}

/// Shell-like text using every printable ASCII byte, as S05-1's page did.
fn terminal_line(n: usize) -> String {
    const WORDS: [&str; 12] = [
        "kai>",
        "ls",
        "/home/user",
        "notes.txt",
        "12.4 KiB",
        "done:",
        "fn",
        "main()",
        "{}",
        "0x7f3a",
        "[ok]",
        "#[test]",
    ];
    let mut line = format!("{n:>3} ");
    let mut i = n;
    while line.len() < COLS {
        line.push_str(WORDS[i % WORDS.len()]);
        line.push(' ');
        i = i.wrapping_mul(7).wrapping_add(3);
    }
    line.truncate(COLS);
    line
}

#[derive(Clone, Copy)]
enum Pick {
    P99,
    Max,
}

/// `name: n=.. p50 .. p95 .. p99 .. max .. ms, budget <pick> .. ms: within|OVER`.
fn line(name: &str, samples: &[u64], budget: u64, pick: Pick) -> String {
    if samples.is_empty() {
        return format!("{name}: no samples");
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    // Nearest rank: the smallest sample at or above the fraction.
    let at = |percent: usize| sorted[(percent * sorted.len()).div_ceil(100).max(1) - 1];
    let (p99, max) = (at(99), sorted[sorted.len() - 1]);
    let (judged, label) = match pick {
        Pick::P99 => (p99, "p99"),
        Pick::Max => (max, "max"),
    };
    let verdict = if judged <= budget { "within" } else { "OVER" };
    format!(
        "{name}: n={} p50 {} p95 {} p99 {} max {} ms, budget {label} {} ms: {verdict}",
        sorted.len(),
        ms(at(50)),
        ms(at(95)),
        ms(p99),
        ms(max),
        ms(budget),
    )
}

fn ms(ns: u64) -> String {
    let hundredths = ns / 10_000;
    format!("{}.{:02}", hundredths / 100, hundredths % 100)
}
