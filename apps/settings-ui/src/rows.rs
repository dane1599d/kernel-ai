//! Rows that keep naming what they showed, and pages that stay within the
//! window's caps.

use crate::MAX_ROWS;
use crate::tree::Entry;
use alloc::vec::Vec;
use uikit::tree::{MAX_BYTES, MAX_NODES};

/// Row ids that stay with what they show: a row keeps its slot while it is
/// shown, and a new row takes a slot no row had in this view, so a press that
/// arrives after its row went never lands on the row that moved up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Slots<K> {
    keys: Vec<Option<K>>,
}

impl<K> Default for Slots<K> {
    fn default() -> Self {
        Slots { keys: Vec::new() }
    }
}

impl<K: Clone + PartialEq> Slots<K> {
    /// The slot of each of `shown`, in order. Rows no longer shown leave a
    /// hole; when every slot is taken the slots start again from `shown`.
    pub(crate) fn place(&mut self, shown: &[K]) -> Vec<usize> {
        for slot in &mut self.keys {
            if slot.as_ref().is_some_and(|k| !shown.contains(k)) {
                *slot = None;
            }
        }
        let fresh = shown.iter().filter(|k| !self.has(k)).count();
        if self.keys.len() + fresh > MAX_ROWS {
            self.keys = shown.iter().cloned().map(Some).collect();
        }
        shown
            .iter()
            .map(
                |k| match self.keys.iter().position(|s| s.as_ref() == Some(k)) {
                    Some(slot) => slot,
                    None => {
                        self.keys.push(Some(k.clone()));
                        self.keys.len() - 1
                    }
                },
            )
            .collect()
    }

    pub(crate) fn get(&self, slot: usize) -> Option<&K> {
        self.keys.get(slot)?.as_ref()
    }

    fn has(&self, k: &K) -> bool {
        self.keys.iter().any(|s| s.as_ref() == Some(k))
    }
}

/// Room kept for the window's root and a "more not shown" line.
const SPARE_NODES: usize = 4;
const SPARE_BYTES: usize = 1024;

/// What the tree uses of `uikit`'s caps on one window.
pub(crate) struct Room {
    nodes: usize,
    bytes: usize,
}

impl Room {
    pub(crate) fn of(tree: &[Entry]) -> Room {
        Room {
            nodes: tree.len(),
            bytes: tree.iter().map(|e| e.widget.weight()).sum(),
        }
    }

    /// Whether `row` still fits; if so, it is counted.
    pub(crate) fn take(&mut self, row: &[Entry]) -> bool {
        let nodes = self.nodes + row.len();
        let bytes = self.bytes + row.iter().map(|e| e.widget.weight()).sum::<usize>();
        let fits = nodes + SPARE_NODES <= MAX_NODES && bytes + SPARE_BYTES <= MAX_BYTES;
        if fits {
            (self.nodes, self.bytes) = (nodes, bytes);
        }
        fits
    }
}
