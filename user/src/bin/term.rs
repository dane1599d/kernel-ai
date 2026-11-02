//! `term`, the terminal window (P3 task 8): the kernel console's `kai>`
//! terminal in a `desktop` window. It is a mirror, not a second shell (P3 plan
//! §4 item 5): the kernel's terminal still edits the line, talks to the shell
//! and draws every question.
//!
//! - It reads the console's changed rows with TERM_READ (`termcore::mirror`),
//!   keeps its own copy, and paints the rows that changed with `gfx2d::term`
//!   into a `kai-surface 1` buffer. The window shows the console's last rows,
//!   with the cursor's row always in view.
//! - It takes no keys. While its window has focus `desktop` lends the keys to
//!   the kernel's terminal (INPUT_TO_CONSOLE), and an ask is answered on the
//!   console, which takes the screen while it waits (ADR 0005 §7).
//! - If `desktop` restarts, the new one greets it as a new connection and it
//!   opens its window again from the pool it kept, as `deskdemo` does.
//!
//! The font is JetBrains Mono, built in until P2's `fs` serves `/system/fonts`
//! (P3 plan §4 item 3).

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use gfx2d::term::{Layout, paint_row};
use gfx2d::{Canvas, Damage, Fonts, Rect, Text};
use kabi::handle::{Handle, Rights};
use surfabi::proto::{Event, Interface, REGISTRY, Request};
use surfabi::{Format, wire};
use termcore::mirror::{MAX_BYTES, Snapshot};
use termcore::style::Kind;
use user::{logln, println, sys};

user::entry!(main);

/// JetBrains Mono (OFL-1.1, assets/fonts/), for both of `Text`'s faces: the
/// terminal draws only monospace.
static MONO: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Variable.ttf");

const W: u32 = 960;
const H: u32 = 600;
const BUFFER: u32 = W * H * 4;
const POOL: u32 = 4096 + BUFFER;
const POOL_ID: u32 = 2;
const SURFACE: u32 = 3;
const BUFFER_ID: u32 = 4;
const TOPLEVEL: u32 = 5;
const CALL_MS: u64 = 5_000;
/// How often it reads the console and asks `desktop` for its events.
const POLL_MS: u64 = 16;
/// The window's background where no row is: the console's ink.
const BACKGROUND: u32 = 0xFF00_0000 | look::console::INK;

/// Its copy of the console's rows.
struct Mirror {
    rows: Vec<(Kind, Vec<char>)>,
    cols: usize,
    seq: u64,
    scrolls: u64,
    cursor: Option<(usize, usize)>,
    /// The first console row the window shows.
    offset: usize,
}

impl Mirror {
    fn new() -> Self {
        Self {
            rows: Vec::new(),
            cols: 0,
            seq: 0,
            scrolls: 0,
            cursor: None,
            offset: 0,
        }
    }

    /// Takes `snapshot`; returns the console rows to repaint, or `None` for all.
    fn apply(&mut self, snapshot: &Snapshot<'_>) -> Option<Vec<usize>> {
        let blank = || (Kind::Log { tag: 0 }, Vec::new());
        let mut all = false;
        if snapshot.rows != self.rows.len() || snapshot.cols != self.cols {
            self.rows = (0..snapshot.rows).map(|_| blank()).collect();
            self.cols = snapshot.cols;
            all = true;
        }
        let scrolled = snapshot.scrolls.saturating_sub(self.scrolls) as usize;
        if scrolled > 0 {
            let n = scrolled.min(self.rows.len());
            self.rows.drain(..n);
            self.rows.extend((0..n).map(|_| blank()));
            all = true;
        }
        let mut changed = Vec::new();
        for row in snapshot.rows() {
            let text: Vec<char> = row.text.chars().collect();
            if matches!(row.kind, Kind::Question | Kind::Reply | Kind::Error)
                && self.rows[row.row].1 != text
            {
                logln!("term: shows {}", row.text);
            }
            self.rows[row.row] = (row.kind, text);
            changed.push(row.row);
        }
        for cursor in [self.cursor, snapshot.cursor].into_iter().flatten() {
            changed.push(cursor.0);
        }
        self.cursor = snapshot.cursor;
        self.seq = snapshot.seq;
        self.scrolls = snapshot.scrolls;
        (!all).then_some(changed)
    }

    /// The first row to show in `shown` window rows: the last rows in use,
    /// with the cursor's row in view.
    fn offset_for(&self, shown: usize) -> usize {
        let used = self
            .rows
            .iter()
            .rposition(|(_, text)| !text.is_empty())
            .map_or(0, |row| row + 1)
            .max(self.cursor.map_or(0, |(row, _)| row + 1));
        used.saturating_sub(shown)
    }
}

