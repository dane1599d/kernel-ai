//! The Files window on `kai:ui` (ADR 0005 section 4): the folder's trail, its
//! names, and for the selected one a preview, a field to move or rename it, and
//! the buttons. The tree is rebuilt whole from the model after each event and
//! [`diff`]ed against the last one, as Notes' is.
//!
//! Each button is bound to the row it runs, so the user reads, the agent sees and
//! the log says the same words (ADR 0005 section 4): Move, Trash and Restore are
//! the OS's `fs_move`, `fs_trash` and `fs_restore`, and Up and Open are the
//! app's own `files_open`. Names and file text are data and are drawn as text.

#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use comp_files::names::Kind;
use comp_files::{Files, Os, Preview, Status, shown, tame};
pub use comp_ui_bind::tree::{Change, Entry, diff};
use comp_ui_bind::tree::{column, entry, label, row, wide};
use uikit::style::{Tone, TypeStyle};
use uikit::widget::{Axis, Layout, NodeId, ROOT, Size, TextField, UiEvent, Value, Widget};

pub const COLUMN: NodeId = 1;
pub const BAR: NodeId = 2;
pub const UP: NodeId = 3;
pub const TRAIL: NodeId = 4;
pub const REFRESH: NodeId = 5;
pub const SPLIT: NodeId = 6;
pub const LIST: NodeId = 7;
pub const DETAIL: NodeId = 8;
pub const TITLE: NodeId = 9;
pub const SCROLL: NodeId = 10;
pub const BODY: NodeId = 11;
pub const NAME: NodeId = 12;
pub const BUTTONS: NodeId = 13;
pub const OPEN: NodeId = 14;
pub const MOVE: NodeId = 15;
pub const TRASH: NodeId = 16;
pub const RESTORE: NodeId = 17;
pub const STATUS: NodeId = 18;
/// List items are `ITEM + position` among the folder's names.
pub const ITEM: NodeId = 1000;
/// Preview lines are `LINE + position`.
pub const LINE: NodeId = 2000;
/// Longest a list item or a preview line gets, in characters.
pub const ITEM_CHARS: usize = 40;
pub const LINE_CHARS: usize = 72;
/// The window's size when it opens, in logical px.
pub const SIZE: (u16, u16) = (720, 460);
/// The window's title.
pub const TITLE_TEXT: &str = "Files";

/// The trail above the list: `Home / docs / old`.
fn trail(dir: &str) -> String {
    let mut out = String::from("Home");
    if !dir.is_empty() {
        for name in dir.split('/') {
            out.push_str(" / ");
            out.push_str(&tame(name, ITEM_CHARS));
        }
    }
    out
}

fn button(text: &str, action: Option<&str>) -> Widget {
    Widget::Button {
        text: String::from(text),
        action: action.map(String::from),
    }
}

/// The preview's lines, with the tone each is drawn in.
fn preview_lines(preview: &Preview) -> Vec<(String, Tone)> {
    let line = |text: &str| (tame(text, LINE_CHARS), Tone::Normal);
    match preview {
        Preview::None => alloc::vec![(String::from("Nothing selected"), Tone::Dim)],
        Preview::Loading => alloc::vec![(String::from("Loading…"), Tone::Dim)],
        Preview::Failed(why) => alloc::vec![(tame(why, LINE_CHARS), Tone::Error)],
        Preview::Text { text, cut } => {
            let mut out: Vec<_> = text.lines().map(line).collect();
            if out.is_empty() {
                out.push((String::from("(empty)"), Tone::Dim));
            }
            if *cut {
                out.push((String::from("…"), Tone::Dim));
            }
            out
        }
        Preview::Folder { names, count } => {
            let mut out = alloc::vec![(
                match count {
                    0 => String::from("Empty folder"),
                    1 => String::from("1 item"),
                    n => format!("{n} items"),
                },
                Tone::Muted,
            )];
            out.extend(names.iter().map(|n| line(n)));
            if *count > names.len() {
                out.push((String::from("…"), Tone::Dim));
            }
            out
        }
    }
}

/// What the status line says: the last outcome, or the folder's count.
fn status(files: &Files) -> (String, Tone) {
    match files.status() {
        Status::Failed(why) => return (why.clone(), Tone::Error),
        Status::Done(what) => return (what.clone(), Tone::Ok),
        Status::Ready => {}
    }
    if files.busy() {
        return (String::from("Working…"), Tone::Muted);
    }
    let folders = files.items().iter().filter(|i| i.kind == Kind::Dir).count();
    let (skipped, more) = files.left_out();
    let mut text = format!("{folders} folders, {} files", files.items().len() - folders);
    if more {
        text.push_str(", and maybe more");
    }
    if skipped > 0 {
        text.push_str(&format!(", {skipped} names left out"));
    }
    (text, Tone::Muted)
}

