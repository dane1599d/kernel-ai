//! Notes' window on `kai:ui` (ADR 0005 section 4): a search field, the list of
//! notes and the editor, built as a `uikit` tree. The tree is rebuilt whole from
//! the state after each event and [`diff`]ed against the last one, so only what
//! changed goes to the host. The actions in `kai.actions` are unchanged: "New
//! note" is bound to `notes_add`, so the agent sees the same words and tier.
//! It is its own crate so the source the edit loop changes, and the bundle it
//! builds in the OS, stay as they are.

#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use comp_notes::{Actions, Notes, state};
use uikit::style::{Tone, TypeStyle};
use uikit::widget::{Axis, Insets, Layout, NodeId, ROOT, Size, TextField, UiEvent, Value, Widget};

pub const SPLIT: NodeId = 1;
pub const SIDE: NodeId = 2;
pub const SEARCH: NodeId = 3;
pub const NEW: NodeId = 4;
pub const LIST: NodeId = 5;
pub const MAIN: NodeId = 6;
pub const EDITOR: NodeId = 7;
pub const STATUS: NodeId = 8;
/// Removes the note in the editor, on its second press; there only while one is
/// open.
pub const DELETE: NodeId = 9;
/// List items are `ITEM + position` among the shown notes.
pub const ITEM: NodeId = 1000;
/// Longest a list item's label gets, in characters.
pub const ITEM_CHARS: usize = 40;
/// The window's size when it opens, in logical px.
pub const SIZE: (u16, u16) = (640, 420);
/// Most bytes the editor takes: one note's room in the state.
pub const EDITOR_MAX: u32 = 2000;

/// What the window shows that the state does not hold.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct View {
    /// The search field's text.
    pub search: String,
    /// The note in the editor, by id.
    pub selected: Option<u32>,
    /// The note whose Delete was pressed once, and when (the call's clock, in ms)
    /// that button was last pressed. A press after a pause removes it; any other
    /// event takes the question back.
    pub delete_asked: Option<(u32, u64)>,
}

/// One node of the tree: its id, its parent, its layout and its widget.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: NodeId,
    pub parent: NodeId,
    pub layout: Layout,
    pub widget: Widget,
}

/// A change to send through `kai:ui/view`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    Put(Entry),
    Remove(NodeId),
}

/// Whether `note` matches the search: every word of it, ignoring ASCII case.
fn matches(text: &str, search: &str) -> bool {
    let text = text.to_ascii_lowercase();
    search
        .split_whitespace()
        .all(|w| text.contains(&w.to_ascii_lowercase()))
}

/// The notes the list shows, in order.
pub fn shown<'a>(notes: &'a Notes, view: &View) -> Vec<&'a state::Note> {
    notes
        .state
        .notes
        .iter()
        .filter(|n| matches(&n.text, &view.search))
        .collect()
}

/// A list item's label: the note's first line, cut to [`ITEM_CHARS`].
pub fn title(text: &str) -> String {
    comp_notes::first_line(text, ITEM_CHARS)
}

/// `n` and `noun`, plural but for one: "1 note", "2 notes".
fn counted(n: u32, noun: &str) -> String {
    match n {
        1 => format!("1 {noun}"),
        _ => format!("{n} {noun}s"),
    }
}

fn entry(id: NodeId, parent: NodeId, layout: Layout, widget: Widget) -> Entry {
    Entry {
        id,
        parent,
        layout,
        widget,
    }
}

fn column(pad: u16, gap: u16) -> Layout {
    Layout {
        padding: Insets::all(pad),
        gap,
        ..Layout::FILL
    }
}

fn wide() -> Layout {
    Layout {
        width: Size::Fill(1),
        ..Layout::FIT
    }
}

fn label(text: String, style: TypeStyle, tone: Tone) -> Widget {
    Widget::Label { text, style, tone }
}

