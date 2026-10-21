//! Framebuffer text console. It paints `termcore::grid`: every `log!` line goes into
//! the grid, and each change repaints only the rows the grid marks as damaged. The
//! terminal's prompt and typed text are the grid's input region, redrawn below each
//! log line, with the cursor shown as an ember block. With `screenlog` on the
//! command line, each repaint is copied to serial (`termcore::screenlog`).
//!
//! Each row is coloured by its kind (`termcore::style`), read from its text, so the
//! text the grid holds, which serial and the tests read, is exactly what was
//! printed. A status bar runs across the top (`status`), outside the grid.
//!
//! Every access takes the serial line lock and then the console lock, so a reader
//! never makes another CPU's line miss the screen.

mod fb;
mod layout;
pub mod status;
mod theme;

use crate::serial::{self, Serial};
use core::fmt::{self, Write};
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use fb::Fb;
use layout::Layout;
use limine::framebuffer::Framebuffer;
use spin::Mutex;
use termcore::grid::{Grid, MAX_ROWS};
use termcore::style::Kind;

/// Cells the screen holds: 176 by 42 on CI's 1280x800, 168 by 37 on 1920x1080. The
/// grid is 128 KiB, more than a kernel stack, so it lives only in CONSOLE.
const CELLS: usize = 32 * 1024;

/// The screen's text.
pub type Screen = Grid<CELLS>;

#[derive(Clone, Copy)]
struct Output {
    fb: Fb,
    layout: Layout,
}

/// What a text row on the framebuffer shows now.
#[derive(Clone, Copy)]
struct Painted {
    /// Cells from the left that may be non-blank, so a repaint clears only those.
    width: usize,
    /// The row's background and bar; None when the pixels are unknown or not the
    /// grid's, so the next repaint covers all of it.
    look: Option<theme::Look>,
}

const UNKNOWN: Painted = Painted {
    width: 0,
    look: None,
};

struct Console {
    out: Option<Output>,
    grid: Screen,
    painted: [Painted; MAX_ROWS],
    /// The row the busy sign is drawn on (`show_sign`).
    sign: Option<usize>,
}

// SAFETY: the framebuffer pointer is only used under the CONSOLE mutex.
unsafe impl Send for Console {}

static CONSOLE: Mutex<Console> = Mutex::new(Console {
    out: None,
    grid: Grid::new(),
    painted: [UNKNOWN; MAX_ROWS],
    sign: None,
});
static SCREENLOG: AtomicBool = AtomicBool::new(false);
/// Text columns and rows of the framebuffer console; 0 when there is none.
static COLS: AtomicUsize = AtomicUsize::new(0);
static ROWS: AtomicUsize = AtomicUsize::new(0);
/// Framebuffer width and height in pixels; 0 when there is none.
static WIDTH: AtomicUsize = AtomicUsize::new(0);
static HEIGHT: AtomicUsize = AtomicUsize::new(0);
/// A text cell in pixels.
static CELL_WIDTH: AtomicUsize = AtomicUsize::new(layout::SMALL.width);
static CELL_HEIGHT: AtomicUsize = AtomicUsize::new(layout::SMALL.height);

/// Columns and rows of text the screen holds, if there is a framebuffer console.
pub fn size() -> Option<(usize, usize)> {
    match (COLS.load(Ordering::Relaxed), ROWS.load(Ordering::Relaxed)) {
        (0, _) | (_, 0) => None,
        size => Some(size),
    }
}

/// Width and height of the framebuffer in pixels, if there is a console on it.
pub fn pixels() -> Option<(usize, usize)> {
    match (
        WIDTH.load(Ordering::Relaxed),
        HEIGHT.load(Ordering::Relaxed),
    ) {
        (0, _) | (_, 0) => None,
        size => Some(size),
    }
}

/// Width and height of a text cell in pixels: 7 by 16 up to 1280x800.
pub fn cell() -> (usize, usize) {
    (
        CELL_WIDTH.load(Ordering::Relaxed),
        CELL_HEIGHT.load(Ordering::Relaxed),
    )
}

