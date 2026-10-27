//! The kernel console's colours ("Ember", #17): the dark palette's tokens under
//! the console's own names, and the row tints only the console uses (an answer,
//! an error and a question each get a tinted row and a bar in the margin). The
//! console stays the boot, panic and recovery screen (ADR 0008).

use crate::DARK;

/// The screen.
pub const INK: u32 = DARK.desk;
/// The status bar, and the line under it.
pub const BAR: u32 = DARK.window;
pub const RULE: u32 = DARK.line;
/// What the user types, and the status bar's own words.
pub const PAPER: u32 = DARK.text;
/// Kernel output.
pub const SLATE: u32 = DARK.muted;
/// Log tags and secondary status text.
pub const DIM: u32 = DARK.dim;
/// The accent: prompt, cursor, the AI's bar and the busy sign.
pub const EMBER: u32 = DARK.ai;
pub const CORAL: u32 = DARK.error;
pub const GOLD: u32 = DARK.ask;
pub const SAGE: u32 = DARK.ok;

/// An answer's text and the tint of its row.
pub const REPLY_TEXT: u32 = 0xF6_F0E6;
pub const REPLY_TINT: u32 = 0x1A_1611;
/// An error's text and row tint.
pub const ERROR_TEXT: u32 = 0xFF_B4AB;
pub const ERROR_TINT: u32 = 0x20_1316;
/// A question's text and row tint.
pub const QUESTION_TEXT: u32 = 0xF8_DF9F;
pub const QUESTION_TINT: u32 = 0x1D_1A10;
