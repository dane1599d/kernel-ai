//! "Ember": warm text on blue-black ink, with the AI's voice in ember orange. Every
//! colour has a job: the prompt and the AI's answers share the accent, errors are
//! coral, questions gold, and kernel output steps back in slate so the conversation
//! reads first. Each text colour keeps at least 4.5:1 contrast on its background.

use termcore::style::{Kind, Tone};

/// The screen.
pub const INK: u32 = 0x000D_1017;
/// The status bar, and the line under it.
pub const BAR: u32 = 0x0014_1821;
pub const RULE: u32 = 0x0023_2A38;
/// What the user types, and the status bar's own words.
pub const PAPER: u32 = 0x00EC_E6DA;
/// Kernel output.
pub const SLATE: u32 = 0x009A_A3B4;
/// Log tags and secondary status text.
pub const DIM: u32 = 0x007C_8AA8;
/// The accent: prompt, cursor, the AI's bar and the busy sign.
pub const EMBER: u32 = 0x00FF_9F5A;
pub const CORAL: u32 = 0x00FF_6B6B;
pub const GOLD: u32 = 0x00F5_C451;
pub const SAGE: u32 = 0x007F_D49B;

const REPLY_TEXT: u32 = 0x00F6_F0E6;
const REPLY_TINT: u32 = 0x001A_1611;
const ERROR_TEXT: u32 = 0x00FF_B4AB;
const ERROR_TINT: u32 = 0x0020_1316;
const QUESTION_TEXT: u32 = 0x00F8_DF9F;
const QUESTION_TINT: u32 = 0x001D_1A10;

/// How a whole row looks: its background and, for the shell's answers, errors and
/// questions, a bar in the margin.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Look {
    pub background: u32,
    pub bar: Option<u32>,
}

/// The look of a row of `kind`. Only answers, errors and questions differ from
/// the plain screen, so most rows repaint just the cells that change.
pub fn look(kind: Kind) -> Look {
    let (background, bar) = match kind {
        Kind::Reply | Kind::Ai => (REPLY_TINT, Some(EMBER)),
        Kind::Error => (ERROR_TINT, Some(CORAL)),
        Kind::Question => (QUESTION_TINT, Some(GOLD)),
        _ => (INK, None),
    };
    Look { background, bar }
}

/// A cell's colour and whether it is bold; None for a lead the bar replaces.
pub fn ink(tone: Tone) -> Option<(u32, bool)> {
    Some(match tone {
        Tone::Text => (PAPER, false),
        Tone::Muted => (SLATE, false),
        Tone::Tag => (DIM, false),
        Tone::Prompt | Tone::Banner => (EMBER, true),
        Tone::Reply => (REPLY_TEXT, false),
        Tone::Error => (ERROR_TEXT, false),
        Tone::Question => (QUESTION_TEXT, false),
        Tone::Success => (SAGE, false),
        Tone::Lead => return None,
    })
}