/// Takes over a 32-bpp framebuffer. Returns false for other formats.
pub fn init(fb: &Framebuffer) -> bool {
    let Some(base) = NonNull::new(fb.address().cast::<u32>()) else {
        return false;
    };
    if fb.bpp != 32 {
        return false;
    }
    let (width, height) = (fb.width as usize, fb.height as usize);
    let screen = Fb {
        base,
        stride: (fb.pitch / 4) as usize,
    };
    let mut layout = Layout::new(width, height);
    screen.fill(0, 0, width, height, theme::INK);
    status::paint_brand(screen, &layout);
    let (cols, rows) = serial::with_line(|| {
        let mut console = CONSOLE.lock();
        let size = console.grid.resize(layout.cols, layout.rows);
        (layout.cols, layout.rows) = size;
        console.out = Some(Output { fb: screen, layout });
        console.painted = [UNKNOWN; MAX_ROWS];
        console.sign = None;
        // Every row is blank ink already.
        console.grid.take_damage();
        console.painted.fill(Painted {
            width: 0,
            look: Some(theme::look(Kind::of(&[]))),
        });
        size
    });
    COLS.store(cols, Ordering::Relaxed);
    ROWS.store(rows, Ordering::Relaxed);
    WIDTH.store(width, Ordering::Relaxed);
    HEIGHT.store(height, Ordering::Relaxed);
    CELL_WIDTH.store(layout.font.width, Ordering::Relaxed);
    CELL_HEIGHT.store(layout.font.height, Ordering::Relaxed);
    true
}

/// Reads `screenlog` from the command line.
pub fn configure(cmdline: &str) {
    let on = cmdline.split_whitespace().any(|arg| arg == "screenlog");
    SCREENLOG.store(on, Ordering::Relaxed);
}

/// Writes log text to the console if it is free; `serial::_print` calls it under the
/// line lock. A fault raised while the console is mid-write skips the console instead
/// of deadlocking; serial still gets the text.
pub fn print(args: fmt::Arguments) {
    if let Some(mut console) = CONSOLE.try_lock() {
        let _ = console.grid.write_fmt(args);
        console.flush();
    }
}

/// Shows `prompt` and the typed `text` below the log, the cursor `cursor` characters
/// into `text`.
pub fn show_input(prompt: &str, text: &str, cursor: usize) {
    update(|grid| grid.set_input(prompt, text, cursor));
}

/// Takes the prompt and typed text off the screen.
pub fn hide_input() {
    update(Screen::hide_input);
}

fn update(change: impl FnOnce(&mut Screen)) {
    serial::with_line(|| {
        if let Some(mut console) = CONSOLE.try_lock() {
            change(&mut console.grid);
            console.flush();
        }
    });
}

/// Runs `f` on the console's framebuffer and layout, and on the console itself.
fn draw(f: impl FnOnce(&mut Console, Output)) {
    serial::with_line(|| {
        if let Some(mut console) = CONSOLE.try_lock()
            && let Some(out) = console.out
        {
            f(&mut console, out);
        }
    });
}

/// Runs `f` on the screen's text, with no line drawn meanwhile. None without a
/// framebuffer console, or while a fault on this CPU holds it. `f` runs with
/// interrupts off and must not allocate.
pub fn with_grid<R>(f: impl FnOnce(&Screen) -> R) -> Option<R> {
    serial::with_line(|| {
        let console = CONSOLE.try_lock()?;
        console.out?;
        Some(f(&console.grid))
    })
}

impl Console {
    /// Brings the framebuffer up to date with the grid, then copies the repainted
    /// rows to serial under `screenlog`. Interrupts are off, so it repaints only
    /// rows that changed, and in each only the cells that are or were non-blank,
    /// unless the row's background changed. A busy sign goes with any change; the
    /// status thread draws it again where there is room.
    fn flush(&mut self) {
        let damage = self.grid.take_damage();
        let Some(out) = self.out else {
            return;
        };
        let (_, rows) = self.grid.size();
        let mut sign = self.sign.take();
        if damage.scrolled > 0 && damage.scrolled < rows {
            let layout = out.layout;
            let shift = damage.scrolled * layout.pitch;
            out.fb.move_up(
                layout.top + shift,
                layout.top,
                (rows - damage.scrolled) * layout.pitch,
            );
            self.painted.copy_within(damage.scrolled..rows, 0);
            sign = sign.and_then(|row| row.checked_sub(damage.scrolled));
        }
        let input = self.grid.input_span();
        for row in (0..rows).filter(|&row| damage.has(row) || sign == Some(row)) {
            self.paint_row(out, row, input);
        }
        if let Some((row, col)) = self.grid.cursor() {
            let layout = out.layout;
            let (x, y) = layout.cell(row, col);
            let ch = self.grid.row(row)[col];
            let font = layout.font;
            out.fb
                .glyph(font, (x, y), ch, (theme::INK, false), theme::EMBER);
            self.painted[row].width = self.painted[row].width.max(col + 1);
        }
        if damage.rows != 0 && SCREENLOG.load(Ordering::Relaxed) {
            // Only `:clear`'s bare form feed leaves serial mid-line.
            if !serial::at_line_start() {
                let _ = Serial.write_str("\n");
            }
            let _ = termcore::screenlog::write(&mut Serial, &self.grid, &damage);
        }
    }

