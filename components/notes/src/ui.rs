//! Notes' window on `kai:ui` (ADR 0005 section 4): a search field, the list of
//! notes and the editor, built as a `uikit` tree. The tree is rebuilt whole from
//! the state after each event and [`diff`]ed against the last one, so only what
//! changed goes to the host. The actions in `kai.actions` are unchanged: "New
//! note" is bound to `notes_add`, so the agent sees the same words and tier.

use crate::{Actions, Notes};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
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
pub fn shown<'a>(notes: &'a Notes, view: &View) -> Vec<&'a crate::state::Note> {
    notes
        .state
        .notes
        .iter()
        .filter(|n| matches(&n.text, &view.search))
        .collect()
}

/// A list item's label: the note's first line, cut to [`ITEM_CHARS`].
pub fn title(text: &str) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let line = line.trim();
    if line.is_empty() {
        return String::from("(empty)");
    }
    let mut out: String = line.chars().take(ITEM_CHARS).collect();
    if line.chars().count() > ITEM_CHARS {
        out.push('…');
    }
    out
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
    let tally = notes.count();
    let status = match (shown.len(), view.search.trim().is_empty()) {
        (_, true) => format!("{} notes, {} words", tally.notes, tally.words),
        (n, false) => format!("{n} of {} notes match", tally.notes),
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

impl Notes {
    /// Replaces a note's text from the editor; false if it is gone or the
    /// state has no room for the new text.
    pub fn set_text(&mut self, id: u32, text: &str) -> bool {
        let old = match self.state.notes.iter().find(|n| n.id == id) {
            Some(n) => n.text.len(),
            None => return false,
        };
        let grow = text.len().saturating_sub(old);
        let size = compsdk::state::encode(crate::SCHEMA, &self.state).len() + grow;
        if grow > 0 && size > compsdk::MAX_STATE {
            return false;
        }
        match self.state.notes.iter_mut().find(|n| n.id == id) {
            Some(n) => {
                n.text = String::from(text);
                n.words = crate::state::words(text);
                true
            }
            None => false,
        }
    }

    /// Handles one event from the window; whether the tree may have changed.
    pub fn on_ui(&mut self, view: &mut View, e: &UiEvent) -> bool {
        match e {
            UiEvent::Pressed(NEW) => match self.add("") {
                Ok(id) => {
                    view.selected = Some(id);
                    view.search.clear();
                    true
                }
                Err(_) => false,
            },
            UiEvent::Changed(SEARCH, Value::Text(t)) => {
                view.search.clone_from(t);
                true
            }
            UiEvent::Changed(LIST, Value::Index(i)) => {
                let id = shown(self, view).get(*i as usize).map(|n| n.id);
                if id.is_some() {
                    view.selected = id;
                }
                id.is_some()
            }
            UiEvent::Changed(EDITOR, Value::Text(t)) => match view.selected {
                Some(id) => self.set_text(id, t),
                None => false,
            },
            UiEvent::Submitted(SEARCH) => {
                let first = shown(self, view).first().map(|n| n.id);
                if first.is_some() {
                    view.selected = first;
                }
                first.is_some()
            }
            _ => false,
        }
    }
}
