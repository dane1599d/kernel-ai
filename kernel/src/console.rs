//! Framebuffer text console. Every `log!` line is mirrored here.

use core::fmt::{self, Write};
use core::sync::atomic::{AtomicUsize, Ordering};
use limine::framebuffer::Framebuffer;
use noto_sans_mono_bitmap::{FontWeight, RasterHeight, get_raster, get_raster_width};
use spin::Mutex;

const FOREGROUND: u32 = 0x00E6_E6E6;
const BACKGROUND: u32 = 0x0010_2A43;
const WEIGHT: FontWeight = FontWeight::Regular;
const SIZE: RasterHeight = RasterHeight::Size16;
const CHAR_WIDTH: usize = get_raster_width(WEIGHT, SIZE);
const CHAR_HEIGHT: usize = SIZE as usize;
const REPLACEMENT: char = '\u{FFFD}';
/// Width and height of a text cell in pixels.
pub const CELL: (usize, usize) = (CHAR_WIDTH, CHAR_HEIGHT);

struct Console {
    base: *mut u32,
    /// Pixels per framebuffer row (pitch / 4).
    stride: usize,
    cols: usize,
    rows: usize,
    col: usize,
    row: usize,
}

// SAFETY: the framebuffer pointer is only used under the CONSOLE mutex.
unsafe impl Send for Console {}

static CONSOLE: Mutex<Option<Console>> = Mutex::new(None);
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
    if fb.bpp != 32 {
        return false;
    }
    let mut console = Console {
        base: fb.address().cast::<u32>(),
        stride: (fb.pitch / 4) as usize,
        cols: fb.width as usize / CHAR_WIDTH,
        rows: fb.height as usize / CHAR_HEIGHT,
        col: 0,
        row: 0,
    };
    console.clear(fb.width as usize, fb.height as usize);
    COLS.store(console.cols, Ordering::Relaxed);
    ROWS.store(console.rows, Ordering::Relaxed);
    WIDTH.store(fb.width as usize, Ordering::Relaxed);
    HEIGHT.store(fb.height as usize, Ordering::Relaxed);
    *CONSOLE.lock() = Some(console);
    true
}

/// Writes to the console if it is free. A fault raised while the console is
/// mid-write skips the console instead of deadlocking; serial still gets the text.
pub fn print(args: fmt::Arguments) {
    if let Some(mut guard) = CONSOLE.try_lock()
        && let Some(console) = guard.as_mut()
    {
        let _ = console.write_fmt(args);
    }
}

impl Console {
    fn put(&mut self, x: usize, y: usize, color: u32) {
        // SAFETY: callers keep x and y inside the framebuffer Limine mapped.
        unsafe { self.base.add(y * self.stride + x).write_volatile(color) };
    }

    fn clear(&mut self, width: usize, height: usize) {
        for y in 0..height {
            for x in 0..width {
                self.put(x, y, BACKGROUND);
            }
        }
    }

    fn clear_text_row(&mut self, row: usize) {
        for y in row * CHAR_HEIGHT..(row + 1) * CHAR_HEIGHT {
            for x in 0..self.cols * CHAR_WIDTH {
                self.put(x, y, BACKGROUND);
            }
        }
    }

    fn newline(&mut self) {
        self.col = 0;
        if self.row + 1 < self.rows {
            self.row += 1;
            return;
        }
        let row_pixels = CHAR_HEIGHT * self.stride;
        // SAFETY: source and destination lie inside the text area; ptr::copy handles overlap.
        unsafe {
            core::ptr::copy(
                self.base.add(row_pixels),
                self.base,
                (self.rows - 1) * row_pixels,
            );
        }
        self.clear_text_row(self.rows - 1);
    }

    fn draw(&mut self, ch: char) {
        let Some(glyph) =
            get_raster(ch, WEIGHT, SIZE).or_else(|| get_raster(REPLACEMENT, WEIGHT, SIZE))
        else {
            return;
        };
        let (x0, y0) = (self.col * CHAR_WIDTH, self.row * CHAR_HEIGHT);
        for (dy, line) in glyph.raster().iter().enumerate() {
            for (dx, &intensity) in line.iter().enumerate() {
                self.put(x0 + dx, y0 + dy, blend(intensity));
            }
        }
    }
}

/// ASCII backspace: moves the cursor back one cell, to the end of the row above
/// if it is at the start of a row. Echo "\x08 \x08" to erase a character.
pub const BACKSPACE: char = 8u8 as char;
/// ASCII form feed: clears the screen. Printed like any other text, so it is
/// serialised with other output by the serial line lock.
pub const FORM_FEED: char = 12u8 as char;

impl Console {
    fn clear_screen(&mut self) {
        for row in 0..self.rows {
            self.clear_text_row(row);
        }
        self.col = 0;
        self.row = 0;
    }

    fn back_one(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        } else if self.row > 0 {
            // Wrapped input continues on the next row, so it ends in the last column.
            self.row -= 1;
            self.col = self.cols.saturating_sub(1);
        }
    }
}

impl Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for ch in s.chars() {
            match ch {
                '\n' => self.newline(),
                '\r' => self.col = 0,
                FORM_FEED => self.clear_screen(),
                BACKSPACE => self.back_one(),
                _ => {
                    if self.col >= self.cols {
                        self.newline();
                    }
                    self.draw(ch);
                    self.col += 1;
                }
            }
        }
        Ok(())
    }
}

/// Mixes foreground into background by glyph coverage, per colour channel.
fn blend(intensity: u8) -> u32 {
    let mix = |shift: u32| {
        let fg = (FOREGROUND >> shift) & 0xFF;
        let bg = (BACKGROUND >> shift) & 0xFF;
        let value = (bg * (255 - u32::from(intensity)) + fg * u32::from(intensity)) / 255;
        value << shift
    };
    mix(16) | mix(8) | mix(0)
}
