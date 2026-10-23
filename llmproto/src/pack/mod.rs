//! The skill pack (ADR 0010): what the model reads about the OS, rendered from
//! the action registry (`agentabi`) so it cannot drift from the code. A
//! [`Format`] is one renderer:
//!
//! - [`Format::M8`] is today's script prompt, byte for byte
//!   ([`crate::script::system_prompt`]), the control arm with its fluency
//!   baseline;
//! - [`Format::Dsl1`] is one compact line per row (`name(arg: T) -> R [undo]  //
//!   doc`) after a Rhai card and worked examples in the tagged call, checked
//!   against its golden (`dsl-1.txt`).
//!
//! The OS keeps sending [`Format::M8`] until spike S10-1 picks the default, and
//! task 6's tagged decoder can answer in `dsl-1`'s call. The pack and its `act`
//! MODE turn must fit [`PACK_BUDGET`] tokens; the host test holds the
//! estimate to it with [`estimate_tokens`]'s margin, and the token-count
//! workflow counts the real tokens (`cargo xtask tokens`).

mod dsl1;
#[cfg(test)]
mod tests;

use agentabi::{Action, Owner, Tier};
use alloc::string::String;
use alloc::vec::Vec;

/// ADR 0010 §4 and ADR 0002 D4: core rules plus the OS rows, MODE included.
pub const PACK_BUDGET: usize = 1024;
/// Bytes per token for the estimate: S10-0's real count of the draft `dsl-1`
/// pack (2,682 bytes in 807 tokens, 3.32 a token, run 39842386906) less 10%.
const BYTES_PER_TOKEN: f32 = 3.32 * 0.9;

/// The `act` MODE turn, a second system turn after the pack (ADR 0010 §3).
pub const MODE_ACT: &str =
    "Mode: act. Answer with exactly one call from the pack: a row, script, say or clarify.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    M8,
    Dsl1,
}

impl Format {
    pub const ALL: [Self; 2] = [Self::M8, Self::Dsl1];

    pub const fn name(self) -> &'static str {
        match self {
            Self::M8 => "m8",
            Self::Dsl1 => "dsl-1",
        }
    }
}

/// The rows a pack lists: every OS row the model may call, so not the shell's
/// pure helpers (the Rhai card teaches `fixed`) and nothing never-tier.
pub fn rows() -> Vec<&'static Action> {
    agentabi::ROWS
        .iter()
        .filter(|row| row.owner != Owner::Shell && agentabi::derive(row) != Tier::Never)
        .collect()
}

/// The pack's text in `format`.
pub fn render(format: Format) -> String {
    match format {
        Format::M8 => crate::script::system_prompt(),
        Format::Dsl1 => dsl1::render(&rows()),
    }
}

/// A token count of pack text, at S10-0's rate with a 10% margin. The
/// token-count workflow checks it against the real count.
pub fn estimate_tokens(text: &str) -> usize {
    (text.len() as f32 / BYTES_PER_TOKEN) as usize + 1
}

/// `pack_id` (ADR 0010 §7): FNV-1a 64 over the format's name, the hex SHA-256
/// of the model's tokenizer arrays and the rendered bytes. A label for reports
/// and logs, not a security check.
pub fn pack_id(format: Format, tokenizer_sha256: &str, pack: &str) -> u64 {
    let mut fnv = kabi::model::Fnv1a::new();
    for part in [format.name(), tokenizer_sha256, pack] {
        fnv.update(part.as_bytes());
    }
    fnv.finish()
}
