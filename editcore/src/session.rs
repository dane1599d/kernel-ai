//! One editing session: the files of the version that was opened, as lines that
//! each keep their own ending, so the bundle written back is byte for byte the one
//! read except where a line was edited.

use crate::{MAX_BUNDLE, MAX_FILES, MAX_LINES};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use livecore::artifact;

/// Edits [`Session::undo`] can take back.
pub const UNDO: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
    Lf,
    CrLf,
    /// The last line of a file without a final newline.
    None,
}

impl Ending {
    fn bytes(self) -> &'static [u8] {
        match self {
            Self::Lf => b"\n",
            Self::CrLf => b"\r\n",
            Self::None => b"",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Line {
    text: String,
    ending: Ending,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    pub path: String,
    lines: Vec<Line>,
}

impl File {
    fn parse(path: &str, bytes: &[u8]) -> Result<Self, String> {
        let text = core::str::from_utf8(bytes).map_err(|_| format!("{path} is not text"))?;
        let mut lines = Vec::new();
        for piece in text.split_inclusive('\n') {
            let (text, ending) = if let Some(text) = piece.strip_suffix("\r\n") {
                (text, Ending::CrLf)
            } else if let Some(text) = piece.strip_suffix('\n') {
                (text, Ending::Lf)
            } else {
                (piece, Ending::None)
            };
            lines.push(Line {
                text: text.to_string(),
                ending,
            });
        }
        if lines.len() > MAX_LINES {
            return Err(format!("{path} has more than {MAX_LINES} lines"));
        }
        Ok(Self {
            path: path.to_string(),
            lines,
        })
    }

    pub fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for line in &self.lines {
            out.extend_from_slice(line.text.as_bytes());
            out.extend_from_slice(line.ending.bytes());
        }
        out
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Line `n`, from 1.
    pub fn line(&self, n: usize) -> Option<&str> {
        n.checked_sub(1)
            .and_then(|i| self.lines.get(i))
            .map(|line| line.text.as_str())
    }

    /// The ending new lines get: the file's first, or LF.
    fn ending(&self) -> Ending {
        match self.lines.first().map(|line| line.ending) {
            Some(Ending::CrLf) => Ending::CrLf,
            _ => Ending::Lf,
        }
    }

    fn lacks_final_newline(&self) -> bool {
        self.lines
            .last()
            .is_some_and(|line| line.ending == Ending::None)
    }
}

/// Text typed for a line: one line, no control characters but tabs.
fn check_text(text: &str) -> Result<(), String> {
    if text.chars().any(|c| c.is_control() && c != '\t') {
        return Err("a line cannot hold a line break or control characters".into());
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct Snapshot {
    file: usize,
    before: File,
    line: usize,
}

#[derive(Clone, Debug)]
pub struct Session {
    pub component: String,
    /// The version opened, which a build is based on.
    pub label: String,
    original: Vec<u8>,
    files: Vec<File>,
    current: usize,
    /// The current line, from 1; 0 before the first.
    line: usize,
    undo: Vec<Snapshot>,
    /// The upload built from exactly this text; any edit clears it.
    pub built: Option<String>,
}

impl Session {
    /// Opens the source bundle of `component` version `label`.
    pub fn open(component: &str, label: &str, bundle: &[u8]) -> Result<Self, String> {
        if bundle.len() > MAX_BUNDLE {
            return Err(format!("the source is over {} KiB", MAX_BUNDLE / 1024));
        }
        let parts = artifact::source_files(bundle).map_err(String::from)?;
        if parts.is_empty() || parts.len() > MAX_FILES {
            return Err(format!("the source has {} files", parts.len()));
        }
        let files = parts
            .iter()
            .map(|(path, bytes)| File::parse(path, bytes))
            .collect::<Result<Vec<_>, _>>()?;
        let current = files
            .iter()
            .position(|f| f.path.ends_with("/src/lib.rs"))
            .unwrap_or(0);
        Ok(Self {
            component: component.to_string(),
            label: label.to_string(),
            original: bundle.to_vec(),
            files,
            current,
            line: 1,
            undo: Vec::new(),
            built: None,
        })
    }

    pub fn files(&self) -> &[File] {
        &self.files
    }

    pub fn current(&self) -> &File {
        &self.files[self.current]
    }

    pub fn line(&self) -> usize {
        self.line
    }

    /// The bundle as opened.
    pub fn original(&self) -> &[u8] {
        &self.original
    }

    /// The edited files as a bundle, byte for byte.
    pub fn to_bundle(&self) -> Vec<u8> {
        let bytes: Vec<Vec<u8>> = self.files.iter().map(File::bytes).collect();
        let parts: Vec<(&str, &[u8])> = self
            .files
            .iter()
            .zip(&bytes)
            .map(|(file, bytes)| (file.path.as_str(), bytes.as_slice()))
            .collect();
        artifact::bundle(&parts)
    }

    pub fn changed(&self) -> bool {
        self.to_bundle() != self.original
    }

    /// Whether file `index` differs from the opened version.
    pub fn file_changed(&self, index: usize) -> bool {
        let original = artifact::source_files(&self.original).unwrap_or_default();
        original
            .get(index)
            .is_none_or(|(_, bytes)| *bytes != self.files[index].bytes())
    }

    /// The file named `name`: its path, or a suffix of it after a `/` that only
    /// one file has.
    pub fn resolve(&self, name: &str) -> Result<usize, String> {
        if let Some(index) = self.files.iter().position(|f| f.path == name) {
            return Ok(index);
        }
        let suffix = format!("/{}", name.trim_start_matches('/'));
        let matches: Vec<usize> = (0..self.files.len())
            .filter(|&i| self.files[i].path.ends_with(&suffix))
            .collect();
        match matches[..] {
            [index] => Ok(index),
            [] => Err(format!("no file {name}")),
            _ => Err(format!(
                "{name} names {} files; give more of the path",
                matches.len()
            )),
        }
    }

    /// Makes `name` the current file.
    pub fn select(&mut self, name: &str) -> Result<(), String> {
        self.current = self.resolve(name)?;
        self.line = 1.min(self.current().len());
        Ok(())
    }

    /// Moves to `path` line `line`, if the session has that file.
    pub fn goto(&mut self, path: &str, line: usize) -> bool {
        let Some(index) = self.files.iter().position(|f| f.path == path) else {
            return false;
        };
        self.current = index;
        self.line = line.min(self.files[index].len());
        true
    }

    /// Every line holding `text`: (file, line), in file order.
    pub fn find(&self, text: &str) -> Vec<(usize, usize)> {
        let mut found = Vec::new();
        for (index, file) in self.files.iter().enumerate() {
            for (n, line) in file.lines.iter().enumerate() {
                if line.text.contains(text) {
                    found.push((index, n + 1));
                }
            }
        }
        found
    }

    fn check_line(&self, n: usize) -> Result<(), String> {
        let len = self.current().len();
        if n == 0 || n > len {
            return Err(format!(
                "line {n} is not in {} (1 to {len})",
                self.current().path
            ));
        }
        Ok(())
    }

    /// Runs one edit of the current file, so it can be undone.
    fn edit(&mut self, change: impl FnOnce(&mut File) -> usize) {
        let snapshot = Snapshot {
            file: self.current,
            before: self.files[self.current].clone(),
            line: self.line,
        };
        if self.undo.len() == UNDO {
            self.undo.remove(0);
        }
        self.undo.push(snapshot);
        self.line = change(&mut self.files[self.current]);
        self.built = None;
    }

    /// Replaces the one occurrence of `old` in the current file, or in line `n`.
    pub fn substitute(&mut self, n: Option<usize>, old: &str, new: &str) -> Result<usize, String> {
        if old.is_empty() {
            return Err("nothing to replace".into());
        }
        check_text(new)?;
        if let Some(n) = n {
            self.check_line(n)?;
        }
        let file = self.current();
        let mut hits = Vec::new();
        for (i, line) in file.lines.iter().enumerate() {
            if n.is_none_or(|n| n == i + 1) {
                hits.extend(line.text.match_indices(old).map(|(at, _)| (i, at)));
            }
        }
        let where_ = match n {
            Some(n) => format!("line {n}"),
            None => file.path.clone(),
        };
        let (index, at) = match hits[..] {
            [hit] => hit,
            [] => return Err(format!("{old:?} is not in {where_}")),
            _ => {
                return Err(format!(
                    "{old:?} is in {where_} {} times; give a line number or more text",
                    hits.len()
                ));
            }
        };
        self.edit(|file| {
            let text = &mut file.lines[index].text;
            text.replace_range(at..at + old.len(), new);
            index + 1
        });
        Ok(index + 1)
    }

    /// Replaces line `n` of the current file.
    pub fn change(&mut self, n: usize, text: &str) -> Result<(), String> {
        self.check_line(n)?;
        check_text(text)?;
        self.edit(|file| {
            file.lines[n - 1].text = text.to_string();
            n
        });
        Ok(())
    }

    /// Inserts a line so that it becomes line `n` (1 to one past the last).
    pub fn insert(&mut self, n: usize, text: &str) -> Result<(), String> {
        let len = self.current().len();
        if n == 0 || n > len + 1 {
            return Err(format!("cannot insert at line {n} (1 to {})", len + 1));
        }
        check_text(text)?;
        if self.current().len() >= MAX_LINES {
            return Err(format!("a file holds at most {MAX_LINES} lines"));
        }
        self.edit(|file| {
            let mut ending = file.ending();
            if n == len + 1 && file.lacks_final_newline() {
                // The old last line now ends; the new one has no newline, as before.
                file.lines[len - 1].ending = ending;
                ending = Ending::None;
            }
            let line = Line {
                text: text.to_string(),
                ending,
            };
            file.lines.insert(n - 1, line);
            n
        });
        Ok(())
    }

    /// Deletes lines `from` to `to`.
    pub fn delete(&mut self, from: usize, to: usize) -> Result<(), String> {
        self.check_line(from)?;
        self.check_line(to)?;
        if to < from {
            return Err(format!("{from},{to} is not a range"));
        }
        self.edit(|file| {
            let lacked = file.lacks_final_newline() && to == file.len();
            file.lines.drain(from - 1..to);
            if lacked && let Some(last) = file.lines.last_mut() {
                last.ending = Ending::None;
            }
            from.min(file.len())
        });
        Ok(())
    }

    /// Takes back the last edit; false when there is none.
    pub fn undo(&mut self) -> bool {
        let Some(snapshot) = self.undo.pop() else {
            return false;
        };
        self.files[snapshot.file] = snapshot.before;
        self.current = snapshot.file;
        self.line = snapshot.line;
        self.built = None;
        true
    }

    /// Back to the version as opened.
    pub fn reset(&mut self) {
        let fresh = Self::open(&self.component, &self.label, &self.original)
            .expect("the opened bundle opens again");
        *self = fresh;
    }
}
