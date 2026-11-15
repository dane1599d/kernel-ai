//! The Kai menu (Start in the code; ADR 0005 §8) and the taskbar's tooltip.
//! Start is 640 wide and min(680, H-72) tall, centred, its bottom 12 above
//! the taskbar. Its parts are offsets from its top-left; the padding is 24.

use crate::{FLYOUT_MARGIN, Rect, TASKBAR};

/// Start's width and tallest height.
pub const WIDTH: i32 = 640;
pub const MAX_H: i32 = 680;
/// The padding inside it.
pub const PAD: i32 = 24;
/// The pinned grid: 6 columns of 3 rows of 96x84 cells from (32, 116).
pub const GRID_COLS: i32 = 6;
pub const GRID_ROWS: i32 = 3;
pub const CELL: (i32, i32) = (96, 84);
pub const GRID_AT: (i32, i32) = (32, 116);
/// Recent: up to 5 rows of 40 from y 408.
pub const RECENT_ROWS: i32 = 5;
pub const RECENT_AT: i32 = 408;
pub const RECENT_H: i32 = 40;
/// Search: rows of 52 from y 80, 10 of them; All apps: rows of 52 from
/// y 112 under a 30 px header, 9 of them.
pub const ROW_H: i32 = 52;
pub const SEARCH_AT: i32 = 80;
pub const SEARCH_ROWS: i32 = 10;
pub const ALL_AT: i32 = 112;
pub const ALL_ROWS: i32 = 9;
/// The footer band.
pub const FOOTER_H: i32 = 64;
/// A tooltip: 28 tall, at most 320 wide, 8 above the taskbar.
pub const TIP_H: i32 = 28;
pub const TIP_MAX_W: i32 = 320;
pub const TIP_UP: i32 = 8;

/// Start on a `width` x `height` screen.
pub const fn start(width: i32, height: i32) -> Rect {
    let most = height - TASKBAR - 2 * FLYOUT_MARGIN;
    let h = if MAX_H < most { MAX_H } else { most };
    let bottom = height - TASKBAR - FLYOUT_MARGIN;
    Rect::new((width - WIDTH) / 2, bottom - h, WIDTH, h)
}

/// Start's parts, in screen px.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartParts {
    pub area: Rect,
    /// The "Ask Kai or search apps" box.
    pub field: Rect,
    /// "Pinned", and its "All apps ›" link at the right.
    pub pinned: Rect,
    pub all_apps: Rect,
    /// "Recent".
    pub recent: Rect,
    /// The footer band and its Settings item.
    pub footer: Rect,
    pub settings: Rect,
}

/// The parts of Start at `area`.
pub const fn parts(area: Rect) -> StartParts {
    let (x, y, w, h) = (area.x, area.y, area.w, area.h);
    let footer = Rect::new(x, y + h - FOOTER_H, w, FOOTER_H);
    StartParts {
        area,
        field: Rect::new(x + PAD, y + PAD, w - 2 * PAD, 40),
        pinned: Rect::new(x + PAD, y + 88, w - 2 * PAD, 24),
        all_apps: Rect::new(x + w - PAD - 88, y + 88, 88, 24),
        recent: Rect::new(x + PAD, y + 384, w - 2 * PAD, 24),
        footer,
        settings: Rect::new(x + w - PAD - 40, footer.y + 12, 40, 40),
    }
}

/// Pinned cell `i` (row by row) of Start at `area`.
pub const fn cell(area: Rect, i: i32) -> Rect {
    let (col, row) = (i % GRID_COLS, i / GRID_COLS);
    Rect::new(
        area.x + GRID_AT.0 + col * CELL.0,
        area.y + GRID_AT.1 + row * CELL.1,
        CELL.0,
        CELL.1,
    )
}

/// Recent row `i` of Start at `area`.
pub const fn recent_row(area: Rect, i: i32) -> Rect {
    Rect::new(
        area.x + PAD,
        area.y + RECENT_AT + i * RECENT_H,
        area.w - 2 * PAD,
        RECENT_H,
    )
}

/// Search row `i` of Start at `area`.
pub const fn search_row(area: Rect, i: i32) -> Rect {
    Rect::new(
        area.x + PAD,
        area.y + SEARCH_AT + i * ROW_H,
        area.w - 2 * PAD,
        ROW_H,
    )
}

/// All apps row `i` of Start at `area`.
pub const fn all_row(area: Rect, i: i32) -> Rect {
    Rect::new(
        area.x + PAD,
        area.y + ALL_AT + i * ROW_H,
        area.w - 2 * PAD,
        ROW_H,
    )
}

/// The box a tooltip for the taskbar item `item` may take: centred over it,
/// 8 above the taskbar, kept on the screen. A tooltip is drawn inside it,
/// and its damage is this box.
pub const fn tooltip(width: i32, height: i32, item: Rect) -> Rect {
    let w = if TIP_MAX_W < width { TIP_MAX_W } else { width };
    let mut x = item.x + item.w / 2 - w / 2;
    if x < 0 {
        x = 0;
    }
    if x + w > width {
        x = width - w;
    }
    Rect::new(x, height - TASKBAR - TIP_UP - TIP_H, w, TIP_H)
}