/// The whole tree for this model, parents before children.
pub fn build(files: &Files) -> Vec<Entry> {
    let items = files.items();
    let mut tree = alloc::vec![
        entry(
            COLUMN,
            ROOT,
            column(12, 8),
            Widget::Stack { axis: Axis::Column }
        ),
        entry(BAR, COLUMN, wide(), row()),
        entry(UP, BAR, Layout::FIT, button("Up", Some("files_open"))),
        entry(
            TRAIL,
            BAR,
            wide(),
            label(&trail(files.dir()), TypeStyle::Mono, Tone::Muted),
        ),
        entry(REFRESH, BAR, Layout::FIT, button("Refresh", None)),
        entry(
            SPLIT,
            COLUMN,
            Layout::FILL,
            Widget::Split {
                axis: Axis::Row,
                first: 300,
            },
        ),
        entry(
            LIST,
            SPLIT,
            Layout::FILL,
            Widget::ItemList {
                label: String::from("Files"),
                selected: files.selected_index().map(|i| i as u32),
            },
        ),
    ];
    for (i, item) in items.iter().enumerate() {
        let name = tame(&item.name, ITEM_CHARS);
        let text = match item.kind {
            Kind::Dir => format!("{name}/"),
            Kind::File => name,
        };
        let tone = match item.kind {
            Kind::Dir => Tone::Accent,
            Kind::File => Tone::Normal,
        };
        tree.push(entry(
            ITEM + i as u32,
            LIST,
            wide(),
            label(&text, TypeStyle::Body, tone),
        ));
    }
    let title = match (files.selected(), files.selected_kind()) {
        (Some(name), Some(Kind::Dir)) => format!("{}/", tame(name, ITEM_CHARS)),
        (Some(name), _) => tame(name, ITEM_CHARS),
        (None, _) => shown(files.dir()),
    };
    tree.extend([
        entry(
            DETAIL,
            SPLIT,
            column(12, 8),
            Widget::Stack { axis: Axis::Column },
        ),
        entry(
            TITLE,
            DETAIL,
            wide(),
            label(&title, TypeStyle::Heading, Tone::Normal),
        ),
        entry(
            SCROLL,
            DETAIL,
            Layout::FILL,
            Widget::Scroll { axis: Axis::Column },
        ),
        entry(
            BODY,
            SCROLL,
            Layout {
                width: Size::Fill(1),
                ..Layout::FIT
            },
            Widget::Stack { axis: Axis::Column },
        ),
    ]);
    for (i, (text, tone)) in preview_lines(files.preview()).into_iter().enumerate() {
        tree.push(entry(
            LINE + i as u32,
            BODY,
            wide(),
            label(&text, TypeStyle::Mono, tone),
        ));
    }
    tree.extend([
        entry(
            NAME,
            DETAIL,
            wide(),
            Widget::TextLine(TextField {
                label: String::from("Rename to"),
                value: String::from(files.rename_text()),
                placeholder: String::from("A new name"),
                max_len: comp_files::names::MAX_PATH as u32,
            }),
        ),
        entry(BUTTONS, DETAIL, wide(), row()),
        entry(
            OPEN,
            BUTTONS,
            Layout::FIT,
            button("Open", Some("files_open")),
        ),
        entry(
            MOVE,
            BUTTONS,
            Layout::FIT,
            button("Rename", Some("fs_move")),
        ),
        entry(
            TRASH,
            BUTTONS,
            Layout::FIT,
            button("Trash", Some("fs_trash")),
        ),
        entry(
            RESTORE,
            BUTTONS,
            Layout::FIT,
            button("Restore", Some("fs_restore")),
        ),
    ]);
    let (text, tone) = status(files);
    tree.push(entry(
        STATUS,
        COLUMN,
        wide(),
        label(&text, TypeStyle::Small, tone),
    ));
    tree
}

/// Handles one event from the window; whether the tree may have changed.
pub fn on_ui(files: &mut Files, os: &mut dyn Os, e: &UiEvent) -> bool {
    match e {
        UiEvent::Pressed(UP) => files.up(os),
        UiEvent::Pressed(REFRESH) => files.refresh(os),
        UiEvent::Pressed(OPEN) => files.open_selected(os),
        UiEvent::Pressed(MOVE) | UiEvent::Submitted(NAME) => files.move_selected(os),
        UiEvent::Pressed(TRASH) => files.trash_selected(os),
        UiEvent::Pressed(RESTORE) => files.restore_last(os),
        UiEvent::Changed(NAME, Value::Text(text)) => files.set_rename(text),
        UiEvent::Changed(LIST, Value::Index(i)) => {
            let Some(name) = files.items().get(*i as usize).map(|i| i.name.clone()) else {
                return false;
            };
            // The name came from the list, so it is there.
            let _ = files.select(os, &name);
        }
        _ => return false,
    }
    true
}
