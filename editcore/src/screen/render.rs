//! What to draw: rows of text with their line numbers, the status line, the
//! prompt and where the cursor is, in cells. The front end (a uikit window, the
//! kernel's console) only paints them.

use super::pager::Pager;
use super::{Mode, PromptKind, Screen, link};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// A tab stop's width in cells.
pub const TAB: usize = 4;

/// One row of the text area.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Row {
    /// The line number, none for a pager row or past the end of the file.
    pub number: Option<usize>,
    /// The visible part of the text, tabs spread to spaces.
    pub text: String,
    /// The pager's selected row.
    pub selected: bool,
    /// Where source links are in `text`, as cell ranges.
    pub links: Vec<(usize, usize)>,
}

/// Everything a front end draws.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct View {
    /// `notes 2: lib.rs`, with `*` once the text differs from the version.
    pub title: String,
    pub rows: Vec<Row>,
    /// Width of the line numbers and the gap after them; 0 in a pager.
    pub gutter: usize,
    pub status: String,
    pub status_error: bool,
    pub building: bool,
    /// The cursor's row and cell in the text area, after the gutter.
    pub cursor: Option<(usize, usize)>,
    /// A prompt's label and what was typed, when one is open.
    pub prompt: Option<(&'static str, String)>,
    /// What the keys do now: `edit`, `pager` or `prompt`.
    pub mode: &'static str,
}

/// `text` with each tab spread to the next tab stop.
fn spread(text: &str) -> Vec<char> {
    let mut cells = Vec::new();
    for c in text.chars() {
        if c == '\t' {
            let stop = TAB - cells.len() % TAB;
            cells.extend(core::iter::repeat_n(' ', stop));
        } else {
            cells.push(c);
        }
    }
    cells
}

/// The cell character `col` of `text` starts at.
pub fn cell_of(text: &str, col: usize) -> usize {
    spread(&text.chars().take(col).collect::<String>()).len()
}

/// The character a click on `cell` lands on: the one that covers it, or the end.
pub fn col_of_cell(text: &str, cell: usize) -> usize {
    let mut at = 0;
    for (col, c) in text.chars().enumerate() {
        let width = if c == '\t' { TAB - at % TAB } else { 1 };
        if cell < at + width {
            return col;
        }
        at += width;
    }
    text.chars().count()
}

/// Spreads `text`, drops the first `left` cells and keeps `width` of the rest.
fn window(text: &str, left: usize, width: usize) -> String {
    spread(text).into_iter().skip(left).take(width).collect()
}

/// Where the links of `text` are, as cell ranges inside the window.
fn link_cells(text: &str, left: usize, width: usize) -> Vec<(usize, usize)> {
    let char_at = |byte: usize| text[..byte].chars().count();
    link::scan(text)
        .iter()
        .filter_map(|l| {
            let (from, to) = (
                cell_of(text, char_at(l.start)),
                cell_of(text, char_at(l.end)),
            );
            let (from, to) = (from.max(left), to.min(left + width));
            (from < to).then_some((from - left, to - left))
        })
        .collect()
}

impl Screen {
    /// The cell column the text area starts showing at, so the cursor shows.
    fn scroll_left(&self) -> usize {
        let text = self.text_of(self.line).unwrap_or("");
        let width = self.cols.saturating_sub(self.gutter()).max(1);
        let at = cell_of(text, self.col);
        (at + 1).saturating_sub(width)
    }

    fn title(&self) -> String {
        match &self.place {
            Some((component, label)) => {
                let mark = if self.doc.flags.changed { " *" } else { "" };
                format!("{component} {label}: {}{mark}", self.doc.path)
            }
            None => String::from("editor"),
        }
    }

    pub fn view(&self) -> View {
        let mut view = View {
            title: self.title(),
            status: self.status.text.clone(),
            status_error: self.status.error,
            building: self.building,
            ..View::default()
        };
        match &self.mode {
            Mode::Pager(pager) => self.pager_rows(&mut view, pager),
            Mode::Edit => self.edit_rows(&mut view, "edit"),
            Mode::Prompt(kind, text) => {
                self.edit_rows(&mut view, "prompt");
                let label = match kind {
                    PromptKind::Find => "find",
                    PromptKind::Goto => "go to line",
                    PromptKind::Command => "command",
                };
                view.prompt = Some((label, text.clone()));
                view.cursor = None;
            }
        }
        view
    }

    fn edit_rows(&self, view: &mut View, mode: &'static str) {
        let gutter = self.gutter();
        let width = self.cols.saturating_sub(gutter).max(1);
        let left = self.scroll_left();
        view.mode = mode;
        view.gutter = gutter;
        for row in 0..self.rows {
            let n = self.top + row;
            let text = if n <= self.doc.len {
                self.text_of(n)
            } else {
                None
            };
            view.rows.push(match text {
                Some(text) => Row {
                    number: Some(n),
                    text: window(text, left, width),
                    selected: n == self.line,
                    links: link_cells(text, left, width),
                },
                // A line not loaded yet, or past the end.
                None => Row {
                    number: (n <= self.doc.len).then_some(n),
                    ..Row::default()
                },
            });
        }
        let at = cell_of(self.text_of(self.line).unwrap_or(""), self.col);
        if self.line >= self.top && self.line < self.top + self.rows {
            view.cursor = Some((self.line - self.top, at.saturating_sub(left)));
        }
    }

    fn pager_rows(&self, view: &mut View, pager: &Pager) {
        view.mode = "pager";
        view.title = format!("{} - {}", self.title(), pager.title);
        let width = self.cols.max(1);
        for (n, line) in pager.page(self.rows) {
            view.rows.push(Row {
                number: None,
                text: window(line, 0, width),
                selected: n == pager.selected(),
                links: link_cells(line, 0, width),
            });
        }
    }
}
