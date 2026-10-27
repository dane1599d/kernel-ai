//! The window frame `desktop` draws around every window (ADR 0005 §8):
//! a 32 px title bar, three 28x28 controls, a 1 px frame, and resize grabs
//! reaching 6 px outside it, 16x16 at the corners. Only `desktop` draws it, so
//! an app cannot pose as another app or as the system.

use crate::geom::{Point, Rect};

pub const TITLE_BAR: i32 = 32;
pub const FRAME: i32 = 1;
pub const CONTROL: i32 = 28;
/// The gap from a control to the next and to the frame.
pub const CONTROL_GAP: i32 = 2;
pub const GRAB: i32 = 6;
pub const CORNER: i32 = 16;
pub const RADIUS: i32 = 6;
/// The top bar and the AI panel along the bottom.
pub const TOP_BAR: i32 = 28;
pub const PANEL: i32 = 36;

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

/// The outer frame for `content`: the title bar above it and the frame round
/// both.
pub fn frame_for(content: Rect) -> Rect {
    Rect::new(
        content.x - FRAME,
        content.y - TITLE_BAR - FRAME,
        content.w + 2 * FRAME,
        content.h + TITLE_BAR + 2 * FRAME,
    )
}

/// The content rectangle inside `frame`.
pub fn content_of(frame: Rect) -> Rect {
    Rect::new(
        frame.x + FRAME,
        frame.y + FRAME + TITLE_BAR,
        frame.w - 2 * FRAME,
        frame.h - TITLE_BAR - 2 * FRAME,
    )
}

pub fn title_bar(frame: Rect) -> Rect {
    Rect::new(
        frame.x + FRAME,
        frame.y + FRAME,
        frame.w - 2 * FRAME,
        TITLE_BAR,
    )
}

/// The three controls, right to left: close, maximize, minimize.
pub fn controls(frame: Rect) -> [(Part, Rect); 3] {
    let bar = title_bar(frame);
    let y = bar.y + (TITLE_BAR - CONTROL) / 2;
    let at = |n: i32| bar.right() - CONTROL_GAP - (n + 1) * CONTROL - n * CONTROL_GAP;
    [
        (Part::Close, Rect::new(at(0), y, CONTROL, CONTROL)),
        (Part::Maximize, Rect::new(at(1), y, CONTROL, CONTROL)),
        (Part::Minimize, Rect::new(at(2), y, CONTROL, CONTROL)),
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
