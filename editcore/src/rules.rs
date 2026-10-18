//! The edit rules, checked by the edit service before it builds and enforced again
//! by the build service before it compiles. They close the plain ways for source
//! typed in the OS to read or run things on the host at build time: only existing
//! `.rs` files under the component's `src` may change, the manifest and the file
//! set stay as they were, and new lines may not name the macros and attributes
//! that read files or the environment. A deny-list stays best-effort: this is a
//! guardrail for code the founder types, not a sandbox, and LLM-written source
//! needs a sandbox first (M9c).

use crate::Editable;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use livecore::artifact::{source_files, valid_path};
use livecore::diff;

/// Words a new line may not hold: they read files or the environment at compile
/// time (`path` names the attribute that loads a module from any file, however
/// it is written), or pass text to the host's assembler. An identifier cannot be
/// split, renamed or hidden in a comment, so matching words catches every form.
pub const DENIED_WORDS: &[&str] = &[
    "include",
    "include_str",
    "include_bytes",
    "env",
    "option_env",
    "path",
    "asm",
    "global_asm",
    "naked_asm",
    "debugger_visualizer",
];
/// Text the new lines of one change may not hold once whitespace is taken out,
/// so an attribute split over lines is caught too.
pub const DENIED_TEXT: &[&str] = &["#[path", "#!"];

/// A relative path with `/` between plain segments.
pub fn check_path(path: &str) -> Result<(), String> {
    valid_path(path).map_err(|why| format!("{path:?}: {why}"))
}

fn words(line: &str) -> impl Iterator<Item = &str> {
    line.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|word| !word.is_empty())
}

/// Control characters other than tab, and the characters rustc reads as
/// whitespace or the console could reorder, none of which a typed line holds.
fn odd(c: char) -> bool {
    (c.is_control() && c != '\t')
        || matches!(
            c,
            '\u{85}' | '\u{200e}' | '\u{200f}' | '\u{2028}' | '\u{2029}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
        )
}

/// Why a new line (without its ending) is refused, if it is.
pub fn check_line(line: &str) -> Result<(), String> {
    if line.chars().any(odd) {
        return Err(String::from("new lines may not hold control characters"));
    }
    if let Some(word) = words(line).find(|word| DENIED_WORDS.contains(word)) {
        return Err(format!("new lines may not use {word}"));
    }
    squeezed_text(line)
}

fn squeezed_text(text: &str) -> Result<(), String> {
    let squeezed: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    match DENIED_TEXT.iter().find(|denied| squeezed.contains(*denied)) {
        Some(denied) => Err(format!("new lines may not hold {denied}")),
        None => Ok(()),
    }
}

/// A line's text without its ending.
fn text(line: &[u8]) -> Result<&str, String> {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    core::str::from_utf8(line).map_err(|_| String::from("new lines must be text"))
}

/// Checks the edited bundle against the one it was opened from.
pub fn check(editable: &Editable, base: &[u8], edited: &[u8]) -> Result<(), String> {
    let base_files = source_files(base).map_err(String::from)?;
    let edited_files = source_files(edited).map_err(String::from)?;
    let inside = format!("{}/", editable.dir);
    let src = format!("{}/src/", editable.dir);
    for (path, _) in base_files.iter().chain(&edited_files) {
        check_path(path)?;
        if !path.starts_with(&inside) {
            return Err(format!("{path} is outside {}", editable.dir));
        }
    }
    let paths = |files: &[(&str, &[u8])]| -> Vec<String> {
        let mut paths: Vec<String> = files.iter().map(|(p, _)| String::from(*p)).collect();
        paths.sort();
        paths
    };
    if paths(&base_files) != paths(&edited_files) {
        return Err(String::from("files cannot be added, removed or renamed"));
    }
    let changes = diff::changes(base, edited).map_err(String::from)?;
    for change in &changes {
        let path = change.path;
        if !(path.starts_with(&src) && path.ends_with(".rs")) {
            return Err(format!(
                "{path} cannot change; only .rs files under {src} can"
            ));
        }
        // A region spans every line between the first and last change; lines it
        // only carries over (or moves) are not new text.
        let mut carried: Vec<&[u8]> = change.removed.clone();
        carried.sort_unstable();
        let mut joined = String::new();
        for line in &change.added {
            if let Ok(at) = carried.binary_search(line) {
                carried.remove(at);
                continue;
            }
            let line = text(line).map_err(|why| format!("{path}: {why}"))?;
            check_line(line).map_err(|why| format!("{path}: {why}"))?;
            joined.push_str(line);
        }
        squeezed_text(&joined).map_err(|why| format!("{path}: {why}"))?;
    }
    Ok(())
}
