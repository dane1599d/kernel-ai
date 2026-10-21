//! The status bar across the top of the screen, and the busy sign under the last
//! line while the terminal waits for the shell or the model. A `status` thread
//! refreshes both; the terminal says when it starts and stops waiting. Neither is
//! part of the grid, so nothing here reaches serial or `screenlog`.
//!
//! The bar shows the model's state, how busy the CPUs were over the last second,
//! memory in use and the time since boot. (The kernel has no wall clock yet, and
//! no token rate: the llm service does not report one.)

use super::fb::{Fb, blend};
use super::layout::Layout;
use super::theme::{BAR, DIM, EMBER, GOLD, INK, PAPER, RULE, SAGE, SLATE};
use crate::{hpet, llm, log, metrics, threads, version};
use alloc::format;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Cells on the right of the status bar.
pub const FIELD: usize = 60;
/// Cells the brand on the left takes, which the field leaves free.
const BRAND: usize = 24;
const TICK_MS: u64 = 125;
/// Ticks between refreshes of the numbers: one second.
const REFRESH_TICKS: u64 = 8;
/// A cell of the field that shows the model's state as a dot.
const DOT: char = '\u{1}';
/// Cells the busy sign's label is padded to, so a shorter one covers a longer.
const LABEL: usize = 16;
const PAGE_KIB: u64 = 4;

/// The right side of the status bar: a character and a colour per cell.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Field {
    cells: [(char, u32); FIELD],
}

/// When the terminal started waiting, in HPET nanoseconds; 0 while it is not.
static BUSY_SINCE: AtomicU64 = AtomicU64::new(0);
static STARTED: AtomicBool = AtomicBool::new(false);

/// Marks the terminal as waiting for the shell or the model, or done waiting.
pub fn set_busy(busy: bool) {
    let since = if busy { hpet::nanos().max(1) } else { 0 };
    BUSY_SINCE.store(since, Ordering::Relaxed);
    if !busy {
        super::hide_sign();
    }
}

/// Starts the status thread, once, if there is a framebuffer console.
pub fn start() {
    if super::size().is_none() || STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    if let Err(why) = threads::spawn("status", run, 0) {
        log!("status: the status bar has no thread ({why})");
    }
}

fn run(_: usize) {
    let mut shown: Option<Field> = None;
    let (mut busy_cpus, mut samples) = (0, 0);
    let mut was_busy = false;
    let mut tick: u64 = 0;
    loop {
        busy_cpus += threads::busy_cpus();
        samples += 1;
        let since = BUSY_SINCE.load(Ordering::Relaxed);
        let waited = (since != 0).then(|| hpet::nanos().saturating_sub(since) / 1_000_000_000);
        if let Some(seconds) = waited {
            let label = format!("thinking {seconds}s");
            super::show_sign(tick as usize, &format!("{label:LABEL$}"));
        }
        if tick.is_multiple_of(REFRESH_TICKS) || waited.is_some() != was_busy {
            let field = field(waited, busy_cpus, samples);
            if shown != Some(field) {
                super::show_status(&field);
                shown = Some(field);
            }
            (busy_cpus, samples) = (0, 0);
        }
        was_busy = waited.is_some();
        tick += 1;
        threads::sleep_ms(TICK_MS);
    }
}

/// The field for now: `waited` seconds into a wait, and `busy` busy CPUs counted
/// over `samples` looks.
fn field(waited: Option<u64>, busy: usize, samples: usize) -> Field {
    let m = metrics::snapshot();
    let llm_runs = m.processes[..m.process_count as usize]
        .iter()
        .any(|p| &p.name[..p.name_len as usize] == b"llm");
    let mut out = Builder::new();
    match (waited, llm::available(), llm_runs) {
        (Some(seconds), ..) => out.dot(EMBER).push(&format!("thinking {seconds}s"), PAPER),
        (None, false, _) => out.dot(DIM).push("no model", SLATE),
        (None, true, false) => out.dot(GOLD).push("model starting", PAPER),
        (None, true, true) => out.dot(SAGE).push("model ready", PAPER),
    };
    let cpus = m.cpus.max(1) as usize;
    let load = 100 * busy / (samples.max(1) * cpus);
    out.push("   cpu ", DIM)
        .push(&format!("{load}%"), PAPER)
        .push(&format!(" of {cpus}"), DIM);
    let mib = |frames: u64| frames * PAGE_KIB / 1024;
    let used = mib(m.total_frames - m.free_frames);
    out.push("   mem ", DIM)
        .push(&format!("{used}"), PAPER)
        .push(&format!("/{} MiB", mib(m.total_frames)), DIM);
    let s = m.uptime_ns / 1_000_000_000;
    out.push("   up ", DIM).push(
        &format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60),
        PAPER,
    );
    out.right_aligned()
}

