//! The points the kernel selftests and xtask cases click and read, computed
//! from [`Layout`], so a layout change moves them too. Each is a point inside
//! its target, not on its edge.
//!
//! Window points name a window by its cascade spot: `n` windows were open
//! before it, so the first window is spot 0. A window's frame width is the
//! app's content width plus `2 * FRAME`.

use crate::{FRAME, Layout, Point, Rect, Size, TITLE_BAR, UiState, content_of, controls};

/// A relative pointer move far enough to pin the pointer to the screen's
/// bottom-right corner from anywhere.
const FAR: i32 = 10_000;

/// How far down a title-bar point sits from the frame's top: inside the bar,
/// and above the next cascade spot's frame, so the next window never covers it.
const TITLE_DOWN: i32 = 8;
/// The points of a `width` x `height` desk with nothing but windows open.
pub const fn points(width: i32, height: i32) -> Points {
    Points {
        layout: Layout::for_screen(width, height, UiState { pane_open: false }),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Points {
    pub layout: Layout,
}

impl Points {
    /// The middle of the screen, where `desktop` starts the pointer.
    pub const fn screen_centre(&self) -> (i32, i32) {
        (self.layout.width / 2, self.layout.height / 2)
    }

    /// The Kai corner, the screen's bottom-right pixel, where [`Points::pin`]
    /// leaves the pointer: a click there opens the Kai pane and focuses its
    /// input, from anywhere.
    pub const fn focus_kai(&self) -> (i32, i32) {
        self.pinned()
    }

    /// The pane's input, once the pane is open: a click there keeps the
    /// focus in it.
    pub const fn pane_input(&self) -> (i32, i32) {
        middle(self.layout.pane_parts().input)
    }

    /// The pane's close item, once the pane is open: a click there closes
    /// it at every size (Esc in its input only leaves it at 1280 and up,
    /// where it sits beside the windows).
    pub const fn pane_close(&self) -> (i32, i32) {
        middle(self.layout.pane_parts().close)
    }

    /// The middle of the Kai button, the taskbar's first item, with `n`
    /// items on the taskbar (the Kai button and one a running app).
    pub const fn kai_button(&self, n: i32) -> (i32, i32) {
        let l = &self.layout;
        middle(crate::taskbar::item(l.width, l.height, 0, n))
    }

    /// The relative move that pins the pointer to the screen's bottom-right
    /// corner, wherever it was: for a selftest that does not know where the
    /// pointer is. [`Points::pinned`] is where it lands.
    pub const fn pin(&self) -> (i32, i32) {
        (FAR, FAR)
    }

    /// Where [`Points::pin`] leaves the pointer: the screen's last pixel.
    pub const fn pinned(&self) -> (i32, i32) {
        (self.layout.width - 1, self.layout.height - 1)
    }

    /// The frame of the window at cascade spot `n`, `frame` in size.
    pub const fn frame(&self, n: i32, frame: Size) -> Rect {
        let at = self.layout.cascade(n);
        Rect::new(at.x, at.y, frame.w, frame.h)
    }

    /// A point in the title bar of the window at spot `n`, `across` px from
    /// its frame's left edge. The caller keeps `across` clear of the controls.
    pub const fn window_title(&self, n: i32, across: i32) -> (i32, i32) {
        let at = self.layout.cascade(n);
        (at.x + across, at.y + TITLE_DOWN)
    }

    /// A point in the content of the window at spot `n`, `(dx, dy)` from the
    /// content's top-left.
    pub const fn content(&self, n: i32, dx: i32, dy: i32) -> (i32, i32) {
        let c = content_of(self.bar_frame(n, 2 * FRAME));
        (c.x + dx, c.y + dy)
    }

    /// The middle of the close control of the window at spot `n` whose frame
    /// is `frame_w` wide.
    pub const fn close(&self, n: i32, frame_w: i32) -> (i32, i32) {
        middle(controls(self.bar_frame(n, frame_w)).close)
    }

    /// The middle of the maximize control, as [`Points::close`].
    pub const fn maximize(&self, n: i32, frame_w: i32) -> (i32, i32) {
        middle(controls(self.bar_frame(n, frame_w)).maximize)
    }

    /// The middle of the minimize control, as [`Points::close`].
    pub const fn minimize(&self, n: i32, frame_w: i32) -> (i32, i32) {
        middle(controls(self.bar_frame(n, frame_w)).minimize)
    }

    /// A point of the empty desk: inside the work area, clear of every window
    /// (the first opens at the cascade origin, right and below it) and of the
    /// pointer at the screen's middle.
    pub const fn desk_point(&self) -> (i32, i32) {
        let area = self.layout.work_area;
        (area.x + 16, area.y + 16)
    }

    /// The part of the screen whose pixels change while the desk is still,
    /// for a screendump golden to mask: (x, y, w, h).
    pub const fn tray_mask(&self) -> Rect {
        self.layout.tray.area
    }

    /// A frame at spot `n`, `frame_w` wide and just tall enough for its title
    /// bar: the controls and the content's top-left depend on nothing else.
    const fn bar_frame(&self, n: i32, frame_w: i32) -> Rect {
        self.frame(
            n,
            Size {
                w: frame_w,
                h: TITLE_BAR + 2 * FRAME,
            },
        )
    }
}

const fn middle(r: Rect) -> (i32, i32) {
    (r.x + r.w / 2, r.y + r.h / 2)
}

/// `p` as a [`Point`].
pub const fn point(p: (i32, i32)) -> Point {
    Point { x: p.0, y: p.1 }
}
