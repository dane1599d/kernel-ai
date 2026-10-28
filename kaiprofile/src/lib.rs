//! `kai-profile 1` (P3 task 12; the founder's decision of 2026-10-27): a named
//! resource profile, a CPU class and a memory limit for each app or group.
//!
//! ```text
//! kai-profile 1
//! name gaming
//! focused class=normal mem=- gpu=0
//! group builds members=build-* class=background mem=65536 gpu=0
//! others class=background mem=- gpu=0
//! ```
//!
//! A line names its target, then `class` (`normal` or `background`, the classes
//! policy's two that PROC_CLASS_SET takes), `mem` (a page quota, or `-` to leave
//! it as it is) and `gpu`, reserved: anything but 0 is refused until GPU support
//! (P7+). Targets are `app <name>`, `group <name> members=<a>+<b>` (a member may
//! end in `*`), `focused` (the app with focus) and `others` (every app the
//! profile names no other way). Profiles never name the shell, the supervisor,
//! the model service or `desktop`: whatever a profile says, the kernel keeps
//! them from being starved, and refuses to lower them.
//!
//! The text is read as hostile, as one a user wrote or one that comes back from
//! disk: anything out of shape refuses the whole profile, with its line. What
//! reads writes back byte for byte.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::Write;

pub const HEADER: &str = "kai-profile 1";
/// Most lines a profile may hold, after its header and name.
pub const MAX_LINES: usize = 64;
/// Longest line.
pub const MAX_LINE: usize = 200;
/// The lowest page quota PROC_QUOTA_SET takes.
pub const MIN_PAGES: u64 = 64;
/// The processes a profile may never name; the kernel protects them too.
pub const PROTECTED: [&str; 4] = ["shell", "supervisor", "llm", "desktop"];

/// The classes policy's classes a profile sets (PROC_CLASS_SET's 0 and 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Normal,
    Background,
}

impl Class {
    pub fn word(self) -> &'static str {
        match self {
            Class::Normal => "normal",
            Class::Background => "background",
        }
    }

    /// The number PROC_CLASS_SET takes.
    pub fn number(self) -> u64 {
        match self {
            Class::Normal => 0,
            Class::Background => 1,
        }
    }
}

/// What a line applies to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    App(String),
    Group { name: String, members: Vec<String> },
    Focused,
    Others,
}

/// One line: a target and what it gets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub target: Target,
    pub class: Class,
    /// A page quota, or `None` to leave it as it is.
    pub mem: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub lines: Vec<Line>,
}

/// Why a profile was refused, and on which line (1 is the header).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub line: usize,
    pub why: &'static str,
}

/// What a profile gives one running process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Setting {
    pub process: String,
    pub class: Class,
    pub mem: Option<u64>,
}

impl Profile {
    /// The profile's text: what [`parse`] reads back the same.
    pub fn text(&self) -> String {
        let mut out = format!("{HEADER}\nname {}\n", self.name);
        for line in &self.lines {
            let target = match &line.target {
                Target::App(name) => format!("app {name}"),
                Target::Group { name, members } => {
                    format!("group {name} members={}", members.join("+"))
                }
                Target::Focused => String::from("focused"),
                Target::Others => String::from("others"),
            };
            let mem = line.mem.map_or(String::from("-"), |p| p.to_string());
            let _ = writeln!(out, "{target} class={} mem={mem} gpu=0", line.class.word());
        }
        out
    }

    /// What the profile gives each of `running` (process names), with `focused`
    /// the app that has focus. The most specific line wins: an app's own, then
    /// the focused app's, then its group's (the first that names it), then
    /// `others`. A process no line covers, and a protected one, is left alone.
    pub fn resolve(&self, running: &[&str], focused: Option<&str>) -> Vec<Setting> {
        let find = |pick: &dyn Fn(&Target) -> bool| self.lines.iter().find(|l| pick(&l.target));
        running
            .iter()
            .filter(|name| !PROTECTED.contains(name))
            .filter_map(|&name| {
                let line = find(&|t| *t == Target::App(name.to_string()))
                    .or_else(|| {
                        (focused == Some(name))
                            .then(|| find(&|t| *t == Target::Focused))
                            .flatten()
                    })
                    .or_else(|| {
                        find(&|t| match t {
                            Target::Group { members, .. } => {
                                members.iter().any(|m| member_matches(m, name))
                            }
                            _ => false,
                        })
                    })
                    .or_else(|| find(&|t| *t == Target::Others))?;
                Some(Setting {
                    process: name.to_string(),
                    class: line.class,
                    mem: line.mem,
                })
            })
            .collect()
    }
}

fn member_matches(member: &str, name: &str) -> bool {
    match member.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => member == name,
    }
}

