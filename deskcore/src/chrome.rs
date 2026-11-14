//! The window frame `desktop` draws around every window (ADR 0005 §8):
//! a 32 px title bar, three 28x28 controls, a 1 px frame, and resize grabs
//! reaching 6 px outside it, 16x16 at the corners. Only `desktop` draws it, so
//! an app cannot pose as another app or as the system.

use crate::geom::{Point, Rect};

pub use deskgeom::{
    CONTROL, CONTROL_GAP, CORNER, FRAME, GRAB, PANEL, TITLE_BAR, TOP_BAR, content_of, frame_for,
    title_bar,
};
pub const RADIUS: i32 = 6;

/// Which edges a resize moves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Edges {
    pub left: bool,
    pub right: bool,
    pub top: bool,
    pub bottom: bool,
}

/// What a point on a window's frame hits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// The app's own pixels, at this point in content-local px.
    Content(Point),
    Title,
    Minimize,
    Maximize,
    Close,
    Edge(Edges),
}

/// The three controls, right to left: close, maximize, minimize.
pub fn controls(frame: Rect) -> [(Part, Rect); 3] {
    let c = deskgeom::controls(frame);
    [
        (Part::Close, c.close),
        (Part::Maximize, c.maximize),
        (Part::Minimize, c.minimize),
    ]
}

/// What `p` hits on a window whose frame is `frame`, or `None` past its grabs.
pub fn hit(frame: Rect, p: Point) -> Option<Part> {
    if !frame.grow(GRAB).contains(p) {
        return None;
    }
    let inside = frame.contains(p);
    // Outside the frame, or on its 1 px line: a resize grab.
    let edges = Edges {
        left: p.x < frame.x + FRAME,
        right: p.x >= frame.right() - FRAME,
        top: p.y < frame.y + FRAME,
        bottom: p.y >= frame.bottom() - FRAME,
    };
    if !inside || edges.left || edges.right || edges.top || edges.bottom {
        return Some(Part::Edge(corners(frame, p, edges)));
    }
    if let Some((part, _)) = controls(frame).into_iter().find(|(_, r)| r.contains(p)) {
        return Some(part);
    }
    if title_bar(frame).contains(p) {
        return Some(Part::Title);
    }
    let content = content_of(frame);
    Some(Part::Content(Point {
        x: p.x - content.x,
        y: p.y - content.y,
    }))
}

/// Widens an edge grab to a corner when `p` is within [`CORNER`] of one.
fn corners(frame: Rect, p: Point, mut edges: Edges) -> Edges {
    let close = |a: i32, b: i32| (a - b).abs() < CORNER;
    if edges.left || edges.right {
        edges.top |= close(p.y, frame.y);
        edges.bottom |= close(p.y, frame.bottom());
    }
    if edges.top || edges.bottom {
        edges.left |= close(p.x, frame.x);
        edges.right |= close(p.x, frame.right());
    }
    edges
}

#[cfg(test)]
mod tests {
    /// The stats the top bar paints are what the screendump goldens mask, so
    /// the kernel and xtask read the mask from deskgeom.
    #[test]
    fn the_painted_stats_are_deskgeoms_tray_mask() {
        for (w, h) in [(1920, 1080), (1280, 800), (1024, 768)] {
            assert_eq!(
                crate::paint::stats_area(w),
                deskgeom::points(w, h).tray_mask()
            );
        }
    }
}
