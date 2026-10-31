//! The editor's window on `kai:ui` and `kai:code` (P4 task 11): a title, the
//! code view and a status line. The code view is where the file is; the title
//! names the version and the file, and the status line holds what the editor
//! last said, a build running, or the prompt being typed. The tree is rebuilt
//! whole from `editcore::screen`'s view after each event and diffed against the
//! last one, so only what changed goes to the host.
//!
//! What the screen holds that a view cannot show (a row longer than the code
//! view takes, more rows than it holds, a link that runs past its row) is cut
//! to the limits `uikit` holds an app to, so a put is never refused for size.

#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use comp_ui_bind::tree::{Entry, entry, label};
use editcore::screen::{Row, View};
use uikit::style::{Tone, TypeStyle};
use uikit::widget::{
    CodeRow, CodeSpan, CodeView, Insets, Layout, MAX_CODE_CELLS, MAX_CODE_ROWS, MAX_CODE_SPANS,
    NodeId, ROOT, Size, Widget,
};

pub use comp_ui_bind::tree::{Change, diff};

/// The file's name and version, above the text.
pub const TITLE: NodeId = 1;
/// The code view.
pub const CODE: NodeId = 2;
/// What the editor last said, or the prompt.
pub const STATUS: NodeId = 3;
/// The window's title, and its size when it opens, in logical px.
pub const WINDOW_TITLE: &str = "Editor";
pub const SIZE: (u16, u16) = (800, 560);
/// What the status line says while nothing else does: the keys that matter.
pub const HINT: &str =
    "^B build  ^Z undo  ^D diff  ^F find  ^G go to  ^O files  ^E command  ^H help";

fn bar() -> Layout {
    Layout {
        width: Size::Fill(1),
        height: Size::Fit,
        padding: Insets {
            top: 4,
            right: 8,
            bottom: 4,
            left: 8,
        },
        gap: 0,
    }
}

/// A span of the row's cells, within its text, or none.
fn span(text_cells: usize, (from, to): (usize, usize)) -> Option<CodeSpan> {
    let (from, to) = (from, to.min(text_cells));
    (from < to).then(|| CodeSpan {
        from: u16::try_from(from).unwrap_or(u16::MAX),
        to: u16::try_from(to).unwrap_or(u16::MAX),
        tone: Tone::Accent,
        link: true,
    })
}

fn code_row(row: &Row) -> CodeRow {
    let text: String = row.text.chars().take(MAX_CODE_CELLS).collect();
    let cells = text.chars().count();
    CodeRow {
        gutter: row.number.map(|n| format!("{n}")).unwrap_or_default(),
        spans: row
            .links
            .iter()
            .filter_map(|&link| span(cells, link))
            .take(MAX_CODE_SPANS)
            .collect(),
        text,
    }
}

/// The code view for `view`'s rows.
pub fn code(view: &View) -> CodeView {
    let rows: Vec<CodeRow> = view.rows.iter().take(MAX_CODE_ROWS).map(code_row).collect();
    let selected = view.rows.iter().position(|r| r.selected);
    CodeView {
        label: view.title.clone(),
        gutter_width: u16::try_from(view.gutter).unwrap_or(0),
        cursor: view.cursor.map(|(row, cell)| (row as u32, cell as u32)),
        selected: selected.map(|i| i as u32),
        rows,
    }
}

/// The status line's text and tone.
pub fn status(view: &View) -> (String, Tone) {
    if let Some((what, typed)) = &view.prompt {
        return (format!("{what}: {typed}_"), Tone::Accent);
    }
    if view.building {
        return (format!("building... {}", view.status), Tone::Accent);
    }
    if view.status.is_empty() {
        return (String::from(HINT), Tone::Dim);
    }
    let tone = if view.status_error {
        Tone::Error
    } else {
        Tone::Muted
    };
    (view.status.clone(), tone)
}

/// The whole tree for `view`, parents before children.
pub fn build(view: &View) -> Vec<Entry> {
    let (status, tone) = status(view);
    alloc::vec![
        entry(
            TITLE,
            ROOT,
            bar(),
            label(&view.title, TypeStyle::Mono, Tone::Muted)
        ),
        entry(CODE, ROOT, Layout::FILL, Widget::Code(code(view))),
        entry(STATUS, ROOT, bar(), label(&status, TypeStyle::Mono, tone)),
    ]
}
