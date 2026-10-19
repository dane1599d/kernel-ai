//! Framebuffer text console. It paints `termcore::grid`: every `log!` line goes into
//! the grid, and each change repaints only the rows the grid marks as damaged. The
//! terminal's prompt and typed text are the grid's input region, redrawn below each
//! log line, with the cursor shown as an inverted cell. With `screenlog` on the
//! command line, each repaint is copied to serial (`termcore::screenlog`).
//!
//! Every access takes the serial line lock and then the console lock, so a reader
//! never makes another CPU's line miss the screen.

use crate::serial::{self, Serial};
use core::fmt::{self, Write};
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use limine::framebuffer::Framebuffer;
use noto_sans_mono_bitmap::{FontWeight, RasterHeight, get_raster, get_raster_width};
use spin::Mutex;
use termcore::grid::{Grid, MAX_ROWS};

const FOREGROUND: u32 = 0x00E6_E6E6;
const BACKGROUND: u32 = 0x0010_2A43;
const WEIGHT: FontWeight = FontWeight::Regular;
const SIZE: RasterHeight = RasterHeight::Size16;
const CHAR_WIDTH: usize = get_raster_width(WEIGHT, SIZE);
const CHAR_HEIGHT: usize = SIZE as usize;
const REPLACEMENT: char = '\u{FFFD}';
/// Width and height of a text cell in pixels.
pub const CELL: (usize, usize) = (CHAR_WIDTH, CHAR_HEIGHT);
/// Cells the screen holds: 182 by 50 on CI's 1280x800, 274 by 67 on 1920x1080. The
/// grid is 128 KiB, more than a kernel stack, so it lives only in CONSOLE.
const CELLS: usize = 32 * 1024;

/// The screen's text.
pub type Screen = Grid<CELLS>;

#[derive(Clone, Copy)]
struct Fb {
    base: NonNull<u32>,
    /// Pixels per framebuffer row (pitch / 4).
    stride: usize,
}

struct Console {
    fb: Option<Fb>,
    grid: Screen,
    /// Per text row, how many cells from the left the last repaint may have left
    /// non-blank, so a repaint clears only those.
    painted: [usize; MAX_ROWS],
}

// SAFETY: the framebuffer pointer is only used under the CONSOLE mutex.
unsafe impl Send for Console {}

static CONSOLE: Mutex<Console> = Mutex::new(Console {
    fb: None,
    grid: Grid::new(),
    painted: [0; MAX_ROWS],
});
static SCREENLOG: AtomicBool = AtomicBool::new(false);
/// Text columns and rows of the framebuffer console; 0 when there is none.
static COLS: AtomicUsize = AtomicUsize::new(0);
static ROWS: AtomicUsize = AtomicUsize::new(0);
/// Framebuffer width and height in pixels; 0 when there is none.
static WIDTH: AtomicUsize = AtomicUsize::new(0);
static HEIGHT: AtomicUsize = AtomicUsize::new(0);

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
    screen.fill(0, 0, width, height, BACKGROUND);
    let (cols, rows) = serial::with_line(|| {
        let mut console = CONSOLE.lock();
        console.fb = Some(screen);
        console.painted = [0; MAX_ROWS];
        let size = console
            .grid
            .resize(width / CHAR_WIDTH, height / CHAR_HEIGHT);
        // The framebuffer is blank already.
        console.grid.take_damage();
        size
    });
    COLS.store(cols, Ordering::Relaxed);
    ROWS.store(rows, Ordering::Relaxed);
    WIDTH.store(width, Ordering::Relaxed);
    HEIGHT.store(height, Ordering::Relaxed);
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

/// Runs `f` on the screen's text, with no line drawn meanwhile. None without a
/// framebuffer console, or while a fault on this CPU holds it. `f` runs with
/// interrupts off and must not allocate.
pub fn with_grid<R>(f: impl FnOnce(&Screen) -> R) -> Option<R> {
    serial::with_line(|| {
        let console = CONSOLE.try_lock()?;
        console.fb?;
        Some(f(&console.grid))
    })
}

