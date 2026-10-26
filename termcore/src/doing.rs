//! `do`: the shell's request that the kernel terminal run one of its own commands
//! for a call consent let through (ADR 0002 D2a): open the editor at a line,
//! apply a component's newest build, or revert one. The line is parsed into a
//! closed [`Do`] and never handed to the typed-command parser; the terminal then
//! calls the functions `:edit`, `:apply` and `:revert` call, and answers
//! `done <id> ok <text>` or `done <id> err <why>`.
//!
//! Wire, one line: `do <id> edit <component> <path> <line>`, `do <id> apply
//! <component>` or `do <id> revert <component> <n>`, then ` once=<badge>` when the
//! terminal showed a menu for the call.

use alloc::format;
use alloc::string::String;

/// Longest component name: a process name.
pub const MAX_COMPONENT: usize = 24;
/// Longest source path, relative to the component's source.
pub const MAX_PATH: usize = 96;
/// Highest line number the editor is asked to open at.
pub const MAX_LINE: u32 = 100_000;
/// How far back `revert` goes (`kabi::live::LINEAGE` is 6).
pub const MAX_BACK: u8 = 5;
/// Longest `do` line.
pub const MAX_DO: usize = 192;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Do {
    Edit {
        component: String,
        path: String,
        line: u32,
    },
    Apply {
        component: String,
    },
    Revert {
        component: String,
        back: u8,
    },
}

/// A parsed `do` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub id: u64,
    pub what: Do,
    /// The `ONCE` badge the menu's Allow gave, when there was a menu.
    pub once: Option<u64>,
}

/// A component name: lower case, digits, `-` and `_`, starting with a letter.
fn component(word: &str) -> Result<String, &'static str> {
    let ok = word.len() <= MAX_COMPONENT
        && word.starts_with(|c: char| c.is_ascii_lowercase())
        && word
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
    match ok {
        true => Ok(String::from(word)),
        false => Err("a component is a lower-case name of at most 24 bytes"),
    }
}

/// A relative source path: `/`-separated parts of letters, digits, `.`, `-` and
/// `_`, none of them empty, `.` or `..`.
fn path(word: &str) -> Result<String, &'static str> {
    let part_ok = |part: &str| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
    };
    match word.len() <= MAX_PATH && word.split('/').all(part_ok) {
        true => Ok(String::from(word)),
        false => Err("a path is relative, of plain parts, at most 96 bytes"),
    }
}

/// A decimal number in `1..=max`, with no sign or leading zero.
fn number(word: &str, max: u32) -> Result<u32, &'static str> {
    let plain =
        !word.is_empty() && !word.starts_with('0') && word.bytes().all(|b| b.is_ascii_digit());
    match word.parse::<u32>() {
        Ok(n) if plain && (1..=max).contains(&n) => Ok(n),
        _ => Err("a number is out of range"),
    }
}

impl Request {
    /// The wire line.
    pub fn encode(&self) -> String {
        let mut line = match &self.what {
            Do::Edit {
                component,
                path,
                line,
            } => format!("do {} edit {component} {path} {line}", self.id),
            Do::Apply { component } => format!("do {} apply {component}", self.id),
            Do::Revert { component, back } => format!("do {} revert {component} {back}", self.id),
        };
        if let Some(badge) = self.once {
            line.push_str(&format!(" once={badge}"));
        }
        line
    }

