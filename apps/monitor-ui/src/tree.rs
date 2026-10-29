//! The tree's nodes and the diff between two trees: the same as `comp-notes-ui`'s
//! (`components/notes-ui/src/lib.rs`) and `comp-settings-ui`'s, kept here until
//! `uikit` holds one copy for every app (lane C's).

use alloc::string::String;
use alloc::vec::Vec;
use uikit::style::{Tone, TypeStyle};
use uikit::widget::{Axis, Insets, Layout, NodeId, ROOT, Size, Widget};

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

pub(crate) fn entry(id: NodeId, parent: NodeId, layout: Layout, widget: Widget) -> Entry {
    Entry {
        id,
        parent,
        layout,
        widget,
    }
}

pub(crate) fn column(pad: u16, gap: u16) -> Layout {
    Layout {
        padding: Insets::all(pad),
        gap,
        ..Layout::FILL
    }
}

pub(crate) fn wide() -> Layout {
    Layout {
        width: Size::Fill(1),
        ..Layout::FIT
    }
}

pub(crate) fn row() -> Widget {
    Widget::Stack { axis: Axis::Row }
}

pub(crate) fn label(text: &str, style: TypeStyle, tone: Tone) -> Widget {
    Widget::Label {
        text: String::from(text),
        style,
        tone,
    }
}

/// The changes that turn `old` into `new`: a removed node's children go with
/// it, and once a parent gets a new child, its later kept children are put
/// again after it, so order holds.
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
        for e in old {
            if !gone.contains(&e.id)
                && (find(new, e.id).is_none() || replaced(e) || gone.contains(&e.parent))
            {
                gone.push(e.id);
            }
        }
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
