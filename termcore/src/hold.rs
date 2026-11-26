//! Text kept off the console while it asks a question (bug hunt 6, H6-10).
//!
//! The terminal opens the hold before it prints the question, so nothing
//! another thread logs meanwhile lands among the question's rows or scrolls
//! its top away; the asking thread's own lines pass. When the answer is in,
//! the hold closes and gives back what it kept, to print then. It holds
//! `BYTES` bytes inline and allocates nothing, since the console takes it
//! under the line lock with interrupts off; text past that is counted, not
//! kept (the serial line and `:log` have it all the same).

use core::fmt;

/// What a closed hold gives back: the text it kept, in whole lines, and how
/// many lines did not fit.
#[derive(Debug, PartialEq, Eq)]
pub struct Released<'a> {
    pub text: &'a str,
    pub lost: usize,
}

/// The hold: closed, or open for one asking thread.
pub struct Hold<const BYTES: usize> {
    /// The asking thread, whose own text passes; None while closed.
    owner: Option<usize>,
    text: [u8; BYTES],
    len: usize,
    /// Set once a piece did not fit: later text is counted, not kept, so the
    /// kept text stays in order.
    full: bool,
    lost: usize,
}

impl<const BYTES: usize> Default for Hold<BYTES> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const BYTES: usize> Hold<BYTES> {
    /// A closed, empty hold.
    pub const fn new() -> Self {
        Hold {
            owner: None,
            text: [0; BYTES],
            len: 0,
            full: false,
            lost: 0,
        }
    }

    /// Opens the hold for `owner`, the thread about to ask. Text kept by an
    /// earlier hold and not taken is dropped.
    pub fn open(&mut self, owner: usize) {
        self.owner = Some(owner);
        self.len = 0;
        self.full = false;
        self.lost = 0;
    }

    pub fn is_open(&self) -> bool {
        self.owner.is_some()
    }

    /// Whether text has stopped being kept: from now on it is only counted.
    pub fn is_full(&self) -> bool {
        self.full
    }

    /// Whether text from `writer` is kept rather than shown now.
    pub fn holds(&self, writer: usize) -> bool {
        self.owner.is_some_and(|owner| owner != writer)
    }

    /// Keeps `text`, or counts its lines as lost once the hold is full.
    pub fn keep(&mut self, text: &str) {
        if !self.full && self.len + text.len() <= BYTES {
            self.text[self.len..self.len + text.len()].copy_from_slice(text.as_bytes());
            self.len += text.len();
            return;
        }
        if !self.full {
            // Back to the last whole line: the rest of this one is lost with
            // the text after it, and its newline is counted when it comes.
            self.full = true;
            let kept = &self.text[..self.len];
            self.len = kept
                .iter()
                .rposition(|&b| b == b'\n')
                .map_or(0, |at| at + 1);
        }
        self.lost += text.bytes().filter(|&b| b == b'\n').count();
    }

    /// Closes the hold and gives back what it kept. The text is whole lines:
    /// a line still open when it closed is ended.
    pub fn close(&mut self) -> Released<'_> {
        self.owner = None;
        if self.len > 0 && self.text[self.len - 1] != b'\n' {
            if self.len < BYTES {
                self.text[self.len] = b'\n';
                self.len += 1;
            } else {
                let kept = &self.text[..self.len];
                self.len = kept
                    .iter()
                    .rposition(|&b| b == b'\n')
                    .map_or(0, |at| at + 1);
                self.lost += 1;
            }
        }
        let len = core::mem::take(&mut self.len);
        let lost = core::mem::take(&mut self.lost);
        self.full = false;
        // Only whole `&str` pieces and newlines were copied in, and the cuts
        // above fall just after a newline, so the bytes are UTF-8.
        let text = core::str::from_utf8(&self.text[..len]).unwrap_or("");
        Released { text, lost }
    }
}

impl<const BYTES: usize> fmt::Write for Hold<BYTES> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.keep(s);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