    /// Reads a `do` line. Anything but the closed forms is refused: an unknown
    /// verb, a missing or extra field, a bad name, path or number.
    pub fn decode(text: &str) -> Result<Self, &'static str> {
        if text.len() > MAX_DO || !text.bytes().all(|b| (b' '..=b'~').contains(&b)) {
            return Err("the do line is too long or not plain text");
        }
        let mut words: alloc::vec::Vec<&str> = text.split(' ').collect();
        let once = match words.last().and_then(|w| w.strip_prefix("once=")) {
            Some(badge) => {
                let badge = badge.parse::<u64>().map_err(|_| "a bad ONCE badge")?;
                words.pop();
                Some(badge)
            }
            None => None,
        };
        let (id, what) = match words.as_slice() {
            ["do", id, rest @ ..] => (id.parse::<u64>().map_err(|_| "a bad id")?, rest),
            _ => return Err("not a do line"),
        };
        let what = match what {
            ["edit", name, file, line] => Do::Edit {
                component: component(name)?,
                path: path(file)?,
                line: number(line, MAX_LINE)?,
            },
            ["apply", name] => Do::Apply {
                component: component(name)?,
            },
            ["revert", name, back] => Do::Revert {
                component: component(name)?,
                back: number(back, u32::from(MAX_BACK))? as u8,
            },
            [] => return Err("a do line names what to do"),
            ["edit" | "apply" | "revert", ..] => return Err("the wrong fields for that verb"),
            _ => return Err("not a verb the terminal runs for the shell"),
        };
        Ok(Self { id, what, once })
    }
}

/// The terminal's answer: `done <id> ok <text>` or `done <id> err <why>`, one
/// line each way (newlines become spaces).
pub fn done(id: u64, result: Result<&str, &str>) -> String {
    let (word, text) = match result {
        Ok(text) => ("ok", text),
        Err(why) => ("err", why),
    };
    let text: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    format!("done {id} {word} {}", text.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(path: &str, line: u32) -> Do {
        Do::Edit {
            component: String::from("notes"),
            path: String::from(path),
            line,
        }
    }

    #[test]
    fn each_form_round_trips() {
        for (what, once) in [
            (edit("src/lib.rs", 40), None),
            (edit("Cargo.toml", 1), Some(77)),
            (
                Do::Apply {
                    component: String::from("notes"),
                },
                None,
            ),
            (
                Do::Revert {
                    component: String::from("notes"),
                    back: 2,
                },
                Some(1),
            ),
        ] {
            let request = Request { id: 9, what, once };
            assert_eq!(Request::decode(&request.encode()), Ok(request.clone()));
        }
        assert_eq!(
            Request::decode("do 3 edit notes src/lib.rs 40 once=12").map(|r| r.encode()),
            Ok(String::from("do 3 edit notes src/lib.rs 40 once=12"))
        );
    }

    #[test]
    fn anything_but_the_closed_forms_is_refused() {
        for bad in [
            "",
            "do",
            "do 1",
            "do x apply notes",
            "do 1 rm notes",
            "do 1 quit",
            "do 1 apply",
            "do 1 apply notes extra",
            "do 1 apply Notes",
            "do 1 apply 1notes",
            "do 1 apply a-name-much-longer-than-24-bytes",
            "do 1 edit notes ../secrets 1",
            "do 1 edit notes /etc/passwd 1",
            "do 1 edit notes src//lib.rs 1",
            "do 1 edit notes ./lib.rs 1",
            "do 1 edit notes src/lib.rs 0",
            "do 1 edit notes src/lib.rs 007",
            "do 1 edit notes src/lib.rs 100001",
            "do 1 edit notes src/lib.rs -1",
            "do 1 edit notes src/lib.rs",
            "do 1 edit notes src/l b.rs 2",
            "do 1 revert notes 6",
            "do 1 revert notes",
            "do 1 apply notes once=x",
            "do 1 apply notes once=1 once=2",
            "do  1 apply notes",
            "do 1 apply notes\n:quit",
            "do 1 apply n\u{e9}tes",
        ] {
            assert!(Request::decode(bad).is_err(), "{bad:?}");
        }
        let long = format!("do 1 edit notes {} 1", "a/".repeat(60));
        assert!(Request::decode(&long).is_err());
    }

    #[test]
    fn done_is_one_line() {
        assert_eq!(
            done(4, Ok("opened src/lib.rs at 40")),
            "done 4 ok opened src/lib.rs at 40"
        );
        assert_eq!(
            done(4, Err("no live component x\nmore")),
            "done 4 err no live component x more"
        );
    }
}
