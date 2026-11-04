//! What is indexed (ADR 0013 section 4, question 3's default): text in
//! `/home/user`, except folders the person excludes; nothing from `system` or
//! `models`. A path is checked whole before anything else is done with it: it
//! must be absolute and clean, so `..`, `.`, `//` and control characters never
//! reach a prefix test, and a folder matches only whole components, so
//! `/home/username` is not under `/home/user`.

use alloc::string::String;
use alloc::vec::Vec;

/// The one folder indexed, with everything under it.
pub const HOME: &str = "/home/user";
/// The longest path kept, in bytes.
pub const MAX_PATH: usize = 1024;
/// Most folders the person may exclude.
pub const MAX_EXCLUDED: usize = 256;

/// Why a path is not one to index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skip {
    /// Not an absolute, clean path of at most [`MAX_PATH`] bytes.
    Unclean,
    /// Outside [`HOME`]: a system file, a model, anything else.
    Outside,
    /// Under a folder the person excluded.
    Excluded,
    /// Not UTF-8 text.
    NotText,
    /// Larger than the most a file may be to be indexed.
    TooLarge,
}

impl Skip {
    pub fn why(self) -> &'static str {
        match self {
            Self::Unclean => "the path is not a clean absolute path",
            Self::Outside => "only files in /home/user are indexed",
            Self::Excluded => "it is in a folder you excluded",
            Self::NotText => "it is not UTF-8 text",
            Self::TooLarge => "it is too large to index",
        }
    }
}

/// Whether `path` is absolute and clean: it starts with `/`, has no empty,
/// `.` or `..` component, no trailing `/`, no control character, and is at
/// most [`MAX_PATH`] bytes. `/` alone is clean.
pub fn clean(path: &str) -> bool {
    if path == "/" {
        return true;
    }
    let Some(rest) = path.strip_prefix('/') else {
        return false;
    };
    path.len() <= MAX_PATH
        && !path.chars().any(char::is_control)
        && rest
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

/// Whether `path` is `folder` or under it, by whole components. Both must be
/// clean.
pub fn within(path: &str, folder: &str) -> bool {
    if folder == "/" {
        return true;
    }
    match path.strip_prefix(folder) {
        Some("") => true,
        Some(rest) => rest.starts_with('/'),
        None => false,
    }
}

/// The folders under [`HOME`] that are never indexed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scope {
    excluded: Vec<String>,
}

impl Scope {
    pub fn new() -> Self {
        Self::default()
    }

    /// Excludes `folder` and everything under it. It must be a clean path under
    /// [`HOME`] (or `HOME` itself, which turns indexing off).
    pub fn exclude(&mut self, folder: &str) -> Result<(), Skip> {
        if !clean(folder) {
            return Err(Skip::Unclean);
        }
        if !within(folder, HOME) {
            return Err(Skip::Outside);
        }
        if self.excluded.len() >= MAX_EXCLUDED {
            return Err(Skip::TooLarge);
        }
        if !self.excluded.iter().any(|f| f == folder) {
            self.excluded.push(String::from(folder));
        }
        Ok(())
    }

    /// The excluded folders, in the order they were added.
    pub fn excluded(&self) -> &[String] {
        &self.excluded
    }

    /// Whether `path` may be indexed, or why not.
    pub fn check(&self, path: &str) -> Result<(), Skip> {
        if !clean(path) {
            return Err(Skip::Unclean);
        }
        if path == HOME || !within(path, HOME) {
            return Err(Skip::Outside);
        }
        if self.excluded.iter().any(|f| within(path, f)) {
            return Err(Skip::Excluded);
        }
        Ok(())
    }
}

/// A file's bytes as text to index: UTF-8 with no NUL, which no text file
/// holds.
pub fn text(bytes: &[u8]) -> Result<&str, Skip> {
    match core::str::from_utf8(bytes) {
        Ok(text) if !text.contains('\0') => Ok(text),
        _ => Err(Skip::NotText),
    }
}
