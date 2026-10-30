//! A window's retained view tree: what the app last `put`, checked against the
//! caps `desktop` holds a window to (ADR 0005 section 3: 4,096 nodes, 256 KiB).

use crate::widget::{Axis, Layout, NodeId, ROOT, UiError, Widget};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

pub const MAX_NODES: usize = 4096;
pub const MAX_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub parent: NodeId,
    pub children: Vec<NodeId>,
    pub layout: Layout,
    pub widget: Widget,
}

#[derive(Clone, Debug)]
pub struct Tree {
    nodes: BTreeMap<NodeId, Node>,
    bytes: usize,
}

impl Default for Tree {
    fn default() -> Self {
        Self::new()
    }
}

impl Tree {
    /// A tree holding only the root, a column filling the window.
    pub fn new() -> Self {
        let root = Node {
            parent: ROOT,
            children: Vec::new(),
            layout: Layout::FILL,
            widget: Widget::Stack { axis: Axis::Column },
        };
        let bytes = root.widget.weight();
        let mut nodes = BTreeMap::new();
        nodes.insert(ROOT, root);
        Self { nodes, bytes }
    }

    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id)
    }

    pub fn widget_mut(&mut self, id: NodeId) -> Option<&mut Widget> {
        self.nodes.get_mut(&id).map(|n| &mut n.widget)
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.len() <= 1
    }

    /// Adds `id` as `parent`'s last child, or replaces it in place keeping its
    /// children. A node never moves to another parent; remove it first.
    pub fn put(
        &mut self,
        id: NodeId,
        parent: NodeId,
        layout: Layout,
        widget: Widget,
    ) -> Result<(), UiError> {
        if id == ROOT || id == parent {
            return Err(UiError::BadParent);
        }
        widget.check()?;
        let weight = widget.weight();
        if let Some(old) = self.nodes.get(&id) {
            if old.parent != parent || old.children.len() > widget.max_children() {
                return Err(UiError::BadParent);
            }
            let bytes = self.bytes - old.widget.weight() + weight;
            if bytes > MAX_BYTES {
                return Err(UiError::TooBig);
            }
            self.bytes = bytes;
            let node = self.nodes.get_mut(&id).ok_or(UiError::NoNode)?;
            node.layout = layout;
            node.widget = widget;
            return Ok(());
        }
        let p = self.nodes.get(&parent).ok_or(UiError::NoNode)?;
        if p.children.len() >= p.widget.max_children() {
            return Err(UiError::BadParent);
        }
        if self.nodes.len() >= MAX_NODES || self.bytes + weight > MAX_BYTES {
            return Err(UiError::TooBig);
        }
        self.bytes += weight;
        self.nodes.insert(
            id,
            Node {
                parent,
                children: Vec::new(),
                layout,
                widget,
            },
        );
        self.nodes
            .get_mut(&parent)
            .ok_or(UiError::NoNode)?
            .children
            .push(id);
        Ok(())
    }

    /// Removes `id` and everything under it.
    pub fn remove(&mut self, id: NodeId) -> Result<(), UiError> {
        if id == ROOT {
            return Err(UiError::NoNode);
        }
        let parent = self.nodes.get(&id).ok_or(UiError::NoNode)?.parent;
        if let Some(p) = self.nodes.get_mut(&parent) {
            p.children.retain(|&c| c != id);
        }
        let mut stack = alloc::vec![id];
        while let Some(n) = stack.pop() {
            if let Some(node) = self.nodes.remove(&n) {
                self.bytes -= node.widget.weight();
                stack.extend(node.children);
            }
        }
        Ok(())
    }

    /// Every node in paint order: parents before children, children in order.
    pub fn preorder(&self) -> Vec<NodeId> {
        let mut out = Vec::with_capacity(self.nodes.len());
        let mut stack = alloc::vec![ROOT];
        while let Some(n) = stack.pop() {
            out.push(n);
            if let Some(node) = self.nodes.get(&n) {
                stack.extend(node.children.iter().rev());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::{Tone, TypeStyle};
    use alloc::string::String;

    fn label(text: &str) -> Widget {
        Widget::Label {
            text: String::from(text),
            style: TypeStyle::Body,
            tone: Tone::Normal,
        }
    }

    #[test]
    fn put_adds_then_replaces_in_place() {
        let mut t = Tree::new();
        t.put(1, ROOT, Layout::FIT, Widget::Stack { axis: Axis::Row })
            .unwrap();
        t.put(2, 1, Layout::FIT, label("a")).unwrap();
        t.put(3, 1, Layout::FIT, label("b")).unwrap();
        t.put(2, 1, Layout::FIT, label("c")).unwrap();
        assert_eq!(t.get(1).unwrap().children, [2, 3]);
        assert_eq!(t.get(2).unwrap().widget, label("c"));
        assert_eq!(t.preorder(), [0, 1, 2, 3]);
    }

    #[test]
    fn refuses_bad_parents() {
        let mut t = Tree::new();
        assert_eq!(
            t.put(0, 0, Layout::FIT, label("x")),
            Err(UiError::BadParent)
        );
        assert_eq!(t.put(1, 9, Layout::FIT, label("x")), Err(UiError::NoNode));
        t.put(1, ROOT, Layout::FIT, label("x")).unwrap();
        // A label holds no children, and a node never moves.
        assert_eq!(
            t.put(2, 1, Layout::FIT, label("y")),
            Err(UiError::BadParent)
        );
        t.put(2, ROOT, Layout::FIT, Widget::Scroll { axis: Axis::Column })
            .unwrap();
        assert_eq!(
            t.put(1, 2, Layout::FIT, label("x")),
            Err(UiError::BadParent)
        );
        // A scroll view holds one child.
        t.put(3, 2, Layout::FIT, label("z")).unwrap();
        assert_eq!(
            t.put(4, 2, Layout::FIT, label("w")),
            Err(UiError::BadParent)
        );
        // A container with children cannot become a leaf.
        assert_eq!(
            t.put(2, ROOT, Layout::FIT, label("v")),
            Err(UiError::BadParent)
        );
    }

    #[test]
    fn remove_drops_the_subtree_and_its_bytes() {
        let mut t = Tree::new();
        let empty = t.bytes;
        t.put(1, ROOT, Layout::FIT, Widget::Stack { axis: Axis::Row })
            .unwrap();
        t.put(2, 1, Layout::FIT, label("a")).unwrap();
        t.remove(1).unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(t.bytes, empty);
        assert!(t.get(ROOT).unwrap().children.is_empty());
        assert_eq!(t.remove(ROOT), Err(UiError::NoNode));
        assert_eq!(t.remove(1), Err(UiError::NoNode));
    }

    #[test]
    fn caps_nodes_and_bytes() {
        let mut t = Tree::new();
        for id in 1..MAX_NODES as u32 {
            t.put(id, ROOT, Layout::FIT, label("")).unwrap();
        }
        assert_eq!(
            t.put(MAX_NODES as u32, ROOT, Layout::FIT, label("")),
            Err(UiError::TooBig)
        );
        let mut t = Tree::new();
        let big = "x".repeat(MAX_BYTES);
        assert_eq!(
            t.put(1, ROOT, Layout::FIT, label(&big)),
            Err(UiError::TooBig)
        );
        t.put(1, ROOT, Layout::FIT, label("ok")).unwrap();
        assert_eq!(
            t.put(1, ROOT, Layout::FIT, label(&big)),
            Err(UiError::TooBig)
        );
    }
}