/// The whole tree for this state and view, parents before children.
pub fn build(notes: &Notes, view: &View) -> Vec<Entry> {
    let shown = shown(notes, view);
    let selected = view
        .selected
        .and_then(|id| shown.iter().position(|n| n.id == id))
        .map(|i| i as u32);
    let mut tree = alloc::vec![
        entry(
            SPLIT,
            ROOT,
            Layout::FILL,
            Widget::Split {
                axis: Axis::Row,
                first: 340
            }
        ),
        entry(
            SIDE,
            SPLIT,
            column(12, 8),
            Widget::Stack { axis: Axis::Column }
        ),
        entry(
            SEARCH,
            SIDE,
            wide(),
            Widget::TextLine(TextField {
                label: String::from("Search"),
                value: view.search.clone(),
                placeholder: String::from("Find a word"),
                max_len: 60,
            }),
        ),
        entry(
            NEW,
            SIDE,
            wide(),
            Widget::Button {
                text: String::from("New note"),
                action: Some(String::from("notes_add")),
            },
        ),
        entry(
            LIST,
            SIDE,
            Layout::FILL,
            Widget::ItemList {
                label: String::from("Notes"),
                selected,
            },
        ),
    ];
    for (i, n) in shown.iter().enumerate() {
        let item = label(title(&n.text), TypeStyle::Body, Tone::Normal);
        tree.push(entry(ITEM + i as u32, LIST, wide(), item));
    }
    tree.push(entry(
        MAIN,
        SPLIT,
        column(12, 8),
        Widget::Stack { axis: Axis::Column },
    ));
    let note = view
        .selected
        .and_then(|id| notes.state.notes.iter().find(|n| n.id == id));
    let open = note.is_some();
    tree.push(match note {
        Some(n) => entry(
            EDITOR,
            MAIN,
            Layout::FILL,
            Widget::TextArea(TextField {
                label: String::from("Note"),
                value: n.text.clone(),
                placeholder: String::from("Write here"),
                max_len: EDITOR_MAX,
            }),
        ),
        None => entry(
            EDITOR,
            MAIN,
            Layout::FILL,
            label(String::from("No note open"), TypeStyle::Body, Tone::Dim),
        ),
    });
    if open {
        let asked = view
            .delete_asked
            .map(|(id, _)| id)
            .is_some_and(|id| Some(id) == view.selected);
        tree.push(entry(
            DELETE,
            MAIN,
            wide(),
            Widget::Button {
                text: String::from(if asked {
                    "Press again to delete"
                } else {
                    "Delete note"
                }),
                action: None,
            },
        ));
    }
    let tally = notes.count();
    let status = match (shown.len(), view.search.trim().is_empty()) {
        (_, true) => format!(
            "{}, {}",
            counted(tally.notes, "note"),
            counted(tally.words, "word")
        ),
        (n, false) => format!("{n} of {} match", counted(tally.notes, "note")),
    };
    tree.push(entry(
        STATUS,
        MAIN,
        wide(),
        label(status, TypeStyle::Small, Tone::Muted),
    ));
    tree
}

/// The changes that turn `old` into `new`: removes for nodes that left (only the
/// top of each removed subtree), then puts for new and changed nodes, parents
/// first. A put of a node the host does not hold appends it to its parent, so a
/// node that moved parent, or whose widget can no longer hold its children, is
/// removed and put again, and so is every later sibling, to keep their order.
pub fn diff(old: &[Entry], new: &[Entry]) -> Vec<Change> {
    let find = |tree: &[Entry], id: NodeId| tree.iter().find(|e| e.id == id).cloned();
    let kids = |tree: &[Entry], id: NodeId| tree.iter().filter(|e| e.parent == id).count();
    let replaced = |e: &Entry| {
        find(new, e.id)
            .is_some_and(|n| n.parent != e.parent || n.widget.max_children() < kids(old, e.id))
    };
    let mut gone: Vec<NodeId> = Vec::new();
    loop {
        let before = gone.len();
        // `old` lists parents before children, so a removed node's children go with it.
        for e in old {
            if !gone.contains(&e.id)
                && (find(new, e.id).is_none() || replaced(e) || gone.contains(&e.parent))
            {
                gone.push(e.id);
            }
        }
        // Once a parent's child is appended, every later kept sibling is appended too.
        let mut appending: Vec<NodeId> = Vec::new();
        for e in new {
            let kept = old.iter().any(|o| o.id == e.id) && !gone.contains(&e.id);
            if !kept {
                appending.push(e.parent);
            } else if appending.contains(&e.parent) {
                gone.push(e.id);
            }
        }
        if gone.len() == before {
            break;
        }
    }
    let mut out: Vec<Change> = gone
        .iter()
        .filter(|&&id| {
            let parent = find(old, id).map_or(ROOT, |e| e.parent);
            !gone.contains(&parent)
        })
        .map(|&id| Change::Remove(id))
        .collect();
    for e in new {
        let same = find(old, e.id).filter(|o| !gone.contains(&o.id));
        if same.as_ref() != Some(e) {
            out.push(Change::Put(e.clone()));
        }
    }
    out
}

