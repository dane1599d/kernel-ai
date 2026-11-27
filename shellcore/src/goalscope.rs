//! A goal's scope and times (G2). The scope comes from the person's own typed
//! line, the request as the console or panel received it, never from the
//! arguments a model wrote for `goal_set`: the model may word the goal, but
//! what it may touch is what the person said. It is worked out once, shown
//! on `show`, counted on the card, and handed to `grant_goal` as the same
//! value; nothing derives it again.
//!
//! - **Rows**: explicit registry rows, never `<group>_*`: each OS row the
//!   line names by a whole cue, path words aside. One consent reads as
//!   negated (`consent::named_in`: "never move") refuses the goal. A word that names a row a goal may never
//!   hold (one that sends, installs, changes a permission or never runs)
//!   refuses the goal and says which word. At most [`MAX_ACTIONS`].
//! - **Folders**: each path written in the line, under the person's home:
//!   relative, or `~/` or `/home/user/` in front, normalised, with no `..`,
//!   no other absolute path and only plain path characters, and one `fs`
//!   lists. (kaifs has no symlinks: a listed folder is the folder.) Naming a
//!   folder adds the file reads, so the goal can see what it works on.
//! - **Times**: `90m`, `2h` or `1d`. Anything else is refused, never guessed.

use agentabi::{Action, Owner, Tier};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// The most actions a goal names.
pub const MAX_ACTIONS: usize = 8;
/// The file reads a goal that names a folder may make.
/// Not `fs_find`, which searches every folder: only the person's own word
/// "find" adds it.
const FOLDER_READS: [&str; 2] = ["fs_list", "fs_read"];
/// What a goal that names nothing it does is told.
pub const NAMES_NOTHING: &str =
    "say what the goal does, for example \"sort the files in notes/ by topic\"";
/// What a goal that names too much is told.
pub const SAY_LESS: &str = "say less: a goal names at most 8 actions";

/// The scope of a goal the person asked for with `line`; `listed` asks `fs`
/// whether a folder is there.
pub(crate) fn scope(
    line: &str,
    listed: impl Fn(&str) -> Result<(), String>,
) -> Result<goalcore::Scope, String> {
    // A folder's path is where the goal works, not what it does: its words
    // ("notes/q3") name no action: `outside_names` drops it.
    let mut rows: Vec<String> = Vec::new();
    for row in agentabi::ROWS.iter().filter(|row| considered(row)) {
        // Named by a whole cue ("go" is not in "good"); negated as consent
        // reads the line ("never move").
        // A joined name ("do-not-delete") is the name's words, not the
        // row's: its cue names no row and its "not" negates none, as consent
        // reads it (bug-hunt-6 H6-3).
        let row_words = outside_names(line, row);
        let plain_words = plain(&row_words);
        let Some(cue) = row.cues.iter().find(|cue| names(&plain_words, cue)) else {
            continue;
        };
        let negated = agentabi::consent::named_in(&row_words, row) == Some(true);
        if negated {
            return Err(format!(
                "\"{cue}\" is said not to happen; say only what the goal does"
            ));
        }
        if !may_hold(row) {
            return Err(format!(
                "\"{cue}\" names {}, which a goal may not do; do that with your own line",
                row.name
            ));
        }
        rows.push(row.name.to_string());
    }
    let folders = folders(line)?;
    for folder in &folders {
        listed(folder).map_err(|why| format!("{folder}: {why}"))?;
    }
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
    if rows.len() > MAX_ACTIONS {
        return Err(String::from(SAY_LESS));
    }
    Ok(goalcore::Scope { rows, folders })
}

/// Whether a word naming `row` counts at all: not a goal row, and not one of
/// the shell's pure helpers.
fn considered(row: &Action) -> bool {
    !row.name.starts_with("goal_") && !(row.owner == Owner::Shell && row.facts.read_only)
}

/// Whether a goal's ceiling may hold `row`: not a shell-owned change (a
/// permission or a restart), never-tier, a send or an install.
fn may_hold(row: &Action) -> bool {
    row.owner != Owner::Shell
        && agentabi::derive(row) != Tier::Never
        && !agentid::goalgrant::sends_or_installs(row)
        && !reaches_past_folders(row)
}

/// Whether `row` reaches past a goal's approved folders with no path the
/// shell can hold to them: a file row with no path argument (`fs_find`,
/// `fs_changes`, `fs_empty_trash`) or a window row. A goal never holds one.
pub(crate) fn reaches_past_folders(row: &Action) -> bool {
    let is_file = agentabi::FS_ROWS.iter().any(|r| r.name == row.name);
    let has_path = row.args.iter().any(|arg| is_path(arg.name));
    (is_file && !has_path) || agentabi::UI_ROWS.iter().any(|r| r.name == row.name)
}

