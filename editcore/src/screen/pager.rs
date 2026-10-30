//! The pager: a reply too long for the status line (a diff, `find` hits, a
//! build's log, the commands) shown a screenful at a time, with a selected row.
//! Enter or a Ctrl-click on a row that holds a source link opens that line; in a
//! file list it opens the file.

use super::input::Key;
use super::link::{self, Link};
use alloc::string::String;
use alloc::vec::Vec;

/// Most lines a pager holds; the rest is cut with a note.
pub const MAX_LINES: usize = 2000;
/// Most characters of a line it keeps.
pub const MAX_CHARS: usize = 400;

/// What the rows are, which decides what Enter does on one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Text; Enter follows the first source link in the row.
    Text,
    /// The session's files, one a row after the first.
    Files,
}

/// What a key or a click asks of the screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Act {
    None,
    /// Back to the text.
    Close,
    Follow(Link),
    /// Open this row's file (`Kind::Files`): the row's text.
    Open(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pager {
    pub title: String,
    pub kind: Kind,
    lines: Vec<String>,
    top: usize,
    selected: usize,
}

impl Pager {
    /// A pager over `text`, each line cut to [`MAX_CHARS`].
    pub fn new(title: &str, kind: Kind, text: &str) -> Self {
        let mut lines: Vec<String> = text
            .split('\n')
            .take(MAX_LINES)
            .map(|line| line.chars().take(MAX_CHARS).collect())
            .collect();
        if text.split('\n').nth(MAX_LINES).is_some() {
            lines.push(String::from("... more lines are not shown"));
        }
        let selected = usize::from(kind == Kind::Files && lines.len() > 1);
        Self {
            title: String::from(title),
            kind,
            lines,
            top: 0,
            selected,
        }
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn top(&self) -> usize {
        self.top
    }

    pub fn line(&self, n: usize) -> Option<&str> {
        self.lines.get(n).map(String::as_str)
    }

    /// The lines from `top`, at most `height`.
    pub fn page(&self, height: usize) -> impl Iterator<Item = (usize, &str)> {
        self.lines
            .iter()
            .enumerate()
            .skip(self.top)
            .take(height)
            .map(|(n, line)| (n, line.as_str()))
    }

    fn select(&mut self, row: usize, height: usize) {
        self.selected = row.min(self.lines.len().saturating_sub(1));
        if self.selected < self.top {
            self.top = self.selected;
        } else if self.selected >= self.top + height.max(1) {
            self.top = self.selected + 1 - height.max(1);
        }
    }

    /// What Enter on row `n` does.
    fn activate(&self, n: usize) -> Act {
        let Some(line) = self.lines.get(n) else {
            return Act::None;
        };
        match self.kind {
            Kind::Files if n > 0 => {
                // `* > path (N lines)`: the path is between the marks and the count.
                let path = line.get(3..).and_then(|rest| rest.rsplit_once(" ("));
                path.map_or(Act::None, |(path, _)| Act::Open(String::from(path)))
            }
            _ => link::scan(line)
                .into_iter()
                .next()
                .map_or(Act::None, Act::Follow),
        }
    }

    /// A key; `height` is the rows the pager shows.
    pub fn key(&mut self, key: Key, height: usize) -> Act {
        let page = height.saturating_sub(1).max(1);
        match key {
            Key::Esc | Key::Char('q') | Key::Ctrl('q') => return Act::Close,
            Key::Up | Key::Char('k') => self.select(self.selected.saturating_sub(1), height),
            Key::Down | Key::Char('j') => self.select(self.selected + 1, height),
            Key::PageUp => self.select(self.selected.saturating_sub(page), height),
            Key::PageDown | Key::Char(' ') => self.select(self.selected + page, height),
            Key::Home | Key::Char('g') => self.select(0, height),
            Key::End | Key::Char('G') => self.select(usize::MAX, height),
            Key::Enter => return self.activate(self.selected),
            _ => {}
        }
        Act::None
    }

    /// A click on page row `row` (from the top of the page), at character
    /// `col`; with Ctrl it follows the link under it, else it selects the row.
    pub fn click(&mut self, row: usize, col: usize, ctrl: bool, height: usize) -> Act {
        let n = self.top + row;
        if n >= self.lines.len() {
            return Act::None;
        }
        self.select(n, height);
        if !ctrl {
            return Act::None;
        }
        match self.kind {
            Kind::Files => self.activate(n),
            Kind::Text => link::at_column(&self.lines[n], col).map_or(Act::None, Act::Follow),
        }
    }
}