/// Replaces a note's text from the editor; false if it is gone or the state
/// has no room for the new text.
pub fn set_text(notes: &mut Notes, id: u32, text: &str) -> bool {
    notes.replace(id, text).is_ok()
}

/// The pause the second press of Delete needs after the press before it, in ms.
/// A double click (two presses a few hundred ms apart) and a held key (a press
/// every few tens of ms) are one press: each press inside the pause moves it on,
/// so holding never confirms.
pub const CONFIRM_PAUSE_MS: u64 = 600;

/// Handles one event from the window at `now_ms` on the call's clock; whether the
/// tree may have changed. Delete asks first: a person's click has no snapshot to
/// undo, so the first press changes the button to "Press again to delete" and
/// only a later press, after a pause and with nothing between, removes the note.
pub fn on_ui(notes: &mut Notes, view: &mut View, e: &UiEvent, now_ms: u64) -> bool {
    let asked = view.delete_asked.take();
    if let (UiEvent::Pressed(DELETE), Some((id, last))) = (e, asked)
        && view.selected == Some(id)
        && now_ms.saturating_sub(last) < CONFIRM_PAUSE_MS
    {
        view.delete_asked = Some((id, now_ms));
        return false;
    }
    handle(notes, view, e, asked.map(|(id, _)| id), now_ms) || asked.is_some()
}

fn handle(
    notes: &mut Notes,
    view: &mut View,
    e: &UiEvent,
    asked: Option<u32>,
    now_ms: u64,
) -> bool {
    match e {
        UiEvent::Pressed(NEW) => match notes.add("") {
            Ok(id) => {
                view.selected = Some(id);
                view.search.clear();
                true
            }
            Err(_) => false,
        },
        UiEvent::Pressed(DELETE) => match view.selected {
            Some(id) if asked == Some(id) => {
                if notes.remove(id).is_ok() {
                    view.selected = None;
                }
                true
            }
            Some(id) if notes.get(id).is_some() => {
                view.delete_asked = Some((id, now_ms));
                true
            }
            _ => false,
        },
        UiEvent::Changed(SEARCH, Value::Text(t)) => {
            view.search.clone_from(t);
            true
        }
        UiEvent::Changed(LIST, Value::Index(i)) => {
            let id = shown(notes, view).get(*i as usize).map(|n| n.id);
            if id.is_some() {
                view.selected = id;
            }
            id.is_some()
        }
        UiEvent::Changed(EDITOR, Value::Text(t)) => match view.selected {
            Some(id) => set_text(notes, id, t),
            None => false,
        },
        UiEvent::Submitted(SEARCH) => {
            let first = shown(notes, view).first().map(|n| n.id);
            if first.is_some() {
                view.selected = first;
            }
            first.is_some()
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::counted;

    #[test]
    fn one_of_a_thing_is_not_plural() {
        assert_eq!(counted(1, "note"), "1 note");
        assert_eq!(counted(0, "note"), "0 notes");
        assert_eq!(counted(2, "word"), "2 words");
    }
}
