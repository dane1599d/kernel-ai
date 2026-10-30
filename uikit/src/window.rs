//! One window: its tree, its state, and the frames it commits.

use crate::a11y::{self, A11yNode};
use crate::geom::Rect;
use crate::layout::{self, Measure, Places};
use crate::paint::{self, DisplayList, Group};
use crate::state::State;
use crate::tree::Tree;
use crate::widget::{Layout, NodeId, UiError, Widget};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// Damage rectangles a frame may carry before they become their bounding box
/// (the header page holds 32).
pub const MAX_DAMAGE: usize = 32;

/// What one commit shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub title: String,
    pub size: (i32, i32),
    pub list: DisplayList,
    /// What changed since the last frame, in window-local logical px.
    pub damage: Vec<Rect>,
    pub a11y: Vec<A11yNode>,
    /// Whether the window holds changes not yet kept.
    pub unsaved: bool,
    /// Whether the window is kept from the agent's view and actions.
    pub agent_private: bool,
}

pub struct Window {
    pub(crate) title: String,
    pub(crate) size: (i32, i32),
    pub(crate) tree: Tree,
    pub(crate) st: State,
    pub(crate) places: Places,
    last: Option<DisplayList>,
    /// Whether the tree or the state changed since the last commit.
    pub(crate) dirty: bool,
    /// Whether all of it must be redrawn (first frame, resize).
    full: bool,
    unsaved: bool,
    agent_private: bool,
}

