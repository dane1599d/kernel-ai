//! Metrics, ADR 0005 §8 (amended 2026-11-14, "Kai Desk"): the 4 px grid, the radii
//! and the sizes of the desktop's chrome, in logical px.
//!
//! Colours and type are in the crate root, motion in [`crate::motion`]; these are
//! the numbers every surface of the desktop is laid out from, so a layout reads
//! them here and never writes a literal. Nothing reads them yet: the layout crate
//! that does comes in a later change, and no pixel moves with this one.

/// The grid every size below sits on, except the few that say why they do not.
pub const GRID: u32 = 4;

/// The spacing scale: gaps, paddings and margins are one of these.
pub const SPACE: [u32; 7] = [4, 8, 12, 16, 24, 32, 48];

/// Tiles, hover fills, tooltips, app controls and the ask card's buttons.
pub const RADIUS_SM: u32 = 4;
/// The Kai pane's input field. Off the grid on purpose: it sits inside a
/// [`RADIUS_MD`] pane with a 12 px margin, and 6 is what reads as concentric.
pub const RADIUS_FIELD: u32 = 6;
/// Windows, the menu, the Kai pane and the ask card.
pub const RADIUS_MD: u32 = 8;

/// A pill's radius: half its height.
pub const fn pill(height: u32) -> u32 {
    height / 2
}

/// A window's corner radius: [`RADIUS_MD`], square while it fills the work area.
pub const fn window_radius(maximized: bool) -> u32 {
    match maximized {
        true => 0,
        false => RADIUS_MD,
    }
}

/// The taskbar's height, at every screen size.
pub const TASKBAR: u32 = 48;
/// A taskbar item: 40 by 40, centred in the 48 px bar, so 4 px above and below.
pub const ITEM: u32 = 40;
/// The gap between taskbar items.
pub const ITEM_GAP: u32 = 4;
/// An icon in the taskbar.
pub const ICON_BAR: u32 = 24;
/// An icon on a menu tile.
pub const ICON_TILE: u32 = 32;
/// An icon in a window's title row.
pub const ICON_TITLE: u32 = 16;
/// How far a flyout (the menu, the Kai pane, the card) keeps from a screen edge
/// and from the taskbar.
pub const FLYOUT_MARGIN: u32 = 12;

/// A window's title bar.
pub const TITLE_BAR: u32 = 32;
/// A window control's width: close, maximize and minimize sit flush in the
/// title bar.
pub const CONTROL_W: u32 = 40;
/// A window control's height: the title bar's, so a control fills it.
pub const CONTROL_H: u32 = 32;
/// A window's frame.
pub const FRAME: u32 = 1;
/// How far a resize grab reaches outside the frame. Off the grid: it is what a
/// pointer can hit, not what is drawn.
pub const GRAB: u32 = 6;
/// A resize grab at a corner is this square.
pub const GRAB_CORNER: u32 = 16;
/// How far a shadow reaches, so how much damage a moved window adds. It is
/// split from [`GRAB`]: the shadow is wider than the grab.
pub const SHADOW_REACH: u32 = 12;

// How the numbers fit one another. A change that breaks one fails the build, not
// a screen.
const _: () = {
    // An item centred in the bar leaves the gap the items keep between them.
    assert!((TASKBAR - ITEM) / 2 == ITEM_GAP);
    // Icons fit their places, and the controls fill the title bar.
    assert!(ICON_BAR <= ITEM && ICON_TILE <= ITEM && ICON_TITLE < TITLE_BAR);
    assert!(CONTROL_H == TITLE_BAR);
    // The shadow reaches past the grab, and the grab past a corner's square.
    assert!(SHADOW_REACH > GRAB && GRAB < GRAB_CORNER);
    // Radii grow with the surface.
    assert!(RADIUS_SM < RADIUS_FIELD && RADIUS_FIELD < RADIUS_MD);
};
