//! The tree's nodes and the diff between two trees. The same rules as
//! `comp-notes-ui`'s `diff` (`components/notes-ui/src/lib.rs`), which `uikit`
//! should hold for every app (lane C's); here it looks nodes up by id instead of
//! scanning, because this tree has a row for every line of scrollback and the
//! diff runs on each key.

use alloc::collections::{BTreeMap, BTreeSet};
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

/// Fills its parent's width and fits its height.
pub(crate) fn wide() -> Layout {
    Layout {
        width: Size::Fill(1),
        ..Layout::FIT
    }
}

pub(crate) fn padded(pad: u16, gap: u16) -> Layout {
    Layout {
        padding: Insets::all(pad),
        gap,
        ..wide()
    }
}

pub(crate) fn column() -> Widget {
    Widget::Stack { axis: Axis::Column }
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

/// The changes that turn `old` into `new`: removes for nodes that left (only the
/// top of each removed subtree), then puts for new and changed nodes, parents
/// first. A put of a node the host does not hold appends it to its parent, so a
/// node that moved parent, or whose widget can no longer hold its children, is
/// removed and put again, and so is every later sibling, to keep their order.
pub fn diff(old: &[Entry], new: &[Entry]) -> Vec<Change> {
    let old_by: BTreeMap<NodeId, &Entry> = old.iter().map(|e| (e.id, e)).collect();
    let new_by: BTreeMap<NodeId, &Entry> = new.iter().map(|e| (e.id, e)).collect();
    let mut old_kids: BTreeMap<NodeId, usize> = BTreeMap::new();
    for e in old {
        *old_kids.entry(e.parent).or_default() += 1;
    }
    let replaced = |e: &Entry| {
        new_by.get(&e.id).is_some_and(|n| {
            n.parent != e.parent
                || n.widget.max_children() < old_kids.get(&e.id).copied().unwrap_or(0)
        })
    };
    let mut gone: BTreeSet<NodeId> = BTreeSet::new();
    let mut order: Vec<NodeId> = Vec::new();
    loop {
        let before = gone.len();
        // `old` lists parents before children, so a removed node's children go with it.
        for e in old {
            if !gone.contains(&e.id)
                && (!new_by.contains_key(&e.id) || replaced(e) || gone.contains(&e.parent))
            {
                gone.insert(e.id);
                order.push(e.id);
            }
        }
        // Once a parent's child is appended, every later kept sibling is appended too.
        let mut appending: BTreeSet<NodeId> = BTreeSet::new();
        for e in new {
            let kept = old_by.contains_key(&e.id) && !gone.contains(&e.id);
            if !kept {
                appending.insert(e.parent);
            } else if appending.contains(&e.parent) {
                gone.insert(e.id);
                order.push(e.id);
            }
        }
        if gone.len() == before {
            break;
        }
    }
    let mut out: Vec<Change> = order
        .iter()
        .filter(|id| {
            let parent = old_by.get(id).map_or(ROOT, |e| e.parent);
            !gone.contains(&parent)
        })
        .map(|&id| Change::Remove(id))
        .collect();
    for e in new {
        let same = old_by.get(&e.id).filter(|o| !gone.contains(&o.id));
        if same.copied() != Some(e) {
            out.push(Change::Put(e.clone()));
        }
    }
    out
}
