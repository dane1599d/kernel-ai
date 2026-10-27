//! "Ember": warm text on blue-black ink, with the AI's voice in ember orange. Every
//! colour has a job: the prompt and the AI's answers share the accent, errors are
//! coral, questions gold, and kernel output steps back in slate so the conversation
//! reads first. Each text colour keeps at least 4.5:1 contrast on its background.

use termcore::style::{Kind, Tone};

// Every colour comes from `look` (ADR 0005 §8), so the console and the desktop
// share one palette and `look`'s tests hold the contrast claim above.
pub use look::console::{BAR, CORAL, DIM, EMBER, GOLD, INK, PAPER, RULE, SAGE, SLATE};
use look::console::{ERROR_TEXT, ERROR_TINT, QUESTION_TEXT, QUESTION_TINT, REPLY_TEXT, REPLY_TINT};

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