impl Console {
    /// Brings the framebuffer up to date with the grid, then copies the repainted
    /// rows to serial under `screenlog`. Interrupts are off, so it repaints only
    /// rows that changed, and in each only the cells that are or were non-blank.
    fn flush(&mut self) {
        let damage = self.grid.take_damage();
        let Some(fb) = self.fb else {
            return;
        };
        let (_, rows) = self.grid.size();
        if damage.scrolled > 0 && damage.scrolled < rows {
            fb.shift_up(damage.scrolled, rows);
            self.painted.copy_within(damage.scrolled..rows, 0);
        }
        for row in (0..rows).filter(|&row| damage.has(row)) {
            let width = self.grid.row_width(row);
            let cells = self.grid.row(row).iter().enumerate();
            for (col, &ch) in cells.take(width.max(self.painted[row])) {
                fb.cell(row, col, ch, false);
            }
            self.painted[row] = width;
        }
        if let Some((row, col)) = self.grid.cursor() {
            fb.cell(row, col, self.grid.row(row)[col], true);
            self.painted[row] = self.painted[row].max(col + 1);
        }
        if damage.rows != 0 && SCREENLOG.load(Ordering::Relaxed) {
            // Only `:clear`'s bare form feed leaves serial mid-line.
            if !serial::at_line_start() {
                let _ = Serial.write_str("\n");
            }
            let _ = termcore::screenlog::write(&mut Serial, &self.grid, &damage);
        }
    }
}

impl Fb {
    fn put(self, x: usize, y: usize, color: u32) {
        // SAFETY: callers keep x and y inside the framebuffer Limine mapped.
        unsafe {
            self.base
                .as_ptr()
                .add(y * self.stride + x)
                .write_volatile(color)
        };
    }

    fn fill(self, x0: usize, y0: usize, width: usize, height: usize, color: u32) {
        for y in y0..y0 + height {
            for x in x0..x0 + width {
                self.put(x, y, color);
            }
        }
    }

    /// Draws `ch` in a text cell; inverted swaps foreground and background.
    fn cell(self, row: usize, col: usize, ch: char, inverted: bool) {
        let (fg, bg) = if inverted {
            (BACKGROUND, FOREGROUND)
        } else {
            (FOREGROUND, BACKGROUND)
        };
        let (x0, y0) = (col * CHAR_WIDTH, row * CHAR_HEIGHT);
        let glyph = match ch {
            ' ' | '\0' => None,
            ch => get_raster(ch, WEIGHT, SIZE).or_else(|| get_raster(REPLACEMENT, WEIGHT, SIZE)),
        };
        let Some(glyph) = glyph else {
            return self.fill(x0, y0, CHAR_WIDTH, CHAR_HEIGHT, bg);
        };
        for (dy, line) in glyph.raster().iter().enumerate() {
            for (dx, &intensity) in line.iter().enumerate() {
                self.put(x0 + dx, y0 + dy, blend(intensity, fg, bg));
            }
        }
    }

    /// Moves text rows `n..rows` up to the top; the bottom `n` keep their pixels.
    fn shift_up(self, n: usize, rows: usize) {
        let row_pixels = CHAR_HEIGHT * self.stride;
        // SAFETY: source and destination lie inside the text area; ptr::copy handles overlap.
        unsafe {
            core::ptr::copy(
                self.base.as_ptr().add(n * row_pixels),
                self.base.as_ptr(),
                (rows - n) * row_pixels,
            );
        }
    }
}

/// ASCII form feed: clears the screen. Printed like any other text, so it is
/// serialised with other output by the serial line lock.
pub const FORM_FEED: char = 12u8 as char;

/// Mixes foreground into background by glyph coverage, per colour channel.
fn blend(intensity: u8, fg: u32, bg: u32) -> u32 {
    let mix = |shift: u32| {
        let fg = (fg >> shift) & 0xFF;
        let bg = (bg >> shift) & 0xFF;
        let value = (bg * (255 - u32::from(intensity)) + fg * u32::from(intensity)) / 255;
        value << shift
    };
    mix(16) | mix(8) | mix(0)
}
