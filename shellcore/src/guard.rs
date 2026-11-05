//! Two shapes a small model writes that the shell must not take at their word
//! (the shipped-pack actbench, #715):
//!
//! - A placeholder argument, `pkg_remove("package_name")`: a call that names
//!   nothing the user said. A direct call asks which one is meant; a script
//!   that holds one is refused.
//! - A claim, `print("Change applied.")`: a script with no OS call that only
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
    "todo",
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

/// Whether the user's `line` holds `value`, ignoring case.
fn said(value: &str, line: &str) -> bool {
    line.to_ascii_lowercase()
        .contains(value.to_ascii_lowercase().as_str())
}

/// The question a direct call with a placeholder gets instead of running.
pub fn which(arg: &str, value: &str) -> String {
    format!("Which {arg}? \"{value}\" is a placeholder, not one you named.")
}

/// Why a script holding a placeholder is refused, fit for a repair.
pub fn refused(call: &str, arg: &str, value: &str) -> String {
    format!("{call}'s {arg} \"{value}\" is a placeholder; use the name the request gives, or ask")
}

/// The first literal argument of `plan` that is a placeholder for `line`, as
/// the reason to refuse the script.
pub fn in_plan(plan: &crate::analyse::Plan, line: &str) -> Option<String> {
    plan.sites.iter().find_map(|site| {
        site.args.iter().find_map(|(arg, val)| {
            let text = val.text.as_deref()?;
            let literal = val.label == agentabi::consent::Label::Literal;
            (literal && placeholder(text, line)).then(|| refused(site.name, arg, text))
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
    "installed",
    "uninstalled",
    "restarted",
    "stopped",
    "started",
    "deleted",
    "enabled",
    "disabled",
    "saved",
    "switched",
    "set to",
    "done",
    "complete",
];

/// The line a claim script's output gets.
pub const NOTHING_CHANGED: &str = "Nothing was changed: the script made no OS call.";

/// Whether `text` claims a change: said by a script that made no OS call, it
/// cannot be true.
pub fn claims(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    CLAIMS.iter().any(|claim| lower.contains(claim))
}