/// The profiles every machine has. `default` is where a boot starts: every app
/// normal, no quota changed.
pub fn builtin() -> Vec<Profile> {
    let line = |target, class, mem| Line { target, class, mem };
    let group = |name: &str, members: &[&str]| Target::Group {
        name: name.to_string(),
        members: members.iter().map(|m| m.to_string()).collect(),
    };
    let profile = |name: &str, lines| Profile {
        name: name.to_string(),
        lines,
    };
    alloc::vec![
        profile(
            "default",
            alloc::vec![line(Target::Others, Class::Normal, None)]
        ),
        // The focused app (the game) runs normal; everything else waits behind it.
        profile(
            "gaming",
            alloc::vec![
                line(Target::Focused, Class::Normal, None),
                line(Target::Others, Class::Background, None),
            ],
        ),
        // Builds and indexing go to the back; the apps you work in stay normal.
        profile(
            "ai-dev",
            alloc::vec![
                line(
                    group("jobs", &["build-*", "index*"]),
                    Class::Background,
                    None
                ),
                line(Target::Others, Class::Normal, None),
            ],
        ),
        // Everything but the system waits.
        profile(
            "quiet",
            alloc::vec![line(Target::Others, Class::Background, None)]
        ),
    ]
}

/// Reads a whole profile, or refuses it.
pub fn parse(text: &str) -> Result<Profile, Error> {
    let at = |line: usize| move |why: &'static str| Error { line, why };
    let mut lines = text.split_inclusive('\n');
    if lines.next() != Some("kai-profile 1\n") {
        return Err(at(1)("a profile starts with kai-profile 1"));
    }
    let name_line = lines
        .next()
        .ok_or_else(|| at(2)("a profile has a name line"))?;
    let name = name_line
        .strip_suffix('\n')
        .and_then(|l| l.strip_prefix("name "))
        .ok_or_else(|| at(2)("the second line is name <name>"))?;
    let name = word(name).map_err(at(2))?.to_string();
    let mut out = Profile {
        name,
        lines: Vec::new(),
    };
    for (i, raw) in lines.enumerate() {
        let n = i + 3;
        let line = raw
            .strip_suffix('\n')
            .ok_or_else(|| at(n)("the last line has no newline"))?;
        if line.len() > MAX_LINE {
            return Err(at(n)("the line is too long"));
        }
        if out.lines.len() >= MAX_LINES {
            return Err(at(n)("a profile has too many lines"));
        }
        let parsed = line_of(line).map_err(at(n))?;
        if out.lines.iter().any(|l| l.target == parsed.target) {
            return Err(at(n)("a target is named twice"));
        }
        out.lines.push(parsed);
    }
    Ok(out)
}

fn line_of(line: &str) -> Result<Line, &'static str> {
    let words: Vec<&str> = line.split(' ').collect();
    let (target, rest) = match words.as_slice() {
        ["app", name, rest @ ..] => {
            let name = word(name)?;
            if PROTECTED.contains(&name) {
                return Err("a profile never names the shell, supervisor, llm or desktop");
            }
            (Target::App(name.to_string()), rest)
        }
        ["group", name, members, rest @ ..] => {
            let members = members
                .strip_prefix("members=")
                .ok_or("a group is group <name> members=<a>+<b>")?
                .split('+')
                .map(|m| {
                    let bare = m.strip_suffix('*').unwrap_or(m);
                    match word(bare).is_ok() && !PROTECTED.contains(&m) {
                        true => Ok(m.to_string()),
                        false => Err("a member is an app name, or one ending in *"),
                    }
                })
                .collect::<Result<Vec<_>, _>>()?;
            (
                Target::Group {
                    name: word(name)?.to_string(),
                    members,
                },
                rest,
            )
        }
        ["focused", rest @ ..] => (Target::Focused, rest),
        ["others", rest @ ..] => (Target::Others, rest),
        _ => return Err("a line is app, group, focused or others"),
    };
    let [class, mem, gpu] = rest else {
        return Err("a line ends class=<class> mem=<pages|-> gpu=0");
    };
    let class = match class.strip_prefix("class=") {
        Some("normal") => Class::Normal,
        Some("background") => Class::Background,
        _ => return Err("class is normal or background"),
    };
    let mem = match mem.strip_prefix("mem=") {
        Some("-") => None,
        Some(pages) => {
            let pages = number(pages).ok_or("mem is a page count or -")?;
            if pages < MIN_PAGES {
                return Err("mem is at least 64 pages");
            }
            Some(pages)
        }
        None => return Err("a line ends class=<class> mem=<pages|-> gpu=0"),
    };
    if *gpu != "gpu=0" {
        return Err("gpu is reserved: it must be 0 until GPU support");
    }
    Ok(Line { target, class, mem })
}

/// A package or profile name: `[a-z][a-z0-9-]`, at most 24 bytes.
fn word(text: &str) -> Result<&str, &'static str> {
    let ok = (1..=24).contains(&text.len())
        && text.starts_with(|c: char| c.is_ascii_lowercase())
        && text
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    ok.then_some(text)
        .ok_or("a name is [a-z][a-z0-9-], at most 24 bytes")
}

/// Digits only, no sign, no leading zero.
fn number(text: &str) -> Option<u64> {
    let digits = !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
    (digits && (text == "0" || !text.starts_with('0')))
        .then(|| text.parse().ok())
        .flatten()
}
