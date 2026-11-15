//! The Kai pane's rectangle and its parts (ADR 0005 §8, "The Kai pane"):
//! a flyout down the right edge, 12 px from the screen's top and right and
//! from the taskbar, holding the header, the history, the code-only status
//! row and the input. Parts are in screen px, from the pane's rectangle.

use crate::{FLYOUT_MARGIN, Rect, TASKBAR};

/// The pane's header.
pub const HEADER: i32 = 48;
/// The header's close item, and the input's send item.
pub const ITEM: i32 = 32;
/// The status row.
pub const STATUS: i32 = 32;
/// The input field.
pub const INPUT: i32 = 44;
/// The history's and the header's inset from the pane's sides.
pub const INSET: i32 = 16;
/// The input field's inset from the pane's sides and bottom.
pub const INPUT_INSET: i32 = 12;
/// Where the Kai mark sits in the header, and its side.
pub const MARK: (i32, i32, i32) = (16, 14, 20);
/// Where "Kai" starts in the header.
pub const TITLE_X: i32 = 44;
/// The Stop item in the status row while Kai thinks.
pub const STOP: (i32, i32) = (64, 24);

/// The pane's width at a screen `width` px wide: 400 from 1600, 360 from
/// 1200, else 340.
pub const fn pane_width(width: i32) -> i32 {
    if width >= 1600 {
        400
    } else if width >= 1200 {
        360
    } else {
        340
    }
}

/// The pane at a `width` x `height` screen: W-12-w across, 12 down, H-72
/// tall (12 above the taskbar).
pub const fn pane(width: i32, height: i32) -> Rect {
    let w = pane_width(width);
    Rect::new(
        width - FLYOUT_MARGIN - w,
        FLYOUT_MARGIN,
        w,
        height - 2 * FLYOUT_MARGIN - TASKBAR,
    )
}

/// The pane's parts, in screen px.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneParts {
    pub header: Rect,
    /// The Kai mark, 20 px.
    pub mark: Rect,
    /// The header's close item.
    pub close: Rect,
    /// Your lines and Kai's words: nothing is drawn past it.
    pub history: Rect,
    /// The code-only state line, a 1 px rule along its top.
    pub status: Rect,
    /// The Stop item at the status row's right end, while Kai thinks.
    pub stop: Rect,
    /// The input field.
    pub input: Rect,
    /// The send item at the input's right end (Stop while Kai thinks).
    pub send: Rect,
}

/// The parts of the pane at `pane`.
pub const fn parts(pane: Rect) -> PaneParts {
    let (x, y, w, h) = (pane.x, pane.y, pane.w, pane.h);
    let status = Rect::new(x, y + h - 100, w, STATUS);
    let input = Rect::new(
        x + INPUT_INSET,
        y + h - INPUT_INSET - INPUT,
        w - 2 * INPUT_INSET,
        INPUT,
    );
    PaneParts {
        header: Rect::new(x, y, w, HEADER),
        mark: Rect::new(x + MARK.0, y + MARK.1, MARK.2, MARK.2),
        close: Rect::new(x + w - 40, y + 8, ITEM, ITEM),
        history: Rect::new(x + INSET, y + 56, w - 2 * INSET, h - 56 - 104),
        status,
        stop: Rect::new(
            x + w - INSET - STOP.0,
            status.y + (STATUS - STOP.1) / 2,
            STOP.0,
            STOP.1,
        ),
        input,
        send: Rect::new(
            input.x + input.w - 6 - ITEM,
            input.y + (INPUT - ITEM) / 2,
            ITEM,
            ITEM,
        ),
    }
}
