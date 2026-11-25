//! Kai Desk's geometry (ADR 0005 §8). Every rectangle `desktop` paints and
//! hit-tests comes from the screen size through [`Layout::for_screen`], and
//! every point a kernel selftest or an xtask case clicks comes from
//! [`points`], so a change to the layout moves the tests' points with it.
//!
//! The numbers here are Kai Desk's: the 48 px taskbar along the bottom
//! with its centred items and right-aligned tray (PR 7), the Kai pane down
//! the right edge (PR 6), windows cascading from 48,52 in steps of 32, and
//! 40x32 title-bar controls flush in the bar's right end (PR 5). There is no
//! top bar. All sizes are logical px, which are physical px at scale 1:
//! `desktop` lays out on the screen's logical size and turns the frame into
//! physical px in one place (ADR 0005 §9, `deskcore::upscale`).

#![no_std]

pub mod pane;
pub mod points;
mod rect;
pub mod scale;
pub mod start;
pub mod taskbar;

pub use pane::{PaneParts, pane_width};
pub use points::{Points, points};
pub use rect::{Point, Rect, Size};
pub use taskbar::Tray;

/// The taskbar along the bottom.
pub const TASKBAR: i32 = 48;
/// A flyout's distance from the screen's edges and from the taskbar.
pub const FLYOUT_MARGIN: i32 = 12;
/// From this width up the pane stays open beside the windows, and maximized
/// windows fit beside it; under it the pane is a flyout.
pub const PANE_BESIDE: i32 = 1280;
/// A window's title bar, inside its frame.
pub const TITLE_BAR: i32 = 32;
/// A window's frame line.
pub const FRAME: i32 = 1;
/// A title-bar control's width (`look::metric::CONTROL_W`).
pub const CONTROL_W: i32 = 40;
/// A title-bar control's height: the whole bar (`look::metric::CONTROL_H`).
pub const CONTROL_H: i32 = TITLE_BAR;
/// How far a window's resize grabs reach outside its frame.
pub const GRAB: i32 = 6;
/// A corner grab's reach along each edge.
pub const CORNER: i32 = 16;
/// Where the first window's frame opens, from the work area's top-left (the
/// screen's): 48,52, where it opened under the old top bar too.
pub const CASCADE_ORIGIN: Point = Point { x: 48, y: 52 };
/// How far each new window opens from the last, down and right.
pub const CASCADE: i32 = 32;
/// How many cascade spots there are before the next window starts over.
pub const CASCADE_SPOTS: i32 = 8;

/// What is open on the desk, for the parts of the layout that depend on it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UiState {
    /// The Kai pane is open.
    pub pane_open: bool,
}

/// Every surface of the desk at one screen size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub width: i32,
    pub height: i32,
    /// Where windows open: the screen above the taskbar.
    pub work_area: Rect,
    /// The taskbar along the bottom.
    pub taskbar: Rect,
    /// The tray's parts, right-aligned in the taskbar.
    pub tray: Tray,
    /// Where a maximized window goes: the work area, or at 1280 wide and up
    /// with the pane open, the part of it left of the pane.
    pub maximized: Rect,
    /// The Kai pane, open or not (`ui.pane_open` says which).
    pub pane: Rect,
    pub pane_open: bool,
    /// The ask card's and the Apps sheet's bottom edge, 12 above the taskbar.
    pub card_bottom: i32,
    /// The tallest the card grows: the whole work height, from 12 below the
    /// screen's top to `card_bottom` (H-72). A call that does not fit is
    /// never cut: the card refuses it (`deskcore::trusted::fits`).
    pub card_max_h: i32,
}

impl Layout {
    pub const fn for_screen(width: i32, height: i32, ui: UiState) -> Layout {
        let work_area = Rect::new(0, 0, width, height - TASKBAR);
        let pane = pane::pane(width, height);
        let maximized = match ui.pane_open && width >= PANE_BESIDE {
            true => Rect::new(0, work_area.y, pane.x - FLYOUT_MARGIN, work_area.h),
            false => work_area,
        };
        Layout {
            width,
            height,
            work_area,
            taskbar: Rect::new(0, height - TASKBAR, width, TASKBAR),
            tray: taskbar::tray(width, height),
            maximized,
            pane,
            pane_open: ui.pane_open,
            card_bottom: height - TASKBAR - FLYOUT_MARGIN,
            card_max_h: height - TASKBAR - 2 * FLYOUT_MARGIN,
        }
    }

    /// The pane's parts (wherever it is, open or not).
    pub const fn pane_parts(&self) -> PaneParts {
        pane::parts(self.pane)
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
/// maximize, minimize, side by side and flush with the bar's top and right.
pub const fn controls(frame: Rect) -> Controls {
    let bar = title_bar(frame);
    let y = bar.y + (TITLE_BAR - CONTROL_H) / 2;
    Controls {
        close: Rect::new(control_x(bar, 0), y, CONTROL_W, CONTROL_H),
        maximize: Rect::new(control_x(bar, 1), y, CONTROL_W, CONTROL_H),
        minimize: Rect::new(control_x(bar, 2), y, CONTROL_W, CONTROL_H),
    }
}

/// The left edge of the `n`th control from the title bar's right end.
const fn control_x(bar: Rect, n: i32) -> i32 {
    bar.right() - (n + 1) * CONTROL_W
}

#[cfg(test)]
mod tests;
