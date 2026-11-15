//! The taskbar and its tray (ADR 0005 §8): the 48 px strip along the
//! bottom, its centred group of 40x40 items (the Kai button first, then the
//! apps), and on the right the Kai chip, the memory, the UTC clock and the
//! Kai corner. Under 1280 wide the tray is compact: a 64 px chip and no
//! memory. Every part is a pure function of the screen's size, so the
//! screendump mask is computed, not written down.

use crate::{PANE_BESIDE, Rect, TASKBAR};

/// An item's side.
pub const ITEM: i32 = 40;
/// Between items; each item's hit box takes half of it either side.
pub const ITEM_GAP: i32 = 4;
/// The items' top, from the taskbar's.
pub const ITEM_TOP: i32 = 4;
/// The group's least distance from the screen's left and from the tray.
pub const GROUP_MARGIN: i32 = 12;
/// The most items, the Kai button and a "more" item included: 12 at 1280
/// wide and up, 9 under it.
pub const fn max_items(width: i32) -> usize {
    if width >= PANE_BESIDE { 12 } else { 9 }
}

/// The tray's parts on a `width` x `height` screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tray {
    /// Hit only: a click opens the Kai pane. It holds (W-1, H-1), where a
    /// pointer pushed into the corner lands.
    pub corner: Rect,
    /// The clock: two lines, 16 px column-aligned at every size.
    pub clock: Rect,
    /// The memory in use; none in the compact tray.
    pub memory: Option<Rect>,
    /// The Kai chip.
    pub chip: Rect,
    /// Everything right of the group whose pixels change while the desk is
    /// still: the screendump mask.
    pub area: Rect,
}

/// The tray at a `width` x `height` screen.
pub const fn tray(width: i32, height: i32) -> Tray {
    let top = height - TASKBAR;
    let compact = width < PANE_BESIDE;
    let (memory, chip, area) = match compact {
        true => (
            None,
            Rect::new(width - 168, top + 4, 64, 40),
            Rect::new(width - 176, top, 176, TASKBAR),
        ),
        false => (
            Some(Rect::new(width - 192, top + 4, 88, 40)),
            Rect::new(width - 360, top + 4, 160, 40),
            Rect::new(width - 368, top, 368, TASKBAR),
        ),
    };
    Tray {
        corner: Rect::new(width - 16, top, 16, TASKBAR),
        clock: Rect::new(width - 96, top, 80, TASKBAR),
        memory,
        chip,
        area,
    }
}

/// The group's width for `n` items.
pub const fn group_width(n: i32) -> i32 {
    if n <= 0 {
        return 0;
    }
    n * ITEM + (n - 1) * ITEM_GAP
}

/// The group's left edge for `n` items: centred, kept 12 from the screen's
/// left and from the tray.
pub const fn group_x(width: i32, height: i32, n: i32) -> i32 {
    let w = group_width(n);
    let centred = (width - w) / 2;
    let most = tray(width, height).area.x - GROUP_MARGIN - w;
    let x = if centred > most { most } else { centred };
    if x < GROUP_MARGIN { GROUP_MARGIN } else { x }
}

/// Item `i` of `n`, 40x40 at y H-44.
pub const fn item(width: i32, height: i32, i: i32, n: i32) -> Rect {
    let x = group_x(width, height, n) + i * (ITEM + ITEM_GAP);
    Rect::new(x, height - TASKBAR + ITEM_TOP, ITEM, ITEM)
}

/// Item `i` of `n`'s hit box: the whole 48 px height, the gaps split between
/// neighbours, so no pixel of the group is dead.
pub const fn item_hit(width: i32, height: i32, i: i32, n: i32) -> Rect {
    let r = item(width, height, i, n);
    Rect::new(
        r.x - ITEM_GAP / 2,
        height - TASKBAR,
        ITEM + ITEM_GAP,
        TASKBAR,
    )
}
