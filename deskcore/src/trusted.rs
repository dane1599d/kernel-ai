//! The trusted layer's geometry (ADR 0005 §7, §8): the ask card and the
//! agent's outline, which `desktop` draws above every app window. The rules,
//! who gets keys and clicks while a card is up and when the user takes over,
//! are [`crate::desk::Desk`]'s.

use crate::chrome::PANEL;
use crate::geom::Rect;

/// The widest the sheet an ask card sits in grows.
pub const SHEET_MAX: i32 = 960;
/// A line of the card's text: mono 14/20.
pub const LINE: i32 = 20;
pub const PADDING: i32 = 16;
pub const BUTTON_W: i32 = 120;
pub const BUTTON_H: i32 = 32;
/// The agent's outline: 2 px wide, 2 px outside the window's frame.
pub const OUTLINE: i32 = 2;
pub const OUTLINE_GAP: i32 = 2;

/// An ask card on screen: the question the menu shows, never anything an app
/// drew.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ask {
    /// The log entry the card asks for (`actlog 1`), which its answer names.
    pub id: u64,
    /// Lines of text the card shows.
    pub lines: i32,
}

/// The card's rectangle on a `width` x `height` screen: centred, above the
/// panel, at most half the screen tall.
pub fn card(width: i32, height: i32, ask: &Ask) -> Rect {
    let w = (width - 2 * PADDING).min(SHEET_MAX);
    let wanted = PADDING * 3 + ask.lines.max(1) * LINE + BUTTON_H;
    let h = wanted.min((height - PANEL) / 2);
    Rect::new((width - w) / 2, height - PANEL - h, w, h)
}

/// The Run (y) and Don't (n) buttons, bottom right of the card.
pub fn buttons(card: Rect) -> (Rect, Rect) {
    let y = card.bottom() - PADDING - BUTTON_H;
    let no = Rect::new(card.right() - PADDING - BUTTON_W, y, BUTTON_W, BUTTON_H);
    let yes = Rect::new(no.x - PADDING / 2 - BUTTON_W, y, BUTTON_W, BUTTON_H);
    (yes, no)
}

/// What the agent's outline covers round a window's `frame`.
pub fn outline(frame: Rect) -> Rect {
    frame.grow(OUTLINE + OUTLINE_GAP)
}
