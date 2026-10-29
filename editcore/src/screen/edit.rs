//! Keys and clicks: moving, typing into the draft line, the structural edits
//! (Enter and joining lines, which the screen shows at once and the service
//! then does as one `split` or `join`), and the viewport.

use super::{Cell, Dirty, Kind, Request, Screen, WINDOW};
use crate::session::MAX_INDENT;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Spaces a Tab types, and an opening brace indents the next line by.
pub(super) const INDENT: usize = 4;
/// What typed text may hold: the build rules allow nothing else.
pub(super) fn typable(c: char) -> bool {
    (' '..='~').contains(&c)
}

fn chars(text: &str) -> usize {
    text.chars().count()
}

fn byte_at(text: &str, col: usize) -> usize {
    text.char_indices()
        .nth(col)
        .map_or(text.len(), |(at, _)| at)
}

impl Screen {
    // The viewport.

    /// The width of the line numbers and the space after them.
    pub(crate) fn gutter(&self) -> usize {
        self.doc.len.max(1).to_string().len() + 1
    }

    /// Scrolls so the cursor line shows.
    pub(crate) fn follow(&mut self) {
        let last = self.doc.len.max(1);
        self.line = self.line.clamp(1, last);
        if self.line < self.top {
            self.top = self.line;
        } else if self.line >= self.top + self.rows {
            self.top = self.line + 1 - self.rows;
        }
        self.top = self.top.clamp(1, last);
        let len = self.line_len(self.line).unwrap_or(self.col);
        self.col = self.col.min(len);
    }

    /// Asks for the first lines on screen (and one either side) that are missing.
    pub(crate) fn fill(&mut self) {
        if self.doc.len == 0 || self.place.is_none() {
            return;
        }
        let first = self.top.saturating_sub(1).max(1);
        let last = (self.top + self.rows).min(self.doc.len);
        let Some(missing) = (first..=last).find(|n| !self.doc.lines.contains_key(n)) else {
            return;
        };
        let asked = self
            .queue
            .iter()
            .chain(self.flight.iter().map(|(_, p)| p))
            .any(|p| {
                matches!(p.kind, Kind::Meta)
                    || matches!(p.kind, Kind::Window(from, to) if (from..=to).contains(&missing))
            });
        if asked {
            return;
        }
        let to = (missing + WINDOW - 1).min(self.doc.len);
        self.push(
            Kind::Window(missing, to),
            Request::Cmd(format!("view {missing},{to}")),
        );
    }

    /// The characters in line `n` as the screen holds it.
    pub(crate) fn line_len(&self, n: usize) -> Option<usize> {
        self.text_of(n).map(chars)
    }

    pub(crate) fn text_of(&self, n: usize) -> Option<&str> {
        match &self.dirty {
            Some(d) if d.line == n => Some(&d.text),
            _ => self.doc.lines.get(&n).map(|c| c.text.as_str()),
        }
    }

    // Typing.

    /// Starts the draft line at the cursor, unless one is going; false when the
    /// line cannot be edited here.
    fn begin_dirty(&mut self) -> bool {
        if self.dirty.as_ref().is_some_and(|d| d.line == self.line) {
            return true;
        }
        self.commit();
        match self.doc.lines.get(&self.line) {
            Some(Cell { text, exact: true }) => {
                self.dirty = Some(Dirty {
                    line: self.line,
                    text: text.clone(),
                });
                true
            }
            Some(_) => {
                self.say(
                    "this line is too long or odd to edit here; use ^E and c",
                    true,
                );
                false
            }
            None if self.doc.len == 0 => {
                self.say("the file is empty; use ^E and i 1 <text>", true);
                false
            }
            None => {
                self.say("loading...", false);
                false
            }
        }
    }

    /// Sends the draft line, if the founder changed it.
    pub(crate) fn commit(&mut self) {
        let Some(dirty) = self.dirty.take() else {
            return;
        };
        let unchanged = self
            .doc
            .lines
            .get(&dirty.line)
            .is_some_and(|c| c.text == dirty.text);
        if unchanged {
            return;
        }
        let command = format!("c {} {}", dirty.line, dirty.text);
        self.doc.lines.insert(
            dirty.line,
            Cell {
                text: dirty.text,
                exact: true,
            },
        );
        self.sent_edit(command);
    }

    /// Queues an edit command the screen has already shown.
    fn sent_edit(&mut self, command: String) {
        self.doc.flags.changed = true;
        self.doc.flags.undo = true;
        self.epoch = self.epoch.wrapping_add(1);
        self.push(Kind::Edit(command.clone()), Request::Cmd(command));
        self.version += 1;
    }