impl Window {
    pub fn new(title: &str, width: u16, height: u16) -> Self {
        Self {
            title: String::from(title),
            size: (i32::from(width), i32::from(height)),
            tree: Tree::new(),
            st: State::default(),
            places: Places::new(),
            last: None,
            dirty: true,
            full: true,
            unsaved: false,
            agent_private: false,
        }
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn tree(&self) -> &Tree {
        &self.tree
    }

    pub fn focus_node(&self) -> Option<NodeId> {
        self.st.focus
    }

    /// Whether a commit would show anything new.
    pub fn needs_frame(&self) -> bool {
        self.dirty
    }

    pub fn set_title(&mut self, title: &str) {
        self.title = String::from(title);
        self.dirty = true;
    }

    /// Whether the window holds changes not yet kept; it shows at the next
    /// commit, as the title does.
    pub fn set_unsaved(&mut self, unsaved: bool) {
        if self.unsaved != unsaved {
            self.unsaved = unsaved;
            self.dirty = true;
        }
    }

    pub fn unsaved(&self) -> bool {
        self.unsaved
    }

    /// Whether the window is kept from the agent; it shows at the next commit,
    /// before the window's contents. Who may set it is the host's to decide.
    pub fn set_agent_private(&mut self, private: bool) {
        if self.agent_private != private {
            self.agent_private = private;
            self.dirty = true;
        }
    }

    pub fn agent_private(&self) -> bool {
        self.agent_private
    }

    /// `desktop`'s configure: the window's new size.
    pub fn resize(&mut self, width: u16, height: u16) {
        self.size = (i32::from(width), i32::from(height));
        self.dirty = true;
        self.full = true;
    }

    pub fn put(
        &mut self,
        id: NodeId,
        parent: NodeId,
        layout: Layout,
        widget: Widget,
    ) -> Result<(), UiError> {
        self.tree.put(id, parent, layout, widget)?;
        if let Some(Widget::TextLine(f) | Widget::TextArea(f)) =
            self.tree.get(id).map(|n| &n.widget)
        {
            let len = f.value.len();
            if let Some(c) = self.st.caret.get_mut(&id) {
                *c = (*c).min(len);
            }
        }
        self.dirty = true;
        Ok(())
    }

    pub fn remove(&mut self, id: NodeId) -> Result<(), UiError> {
        self.tree.remove(id)?;
        let tree = &self.tree;
        let gone = |n: &Option<NodeId>| n.is_some_and(|n| tree.get(n).is_none());
        if gone(&self.st.focus) {
            self.st.focus = None;
        }
        if gone(&self.st.hover) {
            self.st.hover = None;
        }
        if gone(&self.st.pressed) {
            self.st.pressed = None;
        }
        if gone(&self.st.menu) {
            self.st.menu = None;
            self.st.menu_hover = None;
        }
        self.st.caret.retain(|n, _| tree.get(*n).is_some());
        self.st.scroll.retain(|n, _| tree.get(*n).is_some());
        self.dirty = true;
        Ok(())
    }

    /// The app asks for focus on `id`; it must be a focusable node.
    pub fn focus(&mut self, id: NodeId) -> Result<(), UiError> {
        let node = self.tree.get(id).ok_or(UiError::NoNode)?;
        if !node.widget.focusable() {
            return Err(UiError::NoNode);
        }
        self.st.focus = Some(id);
        self.dirty = true;
        Ok(())
    }

    /// Lays the tree out again if anything changed.
    pub(crate) fn relayout(&mut self, m: &dyn Measure) {
        if self.dirty || self.places.is_empty() {
            self.places = layout::layout(&self.tree, m, self.size, &self.st.scroll);
        }
    }

    /// Lays out, paints and diffs: the frame to show.
    pub fn commit(&mut self, m: &dyn Measure) -> Frame {
        self.places = layout::layout(&self.tree, m, self.size, &self.st.scroll);
        let list = paint::paint(&self.tree, &self.places, &self.st, m);
        let window = Rect::new(0, 0, self.size.0, self.size.1);
        let damage = match (&self.last, self.full) {
            (Some(old), false) => damage(&old.groups, &list.groups, window),
            _ => alloc::vec![window],
        };
        let a11y = a11y::build(&self.tree, &self.places, &self.st, &self.title);
        self.last = Some(list.clone());
        self.dirty = false;
        self.full = false;
        Frame {
            title: self.title.clone(),
            size: self.size,
            list,
            damage,
            a11y,
            unsaved: self.unsaved,
            agent_private: self.agent_private,
        }
    }
}

/// Groups by (node, how many times that node came before), so a menu's overlay
/// is told apart from the menu button.
fn keyed(groups: &[Group]) -> BTreeMap<(NodeId, usize), &Group> {
    let mut seen: BTreeMap<NodeId, usize> = BTreeMap::new();
    groups
        .iter()
        .map(|g| {
            let n = seen.entry(g.node).or_insert(0);
            *n += 1;
            ((g.node, *n), g)
        })
        .collect()
}

/// The rectangles that differ between two display lists, merged where they
/// overlap, and their bounding box past [`MAX_DAMAGE`].
pub fn damage(old: &[Group], new: &[Group], window: Rect) -> Vec<Rect> {
    let (a, b) = (keyed(old), keyed(new));
    let mut rects: Vec<Rect> = Vec::new();
    for (k, g) in &a {
        match b.get(k) {
            Some(h) if h == g => {}
            Some(h) => {
                rects.push(g.bounds);
                rects.push(h.bounds);
            }
            None => rects.push(g.bounds),
        }
    }
    rects.extend(
        b.iter()
            .filter(|(k, _)| !a.contains_key(k))
            .map(|(_, g)| g.bounds),
    );
    let mut merged: Vec<Rect> = Vec::new();
    for r in rects
        .into_iter()
        .map(|r| r.intersect(&window))
        .filter(|r| !r.is_empty())
    {
        let mut r = r;
        while let Some(i) = merged.iter().position(|m| m.overlaps(&r)) {
            r = r.union(&merged.swap_remove(i));
        }
        merged.push(r);
    }
    if merged.len() > MAX_DAMAGE {
        let all = merged.iter().fold(Rect::default(), |acc, r| acc.union(r));
        return alloc::vec![all];
    }
    merged.sort_by_key(|r| (r.y, r.x));
    merged
}
