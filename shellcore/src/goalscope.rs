//! A goal's scope and times, from the words of `goal_set` (G2): what the goal
//! may use is taken from the request, as a standing rule's scope is (P3), never
//! from a list the model wrote. The person sees it on the one ask and approves
//! exactly it; a step outside it asks.
//!
//! - **Rows**: every OS row a cue of which starts a word of the goal's text,
//!   except a goal row, a shell-owned row, a never row and one that sends or
//!   installs (`agentid::goalgrant::sends_or_installs`). Naming a folder adds the
//!   file reads, so the goal can see what it works on.
//! - **Folders**: each relative path written in the text (`notes/q3`), with a
//!   leading `~/` or `/home/user/` taken off. Nothing else is guessed.
//! - **Times**: `90m`, `2h` or `1d`. Anything else is refused, never guessed.

use agentabi::{Action, Owner, Tier};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// The file reads a goal that names a folder may make.
const FOLDER_READS: [&str; 3] = ["fs_list", "fs_find", "fs_read"];
/// What a goal that names nothing it does is told.
pub(crate) const NAMES_NOTHING: &str =
    "say what the goal does, for example \"sort the files in notes/ by topic\"";

/// The scope of a goal whose words are `text`, or why there is none.
pub(crate) fn scope(text: &str) -> Result<goalcore::Scope, String> {
    let words = plain(text);
    let folders = folders(text)?;
    let mut rows: Vec<String> = agentabi::ROWS
        .iter()
        .filter(|row| may_hold(row) && row.cues.iter().any(|cue| names(&words, cue)))
        .map(|row| row.name.to_string())
        .collect();
    if !folders.is_empty() {
        for read in FOLDER_READS {
            if !rows.iter().any(|row| row == read) {
                rows.push(read.to_string());
            }
        }
    }
    if rows.is_empty() {
        return Err(String::from(NAMES_NOTHING));
    }
    if rows.len() > goalcore::MAX_ROWS {
        return Err(String::from(
            "the goal names too many kinds of action; say it more narrowly",
        ));
    }
    Ok(goalcore::Scope { rows, folders })
}

/// Whether a goal's ceiling may hold `row` at all.
fn may_hold(row: &Action) -> bool {
    !row.name.starts_with("goal_")
        && row.owner != Owner::Shell
        && agentabi::derive(row) != Tier::Never
        && !agentid::goalgrant::sends_or_installs(row)
}

/// `text` lower-cased, every run of other characters one space, padded.
fn plain(text: &str) -> String {
    let mut out = String::from(" ");
    for c in text.chars() {
        match c.is_ascii_alphanumeric() {
            true => out.push(c.to_ascii_lowercase()),
            false if !out.ends_with(' ') => out.push(' '),
            false => {}
        }
    }
    if !out.ends_with(' ') {
        out.push(' ');
    }
    out
}

/// Whether cue `cue` is words of `words` ([`plain`]'s form), whole, or with a
/// plural `s`: "go" is not in "good".
fn names(words: &str, cue: &str) -> bool {
    let cue = plain(cue);
    let cue = cue.trim_end();
    !cue.trim().is_empty()
        && (words.contains(&format!("{cue} ")) || words.contains(&format!("{cue}s ")))
}

/// The folders written in `text`.
fn folders(text: &str) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        let word = word.trim_matches(|c: char| ",.;:!?\"'()".contains(c));
        if !word.contains('/') {
            continue;
        }
        let path = word
            .strip_prefix("~/")
            .or_else(|| word.strip_prefix("/home/user/"))
            .unwrap_or(word)
            .trim_end_matches('/');
        let plain_path = !path.is_empty()
            && !path.starts_with('/')
            && path
                .split('/')
                .all(|p| !p.is_empty() && p != "." && p != "..");
        if !plain_path {
            return Err(format!("{word} is not a folder in your home folder"));
        }
        if !out.iter().any(|f| f == path) {
            out.push(path.to_string());
        }
    }
    if out.len() > goalcore::MAX_FOLDERS {
        return Err(format!(
            "a goal names at most {} folders",
            goalcore::MAX_FOLDERS
        ));
    }
    Ok(out)
}

/// `90m`, `2h` or `1d`, in milliseconds.
pub(crate) fn duration_ms(text: &str) -> Result<u64, String> {
    let text = text.trim();
    let refused = || format!("{text:?}: write a time as 90m, 2h or 1d");
    let unit = text.chars().last().ok_or_else(refused)?;
    let ms = match unit {
        'm' => 60_000,
        'h' => 3_600_000,
        'd' => 86_400_000,
        _ => return Err(refused()),
    };
    let n: u64 = text[..text.len() - 1].parse().map_err(|_| refused())?;
    match n {
        0 => Err(refused()),
        n => n.checked_mul(ms).ok_or_else(refused),
    }
}
