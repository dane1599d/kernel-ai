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

pub use dsl1::INDEX_HEADING;
#[cfg(test)]
mod tests;

use agentabi::{Action, Owner, Tier};
use alloc::string::String;
use alloc::vec::Vec;

/// ADR 0010 §4 and ADR 0002 D4: core rules plus the OS rows, MODE included.
pub const PACK_BUDGET: usize = 1024;
/// Bytes per token for the estimate. Measured: 1,005 real tokens for pack+MODE
/// at 1,013 estimated with the file rows in (token-count run 40462104781); before
/// them, 890 real at 935 (run 40443905553). The guard stays just above the real
/// count, which the token-count workflow checks.
const BYTES_PER_TOKEN: f32 = 3.46;

/// ADR 0010 §6's index level, in v0.1 since P2's rows filled the budget:
/// groups the pack names on one line each (`tune: tune tune_undo`). A row
/// here has no full line in the pack; an `act` call that names one with
/// arguments is decoded once more with [`group_text`] after the request
/// (`local::act`). Scripts are written by the script prompt, which keeps
/// every function, so they are untouched. The least-used row: the component
/// revert. Reads stay in full: their return types are how `act` tells a
/// question `script` can answer from one it cannot (run 40486342534 indexed
/// them, and M8 through script fell from 47 to 40 of 50). So do the scheduler
/// trial's rows: from a bare `tune` the model asks how to tune instead of
/// calling it (run 40503318565, the os arm at 11 of 13).
pub const INDEX: &[(&str, &[&str])] = &[("component", &["component_revert"])];

/// The index group `name` is in, if it is index-only.
pub fn indexed(name: &str) -> Option<(&'static str, &'static [&'static str])> {
    INDEX
        .iter()
        .find(|(_, names)| names.contains(&name))
        .copied()
}

/// The full lines of `group`'s rows, as a system turn after the request: what
/// a call naming one of them is decoded with the second time.
pub fn group_text(group: &str) -> String {
    let rows = rows();
    let members: Vec<&Action> = INDEX
        .iter()
        .filter(|(name, _)| *name == group)
        .flat_map(|(_, names)| names.iter())
        .filter_map(|name| rows.iter().copied().find(|row| row.name == *name))
        .collect();
    let mut text = alloc::format!("## {group}\n");
    for line in dsl1::row_lines(&members) {
        text.push_str(&line);
        text.push('\n');
    }
    text
}

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

/// A token count of pack text, at the measured rate, above the real count. The
/// token-count workflow checks the real count.
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

/// The `act` pack's `pack_id`: dsl-1 as rendered now, with no tokenizer hash,
/// so llm (its `kai-kv 1` key) and the shell (each model turn's `actlog 1`
/// entry) name the same pack the same way.
pub fn act_pack_id() -> u64 {
    pack_id(Format::Dsl1, "", &render(Format::Dsl1))
}

/// Whether `pack` shows the row `name`: on its own line, as one verb of its
/// family's line (`service_stop|start|restart(`), or by name on an index line.
pub fn shows(pack: &str, name: &str) -> bool {
    let index = pack
        .split_once(dsl1::INDEX_HEADING)
        .map_or("", |(_, index)| index);
    let listed = index.lines().any(|line| {
        line.split_once(": ")
            .is_some_and(|(_, names)| names.split(' ').any(|n| n == name))
    });
    listed
        || pack.lines().any(|line| {
            let head = line.split('(').next().unwrap_or("");
            head == name
                || name.split_once('_').is_some_and(|(family, verb)| {
                    head.strip_prefix(family)
                        .and_then(|rest| rest.strip_prefix('_'))
                        .is_some_and(|verbs| verbs.split('|').any(|v| v == verb))
                })
        })
}