/// Whether argument `name` is a file row's path: `path`, `from` or `to`.
pub(crate) fn is_path(name: &str) -> bool {
    matches!(name, "path" | "from" | "to")
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

/// Parts of a joined word that only negate ("do-not", "don't", "never-ever"
/// as [`plain`] splits them): such a word is no name, so it stays.
const NEGATING_PARTS: &[&str] = &[
    "do", "does", "did", "don", "doesn", "didn", "t", "nt", "not", "never", "ever", "no", "dont",
    "cannot", "can", "won", "wo", "shouldn", "mustn",
];

/// Words before a joined word that make it a name, not a verb: "my
/// do-not-delete folder". So does any possessive ("Bob's").
const NAME_LEADS: &[&str] = &[
    "my", "the", "a", "an", "this", "that", "these", "those", "our", "your", "his", "her", "their",
    "its",
];

/// `words` without the names joined by `-`, `.` or `_` that hold a word none
/// of `row`'s cues has: "do-not-delete" goes for `fs_trash`, "re-sort" or a
/// cue's own "round-robin" stays, and so does a word made only of negation
/// ("do-not move anything" still says not to; #1467's round 20).
///
/// A word of negation and the row's own cues stays too, unless a determiner
/// before it makes it a name: "dont-delete the finals" says not to, "my
/// do-not-delete folder" is a folder. A word that goes leaves its punctuation,
/// so "todo.txt," still ends its clause (#1467's round 21). A folder's path
/// ("notes/q3") goes the same way, and is no determiner: "my notes/q3,
/// dont-delete the finals" still says not to (#1556's review).
fn outside_names(words: &str, row: &Action) -> String {
    let own = |part: &str| {
        part == "re"
            || row.cues.iter().any(|cue| {
                plain(cue)
                    .split(' ')
                    .any(|w| !w.is_empty() && part.starts_with(w))
            })
    };
    let negating = |part: &str| NEGATING_PARTS.contains(&part);
    let mut kept: Vec<&str> = Vec::new();
    let mut before = String::new();
    for raw in words.split_whitespace() {
        // A path goes whole, leaving only the clause or sentence mark after it.
        if raw.contains('/') {
            let ends = raw.trim_end_matches(|c: char| ",;:!?.)\"'".contains(c));
            let mark = &raw[ends.len()..];
            if !mark.is_empty() {
                kept.push(mark);
            }
            before = raw.to_ascii_lowercase();
            continue;
        }
        let word = raw.trim_matches(|c: char| !c.is_ascii_alphanumeric());
        let plain = plain(word);
        let parts: Vec<&str> = plain.split(' ').filter(|p| !p.is_empty()).collect();
        let named = NAME_LEADS.contains(&before.as_str())
            || before.ends_with("'s")
            || before.ends_with("\u{2019}s");
        let stays = !word.contains(['-', '.', '_'])
            || parts.iter().all(|p| own(p))
            || parts.iter().all(|p| negating(p))
            || (!named
                && parts.iter().any(|p| negating(p))
                && parts.iter().all(|p| negating(p) || own(p)));
        before = word.to_ascii_lowercase();
        if stays {
            kept.push(raw);
            continue;
        }
        let at = raw.find(word).unwrap_or(0);
        let (lead, trail) = (&raw[..at], &raw[at + word.len()..]);
        kept.extend([lead, trail].into_iter().filter(|p| !p.is_empty()));
    }
    kept.join(" ")
}

/// Whether cue `cue` is words of `words` ([`plain`]'s form), whole, or with a
/// plural `s`.
fn names(words: &str, cue: &str) -> bool {
    let cue = plain(cue);
    let cue = cue.trim_end();
    !cue.trim().is_empty()
        && (words.contains(&format!("{cue} ")) || words.contains(&format!("{cue}s ")))
}

/// The folders written in `line`, normalised.
fn folders(line: &str) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for word in line.split_whitespace() {
        let word = word.trim_matches(|c: char| ",;:!?\"'()".contains(c));
        let word = word.trim_end_matches('.');
        if !word.contains('/') {
            continue;
        }
        let path = folder(word)?;
        if !out.contains(&path) {
            out.push(path);
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

/// Whether `path`, a goal step's path argument, lies inside one of
/// `folders` once normalised as `goal_set` normalised them ([`folder`]): a
/// path that does not normalise, or `..`, is inside none.
pub(crate) fn path_inside(folders: &[String], path: &str) -> bool {
    let Ok(path) = folder(path) else {
        return false;
    };
    folders.iter().any(|f| {
        path == *f
            || path
                .strip_prefix(f.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

/// `word` as a folder under home, relative and normalised, or why not.
pub(crate) fn folder(word: &str) -> Result<String, String> {
    let refused = || format!("{word} is not a folder in your home folder");
    let plain_chars = word
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "._-/~".contains(c));
    if !plain_chars {
        return Err(refused());
    }
    let rest = match word {
        "~" | "/home/user" => "",
        _ => match (word.strip_prefix("~/"), word.strip_prefix("/home/user/")) {
            (Some(rest), _) | (_, Some(rest)) => rest,
            _ if word.starts_with('/') || word.starts_with('~') => return Err(refused()),
            _ => word,
        },
    };
    let mut parts: Vec<&str> = Vec::new();
    for part in rest.split('/') {
        match part {
            "" | "." => {}
            ".." => return Err(refused()),
            part if part.contains('~') => return Err(refused()),
            part => parts.push(part),
        }
    }
    match parts.is_empty() {
        true => Err(format!(
            "{word}: name a folder in your home folder, not all of it"
        )),
        false => Ok(parts.join("/")),
    }
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
