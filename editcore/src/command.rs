//! The editor's commands, one per line typed in edit mode.

use alloc::string::String;
use alloc::vec::Vec;

/// The widest column or indent a command names.
const MAX_COLUMN: usize = 65_536;

pub const USAGE: &str = "\
files               list the files; * marks changed ones
open <file>         edit another file (its path, or the end of it)
p [n[,m]]           print lines (around the current line, or from n, or n to m)
find <text>         every line holding text, in every file
s |old|new|         replace the one place old appears in this file
s <n> |old|new|     ... in line n; any character can stand for |
c <n> <text>        change line n
i <n> <text>        insert a line before line n
a <n> <text>        add a line after line n
d <n>[,<m>]         delete lines
split <n> <col> [k] break line n after col characters; the rest is line n+1, k spaces in
join <n>            join line n+1 onto line n
view <n>,<m>        the lines for a screen, with the file's state; changes nothing
undo                take back the last edit
diff                what changed since the version was opened
build               build it on the host in the background, as the next version
try                 rehearse that build on copies of the state; changes nothing
status              what is open, and how the last build went
reset               drop every edit
help                this list; :apply runs a build, :quit leaves";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command<'a> {
    Files,
    Open(&'a str),
    Print {
        from: Option<usize>,
        to: Option<usize>,
    },
    Find(&'a str),
    Substitute {
        line: Option<usize>,
        old: &'a str,
        new: &'a str,
    },
    Change(usize, &'a str),
    Insert(usize, &'a str),
    Append(usize, &'a str),
    Delete(usize, usize),
    Split {
        line: usize,
        col: usize,
        indent: usize,
    },
    Join(usize),
    View(usize, usize),
    Undo,
    Diff,
    Build,
    Try,
    Status,
    Reset,
    Help,
}

fn number(text: &str) -> Result<usize, String> {
    text.parse()
        .ok()
        .filter(|&n| n > 0 && n <= crate::MAX_LINES + 1)
        .ok_or_else(|| alloc::format!("{text:?} is not a line number"))
}

/// `n` or `n,m`.
fn range(text: &str) -> Result<(usize, usize), String> {
    match text.split_once(',') {
        Some((from, to)) => Ok((number(from)?, number(to)?)),
        None => number(text).map(|n| (n, n)),
    }
}

/// A column or an indent: from 0.
fn count(text: &str) -> Result<usize, String> {
    text.parse()
        .ok()
        .filter(|&n| n <= MAX_COLUMN)
        .ok_or_else(|| alloc::format!("{text:?} is not a column"))
}

/// A line number, then one space, then the rest exactly as typed.
fn numbered(rest: &str) -> Result<(usize, &str), String> {
    let (n, text) = rest.split_once(' ').unwrap_or((rest, ""));
    Ok((number(n)?, text))
}

/// `|old|new|`, with any character that is not a letter, digit or space as `|`.
fn substitution(rest: &str) -> Result<(&str, &str), String> {
    let bad = || String::from("s expects |old|new|");
    let delimiter = rest.chars().next().ok_or_else(bad)?;
    if delimiter.is_alphanumeric() || delimiter.is_whitespace() {
        return Err(bad());
    }
    let body = &rest[delimiter.len_utf8()..];
    let mut parts = body.split(delimiter);
    let (Some(old), Some(new), Some(""), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(bad());
    };
    Ok((old, new))
}

/// Parses one line; the error is what to print, with the usage on a bad command.
pub fn parse(line: &str) -> Result<Command<'_>, String> {
    let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
    let none = |command: Command<'static>| {
        if rest.trim().is_empty() {
            Ok(command)
        } else {
            Err(alloc::format!("{word} takes nothing more"))
        }
    };
    match word {
        "files" => none(Command::Files),
        "undo" => none(Command::Undo),
        "diff" => none(Command::Diff),
        "build" => none(Command::Build),
        "try" => none(Command::Try),
        "status" => none(Command::Status),
        "reset" => none(Command::Reset),
        "help" => none(Command::Help),
        "open" if !rest.trim().is_empty() => Ok(Command::Open(rest.trim())),
        "find" if !rest.is_empty() => Ok(Command::Find(rest)),
        "p" if rest.trim().is_empty() => Ok(Command::Print {
            from: None,
            to: None,
        }),
        "p" => {
            let (from, to) = range(rest.trim())?;
            let to = rest.contains(',').then_some(to);
            Ok(Command::Print {
                from: Some(from),
                to,
            })
        }
        "s" => {
            let first = rest.chars().next();
            let (line, rest) = if first.is_some_and(|c| c.is_ascii_digit()) {
                let (n, rest) = rest.split_once(' ').ok_or("s expects |old|new|")?;
                (Some(number(n)?), rest)
            } else {
                (None, rest)
            };
            let (old, new) = substitution(rest)?;
            Ok(Command::Substitute { line, old, new })
        }
        "c" => numbered(rest).map(|(n, text)| Command::Change(n, text)),
        "i" => numbered(rest).map(|(n, text)| Command::Insert(n, text)),
        "a" => numbered(rest).map(|(n, text)| Command::Append(n, text)),
        "d" => {
            let (from, to) = range(rest.trim())?;
            Ok(Command::Delete(from, to))
        }
        "split" => {
            let words: Vec<&str> = rest.split(' ').collect();
            match words[..] {
                [line, col] => Ok((number(line)?, count(col)?, 0)),
                [line, col, indent] => Ok((number(line)?, count(col)?, count(indent)?)),
                _ => Err(String::from("split expects <n> <col> [indent]")),
            }
            .map(|(line, col, indent)| Command::Split { line, col, indent })
        }
        "join" => number(rest.trim()).map(Command::Join),
        "view" => range(rest.trim()).map(|(from, to)| Command::View(from, to)),
        "" => Err(String::from("type help for the commands")),
        other => Err(alloc::format!("unknown command {other:?}; type help")),
    }
}
