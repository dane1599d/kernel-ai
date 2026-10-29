//! `kai-draft 1`: what a screen keeps so a crash loses nothing the founder
//! typed. It is the edits the edit service answered `ok` since the version was
//! opened, as the commands that make them, and the line being typed that was not
//! sent yet. The kernel's journal gives a restarted edit service the same edits
//! back from RAM; a draft in storage also survives the machine going down.
//!
//! ```text
//! kai-draft 1
//! component notes
//! label 2
//! file components/notes/src/lib.rs
//! at 58 12 40
//! cmd c 3 let x = 1;
//! cmd split 5 4 2
//! dirty 58 let y =
//! ```
//!
//! A draft is read back from storage, so it is hostile like any file: its size,
//! its commands and every field are bounded, and only editing commands replay.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use livecore::location;

/// Most bytes of a draft.
pub const MAX_DRAFT: usize = 64 * 1024;
/// Most commands in it.
pub const MAX_COMMANDS: usize = 512;
/// Longest command or dirty line.
pub const MAX_TEXT: usize = 1024;
/// The commands that change the text and so replay from a draft.
pub const REPLAYS: [&str; 8] = ["s", "c", "i", "a", "d", "split", "join", "undo"];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Draft {
    pub component: String,
    pub label: String,
    pub file: String,
    pub line: usize,
    pub col: usize,
    pub top: usize,
    pub commands: Vec<String>,
    /// The line being typed and its text, when it differs from the file's.
    pub dirty: Option<(usize, String)>,
}

/// Whether `command` is one a draft keeps and replays.
pub fn replays(command: &str) -> bool {
    let word = command.split(' ').next().unwrap_or("");
    REPLAYS.contains(&word) && text_ok(command)
}

fn text_ok(text: &str) -> bool {
    text.len() <= MAX_TEXT && !text.chars().any(|c| c.is_control())
}

impl Draft {
    pub fn encode(&self) -> String {
        let mut out = format!(
            "kai-draft 1\ncomponent {}\nlabel {}\nfile {}\nat {} {} {}\n",
            self.component, self.label, self.file, self.line, self.col, self.top
        );
        for command in &self.commands {
            out.push_str("cmd ");
            out.push_str(command);
            out.push('\n');
        }
        if let Some((line, text)) = &self.dirty {
            out.push_str(&format!("dirty {line} {text}\n"));
        }
        out
    }

    /// Reads a draft back; the reason when it is not one.
    pub fn parse(text: &str) -> Result<Self, String> {
        let bad = |why: &str| Err(String::from(why));
        if text.len() > MAX_DRAFT {
            return bad("a draft over 64 KiB");
        }
        let mut rows = text.split('\n');
        if rows.next() != Some("kai-draft 1") {
            return bad("not a kai-draft 1");
        }
        let mut field = |name: &str| -> Result<String, String> {
            rows.next()
                .and_then(|row| row.strip_prefix(name))
                .and_then(|rest| rest.strip_prefix(' '))
                .map(String::from)
                .ok_or_else(|| format!("a draft with no {name}"))
        };
        let component = crate::wire::name(&field("component")?)
            .map(String::from)
            .map_err(String::from)?;
        let label = crate::wire::label(&field("label")?)
            .map(String::from)
            .map_err(String::from)?;
        let file = field("file")?;
        if location::shown_path(&file) != file || file.starts_with(".../") {
            return bad("a draft with a bad file path");
        }
        let at = field("at")?;
        let nums: Vec<usize> = at
            .split(' ')
            .map(|n| n.parse().ok().filter(|&n| n <= crate::MAX_LINES + 1))
            .collect::<Option<_>>()
            .unwrap_or_default();
        let [line, col, top] = nums[..] else {
            return bad("a draft with a bad cursor");
        };
        let mut commands = Vec::new();
        let mut dirty = None;
        for row in rows {
            if row.is_empty() {
                continue;
            }
            if dirty.is_some() {
                return bad("a draft with lines after the dirty line");
            }
            if let Some(command) = row.strip_prefix("cmd ") {
                if commands.len() == MAX_COMMANDS || !replays(command) {
                    return bad("a draft with a command that does not replay");
                }
                commands.push(String::from(command));
            } else if let Some(rest) = row.strip_prefix("dirty ") {
                let (n, text) = rest.split_once(' ').unwrap_or((rest, ""));
                let n = n
                    .parse()
                    .ok()
                    .filter(|&n| (1..=crate::MAX_LINES).contains(&n));
                match (n, text_ok(text)) {
                    (Some(n), true) => dirty = Some((n, String::from(text))),
                    _ => return bad("a draft with a bad dirty line"),
                }
            } else {
                return bad("a draft with an unknown line");
            }
        }
        Ok(Self {
            component,
            label,
            file,
            line,
            col,
            top,
            commands,
            dirty,
        })
    }
}
