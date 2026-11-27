//! Entry names: each one checked to be a plain relative path, and the set of
//! them checked for duplicates and for files with entries under them.

use super::{Error, Limits};
use std::collections::BTreeMap;

/// The longest name, in bytes.
pub const MAX_NAME: usize = 1024;
/// The longest part of a name, in bytes.
pub const MAX_PART: usize = 255;

/// `raw`, an entry's name as its bytes are, for a refusal that names it
/// before it is checked: lossy, with what could break the message's line or
/// make it read as another name escaped: control characters, every white
/// space but a plain space (U+2028 and U+2029 split lines), the bidi controls
/// and the Arabic letter mark (which reorder what follows), the zero-width
/// characters and the backslash (so the text `\n` differs from a newline).
pub fn shown(raw: &[u8]) -> String {
    let lossy = String::from_utf8_lossy(raw);
    let mut out = String::with_capacity(lossy.len());
    for ch in lossy.chars() {
        let unseen = matches!(
            ch,
            '\u{200b}'..='\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2060}'
                | '\u{2066}'..='\u{2069}'
                | '\u{061c}'
                | '\u{feff}'
        ) || (ch.is_whitespace() && ch != ' ');
        if ch == '\\' {
            out.push_str("\\\\");
        } else if ch.is_control() {
            out.extend(ch.escape_debug());
        } else if unseen {
            out.extend(ch.escape_unicode());
        } else {
            out.push(ch);
        }
    }
    out
}

/// `raw`, an entry's name, as a relative path of plain parts joined by `/`,
/// without the trailing `/` that marks a folder; whether it was a folder.
pub fn check(raw: &str, limits: &Limits) -> Result<(String, bool), Error> {
    let refuse = |why| {
        Err(Error::Name {
            name: raw.into(),
            why,
        })
    };
    if raw.is_empty() {
        return refuse("it is empty");
    }
    if raw.len() > MAX_NAME {
        return refuse("it is too long");
    }
    if raw.starts_with('/') {
        return refuse("it starts at the root");
    }
    if raw.contains('\\') {
        return refuse("it has a backslash");
    }
    if raw.contains(':') {
        return refuse("it has a colon, as a drive letter or a stream would");
    }
    if raw.chars().any(char::is_control) {
        return refuse("it has a control character");
    }
    let (path, dir) = match raw.strip_suffix('/') {
        Some(path) => (path, true),
        None => (raw, false),
    };
    let mut depth = 0;
    for part in path.split('/') {
        depth += 1;
        match part {
            "" => return refuse("it has an empty part"),
            "." | ".." => return refuse("it has a . or .. part"),
            part if part.len() > MAX_PART => return refuse("a part of it is too long"),
            _ => {}
        }
    }
    if depth > limits.max_depth {
        return refuse("it is too deep");
    }
    Ok((path.into(), dir))
}

/// The names taken so far, each a file or a folder.
#[derive(Default)]
pub struct Names {
    taken: BTreeMap<String, bool>,
}

impl Names {
    /// Takes `path` as a folder (`dir`) or a file: refused if it is taken
    /// already, if a file stands where one of its folders would be, or if it
    /// is a file and something is under it. A folder named again, or named
    /// after an entry under it made it, is the same folder; whether `path` is
    /// new.
    pub fn take(&mut self, path: &str, dir: bool) -> Result<bool, Error> {
        let duplicate = || Error::Duplicate { name: path.into() };
        for (at, _) in path.match_indices('/') {
            if self.taken.get(&path[..at]) == Some(&false) {
                return Err(duplicate());
            }
        }
        if let Some(&was_dir) = self.taken.get(path) {
            return match was_dir && dir {
                true => Ok(false),
                false => Err(duplicate()),
            };
        }
        if !dir {
            let under = format!("{path}/");
            if self
                .taken
                .range(under.clone()..)
                .next()
                .is_some_and(|(k, _)| k.starts_with(&under))
            {
                return Err(duplicate());
            }
        }
        self.taken.insert(path.into(), dir);
        Ok(true)
    }
}
