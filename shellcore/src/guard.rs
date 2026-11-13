//! Two shapes a small model writes that the shell must not take at their word
//! (the shipped-pack actbench, #715):
//!
//! - A placeholder argument, `pkg_remove("package_name")`: a call whose target
//!   names nothing the user said. A direct call asks which one is meant; a
//!   script that holds one is refused. Only targets are checked: a payload
//!   (`"<p>hi</p>"`, `"TODO"`) or an option is whatever the user wants written
//!   (C on #716).
//! - A claim, `print("Change applied.")`: a script that changed nothing but
//!   prints success. Its output is shown with what the shell knows: nothing
//!   was changed.
//!
//! Neither changes what the model is given: both read what came back.

use alloc::format;
use alloc::string::String;

/// Generic nouns a model writes `<noun>_name` or `<noun> name` with.
const NOUNS: &[&str] = &[
    "app",
    "application",
    "file",
    "folder",
    "dir",
    "directory",
    "host",
    "model",
    "package",
    "pkg",
    "path",
    "policy",
    "process",
    "profile",
    "program",
    "service",
    "theme",
    "user",
    "window",
    "target",
    "item",
    "thing",
];

/// Whole values that name nothing.
const WORDS: &[&str] = &[
    "placeholder",
    "tbd",
    "xxx",
    "foo",
    "bar",
    "baz",
    "example",
    "sample",
    "something",
    "name",
    "value",
    "string",
    "text",
    "unknown",
    "none",
    "null",
    "undefined",
    "n/a",
    "...",
];

/// Whether `value` is a placeholder rather than a name. A value the user's
/// `line` holds is theirs, never a placeholder: "remove foo" removes `foo`.
pub fn placeholder(value: &str, line: &str) -> bool {
    let value = value.trim();
    if value.is_empty() || said(value, line) {
        return false;
    }
    let lower = value.to_ascii_lowercase();
    let wrapped = |open: char, close: char| lower.starts_with(open) && lower.ends_with(close);
    if wrapped('<', '>') || wrapped('{', '}') || wrapped('[', ']') {
        return true;
    }
    if lower.contains("path/to/") || WORDS.contains(&lower.as_str()) {
        return true;
    }
    let words: alloc::vec::Vec<&str> = lower
        .split(['_', '-', ' '])
        .filter(|w| !w.is_empty())
        .collect();
    match words.as_slice() {
        ["your", ..] if words.len() > 1 => true,
        [.., "here"] if words.len() > 1 => true,
        [noun, "name" | "id"] => NOUNS.contains(noun),
        _ => false,
    }
}

/// Whether the user's `line` holds `value`, ignoring case and the joins a
/// name is written with: "to-do" holds `todo`, "build 4" holds `build-4`.
fn said(value: &str, line: &str) -> bool {
    let value = squash(value);
    !value.is_empty() && squash(line).contains(value.as_str())
}

/// `text` lowercased, without '-', '_' or spaces.
fn squash(text: &str) -> String {
    text.chars()
        .filter(|c| !matches!(c, '-' | '_' | ' '))
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// The question a direct call with a placeholder gets instead of running.
pub fn which(arg: &str, value: &str) -> String {
    format!("Which {arg}? \"{value}\" is a placeholder, not one you named.")
}

/// Why a script holding a placeholder is refused, fit for a repair.
pub fn refused(call: &str, arg: &str, value: &str) -> String {
    format!("{call}'s {arg} \"{value}\" is a placeholder; use the name the request gives, or ask")
}

/// Whether `row`'s argument `arg` is what the call acts on.
/// The first free target of a direct call to `row` whose value is a
/// placeholder for `line`, as `(argument, value)`: the shell asks which one
/// instead of running the call. An enum target is checked by its type.
pub fn placeholder_target<'a>(
    row: &agentabi::Action,
    args: impl IntoIterator<Item = (&'a str, &'a str)>,
    line: &str,
) -> Option<(&'a str, &'a str)> {
    use agentabi::ArgType;
    args.into_iter().find(|(name, value)| {
        let named = row.args.iter().find(|arg| arg.name == *name);
        let free = named.is_some_and(|arg| {
            !matches!(arg.ty, ArgType::Enum(_) | ArgType::Opt(ArgType::Enum(_)))
        });
        free && is_target(row, name) && placeholder(value, line)
    })
}

pub fn is_target(row: &agentabi::Action, arg: &str) -> bool {
    row.args
        .iter()
        .any(|a| a.name == arg && matches!(a.role, agentabi::Role::Target))
}

/// The first literal target of `plan` that is a placeholder for `line`, as
/// the reason to refuse the script.
pub fn in_plan(plan: &crate::analyse::Plan, line: &str) -> Option<String> {
    plan.sites.iter().find_map(|site| {
        let row = agentabi::find(&agentabi::ROWS, site.name)?;
        site.args.iter().find_map(|(arg, val)| {
            let text = val.text.as_deref()?;
            let literal = val.label == agentabi::consent::Label::Literal;
            let guarded = literal && is_target(row, arg) && placeholder(text, line);
            guarded.then(|| refused(site.name, arg, text))
        })
    })
}

/// Words that claim a change was made.
const CLAIMS: &[&str] = &[
    "success",
    "applied",
    "has been",
    "have been",
    "is now",
    "are now",
    "changed",
    "updated",
    "removed",
    "deleted",
    "saved",
    "switched",
    "set to",
    "done",
];

/// The line a claim script's output gets.
pub const NOTHING_CHANGED: &str = "Nothing was changed: the script made no change.";

/// Whether `text` claims a change: said by a script that changed nothing, it
/// cannot be true.
pub fn claims(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    CLAIMS.iter().any(|claim| lower.contains(claim))
}
