//! "Ember": warm text on blue-black ink, with the AI's voice in ember orange. Every
//! colour has a job: the prompt and the AI's answers share the accent, errors are
//! coral, questions gold, and kernel output steps back in slate so the conversation
//! reads first. Each text colour keeps at least 4.5:1 contrast on its background.
//!
//! Every colour comes from `look` (ADR 0005 §8), whose tests hold that claim, and
//! the row looks and cell inks are termcore's, shared with the desktop's terminal
//! window.

pub use look::console::{BAR, DIM, EMBER, GOLD, INK, PAPER, RULE, SAGE, SLATE};
pub use termcore::style::{Look, ink, look};