    /// Repaints text row `row`; `input` is the grid's input span.
    fn paint_row(&mut self, out: Output, row: usize, input: Option<(usize, usize, usize)>) {
        let layout = out.layout;
        let cells = self.grid.row(row);
        let live = input.filter(|&(top, end, _)| (top..end).contains(&row));
        let kind = match live {
            Some((top, _, prompt)) if row == top => Kind::Input { prompt },
            Some(_) => Kind::Input { prompt: 0 },
            None => Kind::of(cells),
        };
        let look = theme::look(kind);
        let was = self.painted[row];
        let whole = was.look != Some(look);
        let width = self.grid.row_width(row);
        if whole {
            paint_frame(out, row, look);
        }
        let upto = if whole {
            layout.cols
        } else {
            width.max(was.width)
        };
        let font = layout.font;
        for (col, &ch) in cells.iter().enumerate().take(upto) {
            let (x, y) = layout.cell(row, col);
            match theme::ink(kind.tone(col)) {
                Some(ink) => out.fb.glyph(font, (x, y), ch, ink, look.background),
                None => out.fb.fill(x, y, font.width, font.height, look.background),
            }
        }
        self.painted[row] = Painted {
            width,
            look: Some(look),
        };
    }
}

/// Paints the parts of a text row that are not cells: the margins with their bar,
/// and the gap under the glyphs. Each pixel once, so nothing flickers.
fn paint_frame(out: Output, row: usize, look: theme::Look) {
    let (layout, fb) = (out.layout, out.fb);
    let (left, y) = layout.cell(row, 0);
    let (pitch, glyph) = (layout.pitch, layout.font.height);
    let right = left + layout.cols * layout.font.width;
    let (bar_x, bar_width) = layout.bar_x();
    let bg = look.background;
    fb.fill(0, y, bar_x, pitch, bg);
    fb.fill(bar_x, y, bar_width, pitch, look.bar.unwrap_or(bg));
    fb.fill(
        bar_x + bar_width,
        y,
        left.saturating_sub(bar_x + bar_width),
        pitch,
        bg,
    );
    fb.fill(left, y + glyph, right - left, pitch - glyph, bg);
    fb.fill(right, y, layout.width - right, pitch, bg);
}

/// Draws the busy sign on the row the next log line will take, if it is free:
/// three dots, the `frame`th lit, and `label` after them.
pub fn show_sign(frame: usize, label: &str) {
    draw(|console, out| {
        let Some(row) = console.grid.spare_row() else {
            return console.hide_sign(out);
        };
        if console.sign != Some(row) {
            console.hide_sign(out);
            paint_frame(out, row, theme::look(Kind::of(&[])));
            console.painted[row] = UNKNOWN;
            console.sign = Some(row);
        }
        status::paint_sign(out.fb, &out.layout, row, frame, label);
    });
}

/// Takes the busy sign off the screen.
pub fn hide_sign() {
    draw(|console, out| console.hide_sign(out));
}

/// Paints the status bar's right side.
pub fn show_status(field: &status::Field) {
    draw(|_, out| status::paint_field(out.fb, &out.layout, field));
}

impl Console {
    fn hide_sign(&mut self, out: Output) {
        if let Some(row) = self.sign.take() {
            let input = self.grid.input_span();
            self.paint_row(out, row, input);
        }
    }
}

/// ASCII form feed: clears the screen. Printed like any other text, so it is
/// serialised with other output by the serial line lock.
pub const FORM_FEED: char = 12u8 as char;