struct Builder {
    cells: [(char, u32); FIELD],
    len: usize,
}

impl Builder {
    fn new() -> Self {
        Self {
            cells: [(' ', INK); FIELD],
            len: 0,
        }
    }

    fn push(&mut self, text: &str, color: u32) -> &mut Self {
        for ch in text.chars() {
            if let Some(cell) = self.cells.get_mut(self.len) {
                *cell = (ch, color);
                self.len += 1;
            }
        }
        self
    }

    fn dot(&mut self, color: u32) -> &mut Self {
        self.push(&format!("{DOT} "), color)
    }

    fn right_aligned(&self) -> Field {
        let mut cells = [(' ', INK); FIELD];
        cells[FIELD - self.len..].copy_from_slice(&self.cells[..self.len]);
        Field { cells }
    }
}

/// Paints the bar and its left side, the brand: an ember ring, the name, the
/// version and milestone. Once, at boot.
pub fn paint_brand(fb: Fb, layout: &Layout) {
    let font = layout.font;
    fb.fill(0, 0, layout.width, layout.bar - 1, BAR);
    fb.fill(0, layout.bar - 1, layout.width, 1, RULE);
    let y = (layout.bar - font.height) / 2;
    let radius = font.height / 4;
    let (cx, cy) = (layout.left + radius, layout.bar / 2);
    fb.disc(cx, cy, radius, EMBER, BAR);
    fb.disc(cx, cy, radius / 2, BAR, EMBER);
    let x = layout.left + 2 * radius + font.width;
    let x = fb.text(font, (x, y), "Kernel AI", (PAPER, true), BAR);
    let version = format_version();
    fb.text(
        font,
        (x + font.width, y),
        version.as_str(),
        (DIM, false),
        BAR,
    );
}

/// `0.10.0 M10a`, without the heap, which does not exist yet at boot.
fn format_version() -> Short {
    let mut out = Short::default();
    for part in [version::VERSION, " ", version::MILESTONE] {
        out.push(part);
    }
    out
}

/// A few characters on the stack.
#[derive(Default)]
struct Short {
    bytes: [u8; 32],
    len: usize,
}

impl Short {
    fn push(&mut self, text: &str) {
        let take = text.len().min(self.bytes.len() - self.len);
        self.bytes[self.len..self.len + take].copy_from_slice(&text.as_bytes()[..take]);
        self.len += take;
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}

/// Paints the right side of the bar, as much of it as fits beside the brand.
pub fn paint_field(fb: Fb, layout: &Layout, field: &Field) {
    let font = layout.font;
    let shown = FIELD.min(layout.cols.saturating_sub(BRAND));
    let right = layout.left + layout.cols * font.width;
    let y = (layout.bar - font.height) / 2;
    for (i, &(ch, color)) in field.cells[FIELD - shown..].iter().enumerate() {
        let x = right - (shown - i) * font.width;
        if ch == DOT {
            fb.fill(x, y, font.width, font.height, BAR);
            let radius = (font.height / 6).max(2);
            fb.disc(
                x + font.width / 2,
                y + font.height / 2 + 1,
                radius,
                color,
                BAR,
            );
        } else {
            fb.glyph(font, (x, y), ch, (color, false), BAR);
        }
    }
}

/// Paints the busy sign on text row `row`: three dots, the `frame`th of each three
/// lit, and `label`.
pub fn paint_sign(fb: Fb, layout: &Layout, row: usize, frame: usize, label: &str) {
    let font = layout.font;
    let (x, y) = layout.cell(row, 0);
    let radius = (font.height / 7).max(2);
    let step = font.width + font.width / 2;
    for dot in 0..3 {
        let color = match dot == frame % 3 {
            true => EMBER,
            false => blend(90, EMBER, INK),
        };
        let cx = x + font.width / 2 + dot * step;
        fb.disc(cx, y + font.height / 2 + 1, radius, color, INK);
    }
    fb.text(font, (x + 5 * font.width, y), label, (SLATE, false), INK);
}
