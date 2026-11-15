//! Kai Desk's geometry (ADR 0005 §8). Every rectangle `desktop` paints and
//! hit-tests comes from the screen size through [`Layout::for_screen`], and
//! every point a kernel selftest or an xtask case clicks comes from
//! [`points`], so a change to the layout moves the tests' points with it.
//!
//! The numbers here are today's desk: a 28 px top bar, the 36 px AI panel
//! along the bottom, windows cascading from 48,52 in steps of 32, and 28x28
//! title-bar controls 2 px apart. All sizes are physical px at scale 1.

#![no_std]

pub mod points;
mod rect;

pub use points::{Points, points};
pub use rect::{Point, Rect, Size};

/// The top bar.
pub const TOP_BAR: i32 = 28;
/// The AI panel along the bottom.
pub const PANEL: i32 = 36;
/// A window's title bar, inside its frame.
pub const TITLE_BAR: i32 = 32;
/// A window's frame line.
pub const FRAME: i32 = 1;
/// A title-bar control's side.
pub const CONTROL: i32 = 28;
/// The gap from a control to the next and to the frame.
pub const CONTROL_GAP: i32 = 2;
/// How far a window's resize grabs reach outside its frame.
pub const GRAB: i32 = 6;
/// A corner grab's reach along each edge.
pub const CORNER: i32 = 16;
/// Where the first window's frame opens, from the work area's top-left.
pub const CASCADE_ORIGIN: Point = Point { x: 48, y: 24 };
/// How far each new window opens from the last, down and right.
pub const CASCADE: i32 = 32;
/// How many cascade spots there are before the next window starts over.
pub const CASCADE_SPOTS: i32 = 8;
/// The width of the top bar's right end whose text changes while the desk is
/// still (the stats).
pub const STATS_W: i32 = 320;

/// What is open on the desk, for the parts of the layout that depend on it.
/// Today's layout depends on nothing but the screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UiState {}

/// Every surface of the desk at one screen size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub width: i32,
    pub height: i32,
    pub top_bar: Rect,
    pub panel: Rect,
    /// Where windows open and maximize: below the top bar, above the panel.
    pub work_area: Rect,
    /// The top bar's part whose text changes while the desk is still, which a
    /// screendump golden masks.
    pub stats: Rect,
}

impl Layout {
    pub const fn for_screen(width: i32, height: i32, _ui: UiState) -> Layout {
        Layout {
            width,
            height,
            top_bar: Rect::new(0, 0, width, TOP_BAR),
            panel: Rect::new(0, height - PANEL, width, PANEL),
            work_area: Rect::new(0, TOP_BAR, width, height - TOP_BAR - PANEL),
            stats: Rect::new(width - STATS_W, 0, STATS_W, TOP_BAR - 1),
        }
    }

    /// The frame's top-left for the window opened when `open` windows are
    /// already on the desk.
    pub const fn cascade(&self, open: i32) -> Point {
        let n = open % CASCADE_SPOTS;
        Point {
            x: self.work_area.x + CASCADE_ORIGIN.x + n * CASCADE,
            y: self.work_area.y + CASCADE_ORIGIN.y + n * CASCADE,
        }
    }
}

/// The outer frame for `content`: the title bar above it and the frame round
/// both.
pub const fn frame_for(content: Rect) -> Rect {
    Rect::new(
        content.x - FRAME,
        content.y - TITLE_BAR - FRAME,
        content.w + 2 * FRAME,
        content.h + TITLE_BAR + 2 * FRAME,
    )
}

/// The content rectangle inside `frame`.
pub const fn content_of(frame: Rect) -> Rect {
    Rect::new(
        frame.x + FRAME,
        frame.y + FRAME + TITLE_BAR,
        frame.w - 2 * FRAME,
        frame.h - TITLE_BAR - 2 * FRAME,
    )
}

pub const fn title_bar(frame: Rect) -> Rect {
    Rect::new(
        frame.x + FRAME,
        frame.y + FRAME,
        frame.w - 2 * FRAME,
        TITLE_BAR,
    )
}

/// A window's three title-bar controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Controls {
    pub close: Rect,
    pub maximize: Rect,
    pub minimize: Rect,
}

/// The controls of the window whose frame is `frame`: right to left, close,
/// maximize, minimize.
pub const fn controls(frame: Rect) -> Controls {
    let bar = title_bar(frame);
    let y = bar.y + (TITLE_BAR - CONTROL) / 2;
    Controls {
        close: Rect::new(control_x(bar, 0), y, CONTROL, CONTROL),
        maximize: Rect::new(control_x(bar, 1), y, CONTROL, CONTROL),
        minimize: Rect::new(control_x(bar, 2), y, CONTROL, CONTROL),
    }
}

/// The left edge of the `n`th control from the title bar's right end.
const fn control_x(bar: Rect, n: i32) -> i32 {
    bar.right() - CONTROL_GAP - (n + 1) * CONTROL - n * CONTROL_GAP
}

#[cfg(test)]
mod tests;
