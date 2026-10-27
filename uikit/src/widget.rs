//! The widgets and layout of `kai:ui@0.1.0` (`wit/kai-ui/view.wit`), as Rust.
//! comphost converts the guest's lifted values into these.

use crate::style::{Tone, TypeStyle};
use alloc::string::String;
use alloc::vec::Vec;

/// A node's id in its window; 0 is the root.
pub type NodeId = u32;
pub const ROOT: NodeId = 0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Row,
    Column,
}

/// How much room a node takes along one axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    Fit,
    /// A share of what is left, by weight; 0 counts as 1.
    Fill(u16),
    Fixed(u16),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Insets {
    pub top: u16,
    pub right: u16,
    pub bottom: u16,
    pub left: u16,
}

impl Insets {
    pub const fn all(d: u16) -> Self {
        Self {
            top: d,
            right: d,
            bottom: d,
            left: d,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub width: Size,
    pub height: Size,
    pub padding: Insets,
    pub gap: u16,
}

impl Layout {
    /// Fits its content, no padding, no gap.
    pub const FIT: Self = Self {
        width: Size::Fit,
        height: Size::Fit,
        padding: Insets::all(0),
        gap: 0,
    };
    /// Fills its parent both ways.
    pub const FILL: Self = Self {
        width: Size::Fill(1),
        height: Size::Fill(1),
        padding: Insets::all(0),
        gap: 0,
    };
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextField {
    pub label: String,
    pub value: String,
    pub placeholder: String,
    pub max_len: u32,
}

/// The most bytes a text field takes, whatever its `max_len` says.
pub const MAX_TEXT: u32 = 65_536;

/// The v0.1 widgets (ADR 0005 section 4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Widget {
    Stack {
        axis: Axis,
    },
    Label {
        text: String,
        style: TypeStyle,
        tone: Tone,
    },
    Button {
        text: String,
        action: Option<String>,
    },
    TextLine(TextField),
    TextArea(TextField),
    ItemList {
        label: String,
        selected: Option<u32>,
    },
    Table {
        label: String,
        columns: Vec<String>,
        rows: Vec<Vec<String>>,
        selected: Option<u32>,
    },
    Tabs {
        labels: Vec<String>,
        selected: u32,
    },
    Toggle {
        label: String,
        on: bool,
    },
    Slider {
        label: String,
        min: i32,
        max: i32,
        step: i32,
        value: i32,
    },
    Menu {
        label: String,
        items: Vec<String>,
    },
    Scroll {
        axis: Axis,
    },
    Split {
        axis: Axis,
        /// The first child's share, per mille.
        first: u16,
    },
    Image {
        label: String,
        source: String,
    },
    Progress {
        label: String,
        /// Per mille; none while it cannot tell.
        done: Option<u16>,
    },
}

impl Widget {
    /// How many children it may hold (`usize::MAX` for any number).
    pub fn max_children(&self) -> usize {
        match self {
            Self::Stack { .. } | Self::ItemList { .. } | Self::Tabs { .. } => usize::MAX,
            Self::Scroll { .. } => 1,
            Self::Split { .. } => 2,
            _ => 0,
        }
    }

    /// Whether it takes keyboard focus.
    pub fn focusable(&self) -> bool {
        matches!(
            self,
            Self::Button { .. }
                | Self::TextLine(_)
                | Self::TextArea(_)
                | Self::ItemList { .. }
                | Self::Table { .. }
                | Self::Tabs { .. }
                | Self::Toggle { .. }
                | Self::Slider { .. }
                | Self::Menu { .. }
        )
    }

    /// Roughly how many bytes it holds, for the window's 256 KiB cap.
    pub fn weight(&self) -> usize {
        const NODE: usize = 64;
        let strings = |v: &[String]| v.iter().map(|s| s.len() + 8).sum::<usize>();
        NODE + match self {
            Self::Stack { .. } | Self::Scroll { .. } | Self::Split { .. } => 0,
            Self::Label { text, .. } => text.len(),
            Self::Button { text, action } => text.len() + action.as_ref().map_or(0, String::len),
            Self::TextLine(f) | Self::TextArea(f) => {
                f.label.len() + f.value.len() + f.placeholder.len()
            }
            Self::ItemList { label, .. }
            | Self::Toggle { label, .. }
            | Self::Slider { label, .. }
            | Self::Progress { label, .. } => label.len(),
            Self::Table {
                label,
                columns,
                rows,
                ..
            } => label.len() + strings(columns) + rows.iter().map(|r| strings(r)).sum::<usize>(),
            Self::Tabs { labels, .. } => strings(labels),
            Self::Menu { label, items } => label.len() + strings(items),
            Self::Image { label, source } => label.len() + source.len(),
        }
    }
}

/// A widget's new value, as `kai:ui/input.value`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Text(String),
    On(bool),
    Number(i32),
    Index(u32),
}

/// What `uikit` tells the app, as `kai:ui/input.ui-event`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiEvent {
    Pressed(NodeId),
    Changed(NodeId, Value),
    Submitted(NodeId),
    CloseRequested,
}

/// Why the tree refused a change, as `kai:ui/types.ui-error`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiError {
    NoWindow,
    NoNode,
    BadParent,
    TooBig,
    TooManyWindows,
}