    pub(super) fn insert_text(&mut self, text: &str) {
        if !self.begin_dirty() {
            return;
        }
        let col = self.col;
        if let Some(dirty) = self.dirty.as_mut() {
            let at = byte_at(&dirty.text, col);
            dirty.text.insert_str(at, text);
        }
        self.col += chars(text);
        self.goal = self.col;
        self.version += 1;
        self.follow();
    }

    fn remove_char(&mut self, col: usize) {
        if !self.begin_dirty() {
            return;
        }
        if let Some(dirty) = self.dirty.as_mut() {
            let at = byte_at(&dirty.text, col);
            dirty.text.remove(at);
        }
        self.col = col;
        self.goal = col;
        self.version += 1;
    }

    /// Enter: the rest of the line moves to a new one, indented like this one.
    pub(super) fn enter(&mut self) {
        if !self.begin_dirty() {
            return;
        }
        let n = self.line;
        let text = self
            .dirty
            .as_ref()
            .map(|d| d.text.clone())
            .unwrap_or_default();
        let col = self.col.min(chars(&text));
        let head = &text[..byte_at(&text, col)];
        let mut indent = head.chars().take_while(|&c| c == ' ').count();
        if head.trim_end().ends_with('{') {
            indent += INDENT;
        }
        let indent = indent.min(MAX_INDENT);
        self.commit();
        let Some(Cell { text, .. }) = self.doc.lines.get(&n).cloned() else {
            return;
        };
        let (head, tail) = text.split_at(byte_at(&text, col));
        let tail = format!("{}{tail}", " ".repeat(indent));
        let head = head.to_string();
        self.shift(n, 1);
        self.doc.lines.insert(
            n,
            Cell {
                text: head,
                exact: true,
            },
        );
        self.doc.lines.insert(
            n + 1,
            Cell {
                text: tail,
                exact: true,
            },
        );
        self.doc.len += 1;
        self.sent_edit(format!("split {n} {col} {indent}"));
        self.line = n + 1;
        self.col = indent;
        self.goal = indent;
        self.follow();
    }

    /// Joins line `n + 1` onto line `n`, both as the screen holds them.
    fn join(&mut self, n: usize) -> bool {
        self.commit();
        let exact = |s: &Self, n| s.doc.lines.get(&n).filter(|c| c.exact).cloned();
        let (Some(first), Some(second)) = (exact(self, n), exact(self, n + 1)) else {
            self.say("those lines are not loaded or cannot be edited here", true);
            return false;
        };
        let text = format!("{}{}", first.text, second.text);
        self.doc.lines.insert(n, Cell { text, exact: true });
        self.doc.lines.remove(&(n + 1));
        self.shift(n + 1, -1);
        self.doc.len = self.doc.len.saturating_sub(1);
        self.sent_edit(format!("join {n}"));
        true
    }

    /// Moves every loaded line after `after` by `by` places.
    fn shift(&mut self, after: usize, by: isize) {
        let moved: Vec<(usize, Cell)> = self
            .doc
            .lines
            .iter()
            .filter(|&(n, _)| *n > after)
            .map(|(&n, c)| (n, c.clone()))
            .collect();
        self.doc.lines.retain(|&n, _| n <= after);
        for (n, cell) in moved {
            if let Some(shifted) = n.checked_add_signed(by) {
                self.doc.lines.insert(shifted, cell);
            }
        }
    }

    pub(super) fn backspace(&mut self) {
        if self.col > 0 {
            self.remove_char(self.col - 1);
        } else if self.line > 1 {
            let n = self.line - 1;
            let Some(len) = self.line_len(n) else {
                return self.say("loading...", false);
            };
            if self.join(n) {
                self.line = n;
                self.col = len;
                self.goal = len;
            }
        }
        self.follow();
    }

    pub(super) fn delete(&mut self) {
        let len = self.line_len(self.line).unwrap_or(0);
        if self.col < len {
            self.remove_char(self.col);
        } else if self.line < self.doc.len {
            self.join(self.line);
        }
        self.follow();
    }

    // Moving.

    pub(super) fn go_line(&mut self, line: usize, col: usize) {
        if line != self.line {
            self.commit();
        }
        self.line = line.clamp(1, self.doc.len.max(1));
        self.col = col;
        self.follow();
    }

    pub(super) fn left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        } else if self.line > 1 {
            let n = self.line - 1;
            let len = self.line_len(n).unwrap_or(0);
            return self.go_line(n, len);
        }
        self.goal = self.col;
    }

    pub(super) fn right(&mut self) {
        if self.col < self.line_len(self.line).unwrap_or(0) {
            self.col += 1;
        } else if self.line < self.doc.len {
            return self.go_line(self.line + 1, 0);
        }
        self.goal = self.col;
    }
}
