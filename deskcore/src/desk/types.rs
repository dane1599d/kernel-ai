//! The desk's vocabulary: windows, focus, input, the effects `desktop` carries
//! out, and what a point hits (moved out of `mod.rs`, not changed).

use crate::chrome::{self, Edges, Part};
use crate::geom::{Point, Rect, Size};
use alloc::string::String;

use super::{PanePart, TrayPart};

pub mod usage {
    pub const TAB: u16 = 0x2B;
    pub const SPACE: u16 = 0x2C;
    pub const LEFT_CTRL: u16 = 0xE0;
    pub const LEFT_SHIFT: u16 = 0xE1;
    pub const LEFT_ALT: u16 = 0xE2;
    pub const LEFT_GUI: u16 = 0xE3;
    pub const RIGHT_CTRL: u16 = 0xE4;
    pub const RIGHT_SHIFT: u16 = 0xE5;
    pub const RIGHT_ALT: u16 = 0xE6;
    pub const RIGHT_GUI: u16 = 0xE7;
    pub const Y: u16 = 0x1C;
    pub const N: u16 = 0x11;
}
/// The smallest content a resize leaves.
pub(super) const MIN_CONTENT: Size = Size { w: 120, h: 60 };

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WindowId(pub u32);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    pub id: WindowId,
    /// The connection it came from.
    pub client: u32,
    /// The kernel's label for the process, drawn before the title.
    pub label: String,
    pub title: String,
    pub app_id: String,
    /// The outer frame, title bar included.
    pub frame: Rect,
    pub minimized: bool,
    /// The frame to go back to from maximized.
    pub restore: Option<Rect>,
    pub min: Size,
    pub max: Size,
}

impl Window {
    pub fn content(&self) -> Rect {
        chrome::content_of(self.frame)
    }

    pub fn maximized(&self) -> bool {
        self.restore.is_some()
    }
}

/// Where keys go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Nothing,
    Window(WindowId),
    Panel,
    /// The ask card with this log entry id.
    Ask(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Middle,
}

/// Input in logical px, as `desktop` turns the kernel's events into it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Motion(Point),
    Button {
        button: Button,
        down: bool,
    },
    Key {
        usage: u16,
        down: bool,
    },
    /// The vertical wheel turned by detents, positive toward the person
    /// (scroll down).
    Wheel(i32),
}

/// What `desktop` must do after a change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// The pointer is over `window`'s content at `at` (content-local px).
    Pointer { window: WindowId, at: Point },
    /// The pointer left `window`'s content.
    Leave { window: WindowId },
    Button {
        window: WindowId,
        at: Point,
        button: Button,
        down: bool,
    },
    Key {
        window: WindowId,
        usage: u16,
        down: bool,
    },
    /// A key for the AI panel.
    PanelKey { usage: u16, down: bool },
    /// A press on the AI panel, at `at` on the screen: `desktop` decides
    /// whether it was on a control there (undo, show).
    PanelClick { at: Point },
    /// Caps Lock was turned on or off: every client types letters by it.
    Locks { caps: bool },
    /// The modifiers held changed (shift 1, ctrl 2, alt 4, super 8), whatever
    /// had the focus when they were pressed or let go: every client's seat
    /// hears it, and keys after it carry it. Sent before the key that changed it.
    Modifiers(u32),
    /// Keyboard focus moved.
    Focus(Focus),
    /// The taskbar's "more" item: All apps.
    ShowApps,
    /// Start opened (true) or closed: `desktop` gives it the keys meanwhile.
    Start(bool),
    /// A press inside Start at `at` on the screen: `desktop` decides what.
    StartClick { at: Point },
    /// A line for the panel to send as though typed: a pin's `open <name>`.
    PanelLine(String),
    /// The wheel over the Kai pane's history, by detents (positive scrolls
    /// toward the newest).
    PaneWheel(i32),
    /// The wheel over Start's lists, by detents (positive moves down).
    StartWheel(i32),
    /// The wheel over `window`'s content at `at` (content-local px), by
    /// detents (positive scrolls down): a ring event for its client.
    Wheel {
        window: WindowId,
        at: Point,
        detents: i32,
    },
    /// Tell the client its window's content size and state
    /// (`toplevel.configure`).
    Configure {
        window: WindowId,
        size: Size,
        maximized: bool,
        activated: bool,
    },
    /// The close control was pressed: the client decides (`toplevel.close`).
    Close { window: WindowId },
    /// The user answered the ask card for log entry `id`: y mints its `ONCE`.
    Answer { id: u64, yes: bool },
    /// The user acted in the window the agent was driving: stop the run.
    TookOver { window: WindowId },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Grab {
    Move {
        window: WindowId,
        offset: Point,
    },
    Resize {
        window: WindowId,
        edges: Edges,
        start: Rect,
        from: Point,
    },
}

/// How much of a dragged window's title bar stays on screen, either side.
pub(super) const KEEP_ON_SCREEN: i32 = 64;

/// What a point on the screen hits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Window(WindowId, Part),
    /// The taskbar: item `i` of the items now, or between them.
    Taskbar(Option<usize>),
    Tray(TrayPart),
    /// Start, while it is open: over everything but the card.
    Start,
    /// The Kai pane, while it is open.
    Pane(PanePart),
    Desktop,
}