fn main() -> i32 {
    let Some(route) = user::route(0) else {
        println!("term: no route to desktop");
        return 1;
    };
    let pages = u64::from(POOL).div_ceil(4096);
    let Ok(shm) = sys::shm_create(pages) else {
        println!("term: no memory for its pool");
        return 1;
    };
    let Ok(address) = sys::shm_map(shm, true) else {
        println!("term: cannot map its pool");
        return 1;
    };
    // SAFETY: the pool is mapped read-write at `address`, `pages` pages long.
    let pool =
        unsafe { core::slice::from_raw_parts_mut(address as *mut u32, pages as usize * 1024) };
    let Some(mut text) = Text::new(
        Fonts {
            sans: MONO,
            mono: MONO,
        },
        true,
    ) else {
        println!("term: cannot read its font");
        return 1;
    };
    let layout = Layout::new(&text, Rect::new(0, 0, W as i32, H as i32), 1.0);
    let start = 4096 / 4;
    let pixels = &mut pool[start..start + (W * H) as usize];
    pixels.fill(BACKGROUND);
    if !open(route, shm) {
        return 1;
    }
    logln!(
        "term: opened its window, {} by {} cells",
        layout.cols,
        layout.rows
    );
    let mut mirror = Mirror::new();
    let mut snapshot = vec![0u8; MAX_BYTES];
    let mut reply = vec![0u8; kabi::MAX_MESSAGE];
    loop {
        sys::sleep_ms(POLL_MS);
        let mut damage = Damage::<32>::default();
        let mut canvas = Canvas {
            pixels: &mut *pixels,
            width: W,
            height: H,
            stride_px: W,
        };
        if let Ok(len) = sys::term_read(&mut snapshot, mirror.seq)
            && let Ok(read) = Snapshot::decode(&snapshot[..len.min(snapshot.len())])
        {
            let changed = mirror.apply(&read);
            let offset = mirror.offset_for(layout.rows);
            let rows: Vec<usize> = match changed {
                Some(changed) if offset == mirror.offset => changed,
                _ => {
                    canvas.fill(Rect::new(0, 0, W as i32, H as i32), BACKGROUND);
                    damage.add(Rect::new(0, 0, W as i32, H as i32));
                    (0..mirror.rows.len()).collect()
                }
            };
            mirror.offset = offset;
            for row in rows {
                let Some(shown) = row.checked_sub(offset).filter(|&r| r < layout.rows) else {
                    continue;
                };
                let (kind, cells) = &mirror.rows[row];
                let cursor = mirror.cursor.filter(|&(r, _)| r == row).map(|(_, c)| c);
                damage.add(paint_row(
                    &mut canvas,
                    &mut text,
                    &layout,
                    shown,
                    cells,
                    *kind,
                    cursor,
                ));
            }
        }
        let requests = commit(&damage);
        let Ok(len) = sys::call_route(route, &requests, &mut reply, CALL_MS) else {
            continue;
        };
        let mut rest = &reply[..len.min(reply.len())];
        let mut restarted = false;
        while let Ok((header, args, next)) = wire::split(rest) {
            rest = next;
            // Globals come only on a new connection: this is another desktop.
            if header.object == REGISTRY && header.opcode == 0 {
                restarted = true;
            } else if header.object == TOPLEVEL
                && Event::decode(Interface::Toplevel, header.opcode, args) == Ok(Event::Close)
            {
                logln!("term: the terminal stays open; Ctrl+Alt+F12 shows the console");
            }
        }
        if restarted && open(route, shm) {
            logln!("term: opened its window again after desktop restarted");
        }
    }
}

/// The damage and a commit, or nothing when nothing changed.
fn commit(damage: &Damage<32>) -> Vec<u8> {
    if damage.is_empty() {
        return Vec::new();
    }
    let mut requests: Vec<(u32, Request<'_>)> = damage
        .rects()
        .iter()
        .map(|r| {
            (
                SURFACE,
                Request::DamageBuffer(surfabi::Rect {
                    x: r.x.max(0) as u32,
                    y: r.y.max(0) as u32,
                    w: r.w.max(0) as u32,
                    h: r.h.max(0) as u32,
                }),
            )
        })
        .collect();
    requests.push((SURFACE, Request::Commit));
    requests
        .iter()
        .flat_map(|(object, request)| request.encode(*object).unwrap_or_default())
        .collect()
}

/// Opens the window on the pool `shm`, sending `desktop` a copy of the handle
/// so the pool stays this client's for another `desktop` later.
fn open(route: Handle, shm: Handle) -> bool {
    let rights = Rights::MAP | Rights::READ | Rights::TRANSFER;
    let Ok(copy) = sys::handle_dup(shm, rights) else {
        println!("term: cannot copy its pool's handle");
        return false;
    };
    let requests = [
        (
            REGISTRY,
            Request::Bind {
                name: 1,
                interface: "pool",
                version: 1,
                id: POOL_ID,
            },
        ),
        (POOL_ID, Request::PoolCreate { size: POOL }),
        (
            REGISTRY,
            Request::Bind {
                name: 2,
                interface: "surface",
                version: 1,
                id: SURFACE,
            },
        ),
        (
            POOL_ID,
            Request::PoolBuffer {
                id: BUFFER_ID,
                offset: 4096,
                width: W,
                height: H,
                stride: W * 4,
                format: Format::Xrgb8888,
            },
        ),
        (SURFACE, Request::GetToplevel { id: TOPLEVEL }),
        (TOPLEVEL, Request::SetTitle("Terminal")),
        (SURFACE, Request::Attach { buffer: BUFFER_ID }),
        (
            SURFACE,
            Request::DamageBuffer(surfabi::Rect {
                x: 0,
                y: 0,
                w: W,
                h: H,
            }),
        ),
        (SURFACE, Request::Commit),
    ];
    let bytes: Vec<u8> = requests
        .iter()
        .flat_map(|(object, request)| request.encode(*object).unwrap_or_default())
        .collect();
    let mut reply = vec![0u8; kabi::MAX_MESSAGE];
    match sys::call_h(route, &bytes, &mut reply, CALL_MS, &[copy], &mut []) {
        Ok(_) => true,
        Err(error) => {
            println!("term: desktop refused its window: {error:?}");
            false
        }
    }
}
