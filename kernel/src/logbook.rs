//! The log book: the latest lines printed with `quiet!`, which go to serial but not
//! the screen (a program's audit trail, the scheduler's notes). The terminal's
//! `:log` shows them. Lines live in fixed slots, so recording one allocates
//! nothing and can run under the serial line lock.

use crate::log;
use core::fmt::{self, Write};
use spin::Mutex;

/// Lines kept; a new one takes the oldest one's slot.
const LINES: usize = 64;
/// Bytes kept of each line; the rest is cut at a character boundary.
const LINE_BYTES: usize = 160;

struct Book {
    text: [[u8; LINE_BYTES]; LINES],
    len: [usize; LINES],
    /// Lines recorded so far; the newest is in slot `(count - 1) % LINES`.
    count: usize,
}

static BOOK: Mutex<Book> = Mutex::new(Book {
    text: [[0; LINE_BYTES]; LINES],
    len: [0; LINES],
    count: 0,
});

/// Fills one slot, without its newline, up to the last character that fits.
struct Slot<'a> {
    buf: &'a mut [u8; LINE_BYTES],
    len: usize,
    full: bool,
}

impl Write for Slot<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for ch in s.chars().filter(|&ch| ch != '\n' && ch != '\r') {
            let mut utf8 = [0; 4];
            let bytes = ch.encode_utf8(&mut utf8).as_bytes();
            let end = self.len + bytes.len();
            if self.full || end > LINE_BYTES {
                self.full = true;
                break;
            }
            self.buf[self.len..end].copy_from_slice(bytes);
            self.len = end;
        }
        Ok(())
    }
}

/// Keeps the line `args` formats. `serial::_print_quiet` calls it under the line
/// lock; a book busy on this CPU (a fault while recording) skips the line.
pub fn record(args: fmt::Arguments) {
    let Some(mut book) = BOOK.try_lock() else {
        return;
    };
    let book = &mut *book;
    let at = book.count % LINES;
    let mut slot = Slot {
        buf: &mut book.text[at],
        len: 0,
        full: false,
    };
    let _ = slot.write_fmt(args);
    let len = slot.len;
    book.len[at] = len;
    book.count += 1;
}

/// Prints the latest `n` lines, oldest first. Each is copied out before it is
/// printed, so printing never holds the book.
pub fn show(n: usize) {
    let (first, end) = {
        let book = BOOK.lock();
        let shown = n.min(book.count).min(LINES);
        (book.count - shown, book.count)
    };
    if first == end {
        log!("log: nothing yet");
        return;
    }
    for index in first..end {
        let mut line = [0u8; LINE_BYTES];
        let len = {
            let book = BOOK.lock();
            // A slot overwritten since shows its newer line, still a log line.
            let at = index % LINES;
            let len = book.len[at];
            line[..len].copy_from_slice(&book.text[at][..len]);
            len
        };
        log!("{}", core::str::from_utf8(&line[..len]).unwrap_or(""));
    }
}
