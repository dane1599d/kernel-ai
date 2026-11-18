//! What a finished turn is remembered as (#831): a reply that showed
//! Content is kept without its text.

use super::read_from;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Longest result line llm keeps for a turn.
const RESULT_MAX_BYTES: usize = 300;

/// The remembered result of a turn that showed Content, before its sources.
const SHOWED_LEAD: &str = "Showed what Kai read from ";

/// What the last finished script showed, for its turn's memory (#831): the
/// sources of the Content `show` printed, each once, and the line the shell
/// wrote after that output, if it wrote one (`Done: …`, `Nothing changed: …`,
/// what was not sent, the engine's error). `withheld` when the run failed on
/// an app's reply: the reply's text carries the app's words, so the turn is
/// remembered by `outcome`, the shell's own line, shown or not.
#[derive(Debug, Default)]
pub(crate) struct Showed {
    pub from: Vec<&'static str>,
    pub outcome: Option<String>,
    pub withheld: bool,
}

/// Sends a finished model turn to llm's conversation memory: the user's line, the
/// call as `act` returned it, and the result line, its last line of at most 300
/// bytes (`Done: …`, `Taken as: …`, `Refused …`, `Not run: …`).
///
/// A reply that printed Content, from the sources in `showed`, is the person's
/// to read: later prompts would carry it as an unlabelled `Result:` line, so a
/// fixed line naming its sources stands in for it (#831), then the line the
/// shell wrote after it, so a later turn still knows what changed or failed.
/// A run an app's reply failed is remembered by that line alone, never by
/// what the app said (`<row>: the app refused`).
pub(crate) fn remember(
    host: &dyn crate::Host,
    line: &str,
    call: &str,
    reply: &str,
    showed: &Showed,
) {
    let result = match (showed.from.is_empty(), showed.withheld) {
        (false, _) => showed_line(showed),
        (true, true) => showed.outcome.as_deref().map_or("", last_line).to_string(),
        (true, false) => last_line(reply).to_string(),
    };
    host.remember(line, call, capped(&result));
}

/// The fixed line for a turn that showed Content: its sources, then the
/// shell's own line after the output, never the output.
fn showed_line(showed: &Showed) -> String {
    let from: Vec<String> = showed
        .from
        .iter()
        .map(|source| shown_from(source))
        .collect();
    let lead = format!("{SHOWED_LEAD}{}", from.join(", "));
    match showed.outcome.as_deref().map(last_line) {
        Some(outcome) if !outcome.is_empty() => format!("{lead}. {outcome}"),
        _ => lead,
    }
}

/// `text`'s last line that is not blank, or "".
fn last_line(text: &str) -> &str {
    text.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
}

/// `text` cut to [`RESULT_MAX_BYTES`] on a character boundary.
fn capped(text: &str) -> &str {
    &text[..text.floor_char_boundary(RESULT_MAX_BYTES)]
}

/// Where shown Content came from, in words. A query over text the script
/// wrote itself is labelled `query`, which names no app.
fn shown_from(source: &str) -> String {
    match source {
        "query" => String::from("an answer to a question about text it wrote"),
        source => read_from(source),
    }
}
