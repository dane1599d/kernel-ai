//! A folder a goal's line says not to touch (#1577's review): a path the line
//! keeps off refuses the goal, and the refusal names that path.
//!
//! Two passes, and the goal is refused when either refuses (#1650's review):
//!
//! - [`main_pass`] is the reader as it was before #1650, byte for byte, word
//!   lists, clause ends and apostrophe folds included. A test pins its text,
//!   so every line it refused is still refused, by construction.
//! - [`added`] reads what #1650 adds: "leave out", "out" right after a path,
//!   "without" and an "-ing" word, "isn't", "aren't", "wasn't", "shan't" and
//!   "neither", a list after a comma whose first item is not a path, a child
//!   folder by its bare name, three more apostrophe marks, and the carry
//!   decided from the last word that keeps off. It only ever adds a refusal.
//!
//! Each pass's own doc says what it reads and what it does not.

use alloc::string::String;

mod added;
mod main_pass;

/// The first path `line` keeps off, as either pass reads it; `starts_order`
/// says whether a word after "and" starts another order (a row's cue, as the
/// goal's scope reads it).
pub(crate) fn kept_off(line: &str, starts_order: impl Fn(&str) -> bool) -> Option<String> {
    main_pass::kept_off(line, &starts_order).or_else(|| added::kept_off(line, &starts_order))
}

/// [`main_pass`]'s text as it was before #1650 (sha256), so no edit to it can
/// make a line it refused pass.
#[cfg(test)]
pub(crate) const MAIN_PASS_SHA256: &str =
    "cfb5ccb588c4fa99065f8c83e89c08cef245e18112f793683a4bf8a16742bc86";
