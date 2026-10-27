//! What a window remembers between frames that the app's tree does not hold.

use crate::widget::NodeId;
use alloc::collections::BTreeMap;

#[derive(Clone, Debug, Default)]
pub struct State {
    /// The node with keyboard focus inside the window.
    pub focus: Option<NodeId>,
    /// Whether `desktop` gave this window keyboard focus.
    pub window_focused: bool,
    /// The node under the pointer.
    pub hover: Option<NodeId>,
    /// The button held down, until the pointer is released.
    pub pressed: Option<NodeId>,
    /// The open menu and the item under the pointer in it.
    pub menu: Option<NodeId>,
    pub menu_hover: Option<usize>,
    /// The caret of each text field, as a byte offset into its value.
    pub caret: BTreeMap<NodeId, usize>,
    /// The offset of each scroll view, in logical px.
    pub scroll: BTreeMap<NodeId, i32>,
}
