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

/// Most rows a code view holds, and most cells of one row's text.
pub const MAX_CODE_ROWS: usize = 200;
pub const MAX_CODE_CELLS: usize = 400;
/// Most spans on one row.
pub const MAX_CODE_SPANS: usize = 32;

/// A run of one code row's cells, `from` to `to` (not including it), drawn in
/// `tone`, underlined when it is a link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeSpan {
    pub from: u16,
    pub to: u16,
    pub tone: Tone,
    pub link: bool,
}

/// One row of a code view: text whose every character is one cell (tabs already
/// spread), the gutter text beside it, and the runs drawn apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeRow {
    pub text: String,
    pub gutter: String,
    pub spans: Vec<CodeSpan>,
}

/// A grid of mono text the app owns, such as an editor's lines or a terminal's
/// screen, which takes raw keys and clicks (`kai:code`). The app sends the rows
/// to show; uikit draws them and tells the app what was typed and where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeView {
    /// Its accessible name.
    pub label: String,
    /// Cells at the left kept for the gutter.
    pub gutter_width: u16,
    pub rows: Vec<CodeRow>,
    /// The caret's row and cell (after the gutter); none hides it.
    pub cursor: Option<(u32, u32)>,
    /// A row drawn highlighted.
    pub selected: Option<u32>,
}

/// The v0.1 widgets (ADR 0005 section 4), and the code view (`kai:code`).
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
    Code(CodeView),
}

impl Widget {
    /// Whether the widget is within the limits an app is held to.
    pub fn check(&self) -> Result<(), UiError> {
        let Self::Code(v) = self else {
            return Ok(());
        };
        let spans_ok = |r: &CodeRow| {
            r.spans.len() <= MAX_CODE_SPANS
                && r.spans
                    .iter()
                    .all(|s| s.from < s.to && usize::from(s.to) <= MAX_CODE_CELLS)
        };
        let rows_ok = v.rows.iter().all(|r| {
            r.text.chars().count() <= MAX_CODE_CELLS && r.gutter.len() <= 16 && spans_ok(r)
        });
        if v.rows.len() > MAX_CODE_ROWS || !rows_ok {
            return Err(UiError::TooBig);
        }
        Ok(())
    }

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
                | Self::Code(_)
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
            Self::Code(v) => {
                let rows: usize = v
                    .rows
                    .iter()
                    .map(|r| r.text.len() + r.gutter.len() + 8 * r.spans.len() + 8)
                    .sum();
                v.label.len() + rows
            }
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
    /// A key pressed while a code view had focus (`kai:code/code-input.on-key`):
    /// its name (`a`, `enter`, `up`, `backspace`, `pagedown`, `f1`...), what it
    /// types (empty for a command key) and the modifiers held (shift 1, ctrl 2,
    /// alt 4, super 8).
    Key {
        node: NodeId,
        name: String,
        text: String,
        mods: u8,
    },
    /// A click on a code view: the row from its first shown row and the cell
    /// from its first text cell (0 in the gutter), with the modifiers held.
    Click {
        node: NodeId,
        row: u32,
        cell: u32,
        mods: u8,
    },
    /// The wheel turned over a code view: rows, positive down.
    Scroll {
        node: NodeId,
        rows: i32,
    },
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
