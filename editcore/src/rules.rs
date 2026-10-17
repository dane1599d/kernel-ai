//! The edit rules, checked by the edit service before it builds and enforced again
//! by the build service before it compiles. They close the plain ways for source
//! typed in the OS to read or run things on the host at build time: only existing
//! `.rs` files under the component's `src` may change, the manifest and the file
//! set stay as they were, and changed lines may not name the macros and attributes
//! that read files or the environment. This is a guardrail for code the founder
//! types, not a sandbox; LLM-written source needs one first (M9c).

use crate::Editable;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use livecore::artifact::source_files;
use livecore::diff;

/// Words a changed line may not hold: they read files or the environment at
/// compile time, or pass text to the host's assembler.
pub const DENIED_WORDS: &[&str] = &[
    "include",
    "include_str",
    "include_bytes",
    "env",
    "option_env",
    "asm",
    "global_asm",
    "naked_asm",
    "debugger_visualizer",
];
/// Text a changed line may not hold once its spaces are taken out.
pub const DENIED_TEXT: &[&str] = &["#[path", "#!"];

/// A relative path with `/` between plain segments.
pub fn check_path(path: &str) -> Result<(), String> {
    let bad = |why: &str| Err(format!("{path:?}: {why}"));
    if path.is_empty() || path.starts_with('/') {
        return bad("not a relative path");
    }
    if path.contains('\\') || path.contains(':') {
        return bad("backslashes and drive letters are not allowed");
    }
    for segment in path.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return bad("empty, . and .. segments are not allowed");
        }
        if segment.chars().any(|c| c.is_control()) {
            return bad("control characters are not allowed");
        }
    }
    Ok(())
}

fn words(line: &str) -> impl Iterator<Item = &str> {
    line.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|word| !word.is_empty())
}

/// Why a changed line is refused, if it is.
pub fn check_line(line: &str) -> Result<(), String> {
    if let Some(word) = words(line).find(|word| DENIED_WORDS.contains(word)) {
        return Err(format!("changed lines may not use {word}"));
    }
    let squeezed: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    if let Some(text) = DENIED_TEXT.iter().find(|text| squeezed.contains(*text)) {
        return Err(format!("changed lines may not hold {text}"));
    }
    Ok(())
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
        for line in &change.added {
            if let Ok(at) = carried.binary_search(line) {
                carried.remove(at);
                continue;
            }
            check_line(&diff::shown(line)).map_err(|why| format!("{path}: {why}"))?;
        }
    }
    Ok(())
}
