//! Names and paths as the file rows take them (`agentabi/src/files.rs`): text
//! under `/home/user`, relative, at most 255 bytes. What `fs_list` returns is
//! content of the user's files, so every name is checked before the app uses it.

use alloc::string::String;
use alloc::vec::Vec;

/// The longest path or name a file row takes.
pub const MAX_PATH: usize = 255;
/// The most names `fs_list` gives.
pub const MAX_ITEMS: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Dir,
    File,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub name: String,
    pub kind: Kind,
}

/// Whether `name` is one name in a folder: not empty, not `.` or `..`, no `/`
/// and no control characters, and not padded with spaces.
pub fn name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_PATH
        && name != "."
        && name != ".."
        && name.trim() == name
        && !name.chars().any(|c| c == '/' || c.is_control())
}

/// Whether `path` is a path the file rows take: relative to `/home/user`, `""`
/// for the folder itself, names joined by single slashes.
pub fn path_ok(path: &str) -> bool {
    path.is_empty() || (path.len() <= MAX_PATH && path.split('/').all(name_ok))
}

/// `name` in `dir`, if both are fine and the result fits.
pub fn join(dir: &str, name: &str) -> Option<String> {
    if !path_ok(dir) || !name_ok(name) {
        return None;
    }
    let path = match dir.is_empty() {
        true => String::from(name),
        false => alloc::format!("{dir}/{name}"),
    };
    (path.len() <= MAX_PATH).then_some(path)
}

/// The folder `path` is in; `""` for a top-level name and for `""`.
pub fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// The last name of `path`.
pub fn file_name(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, name)| name)
}

/// What a listing made of `fs_list`'s text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listing {
    /// Folders first, then files, each in order of lower-case name.
    pub items: Vec<Item>,
    /// Lines left out: a bad name, a repeat, or past [`MAX_ITEMS`].
    pub skipped: usize,
    /// Whether the folder may hold more than `items` shows: `fs_list` stops at
    /// [`MAX_ITEMS`].
    pub more: bool,
}

/// `fs_list`'s text: one name a line, a folder's with a `/` after it.
pub fn parse_listing(text: &str) -> Listing {
    let mut out = Listing::default();
    for line in text.lines() {
        let (name, kind) = match line.strip_suffix('/') {
            Some(name) => (name, Kind::Dir),
            None => (line, Kind::File),
        };
        let repeat = out.items.iter().any(|i| i.name == name);
        if !name_ok(name) || repeat || out.items.len() == MAX_ITEMS {
            out.skipped += 1;
            continue;
        }
        out.items.push(Item {
            name: String::from(name),
            kind,
        });
    }
    out.more = out.items.len() == MAX_ITEMS;
    out.items
        .sort_by_cached_key(|i| (i.kind == Kind::File, i.name.to_lowercase(), i.name.clone()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;

    #[test]
    fn names_and_paths_are_what_the_rows_take() {
        for name in ["a", "notes.txt", "my file", "é", ".hidden", "a..b"] {
            assert!(name_ok(name), "{name}");
        }
        for name in ["", ".", "..", "a/b", " a", "a ", "a\nb", "a\u{7}", "\u{85}"] {
            assert!(!name_ok(name), "{name:?}");
        }
        assert!(!name_ok(&"x".repeat(256)));
        assert!(path_ok("") && path_ok("a") && path_ok("a/b/c.txt"));
        for path in ["/a", "a/", "a//b", "../a", "a/../b", "a/./b", "./a"] {
            assert!(!path_ok(path), "{path}");
        }
        assert!(!path_ok(&format!(
            "{}/{}",
            "x".repeat(200),
            "y".repeat(100)
        )));
    }

    #[test]
    fn join_parent_and_file_name_agree() {
        assert_eq!(join("", "a.txt").as_deref(), Some("a.txt"));
        assert_eq!(join("docs", "a.txt").as_deref(), Some("docs/a.txt"));
        assert_eq!(join("docs", "../a"), None);
        assert_eq!(join("../docs", "a"), None);
        assert_eq!(join(&"d".repeat(255), "a"), None);
        assert_eq!((parent("a/b/c"), file_name("a/b/c")), ("a/b", "c"));
        assert_eq!((parent("c"), file_name("c")), ("", "c"));
        assert_eq!((parent(""), file_name("")), ("", ""));
    }

    #[test]
    fn a_listing_sorts_folders_first_and_drops_what_cannot_be_used() {
        let text = "b.txt\nDocs/\n../x\nA.txt\nb.txt\n\nlogs/\nok/../no\n a\nbad\u{1b}[0m";
        let l = parse_listing(text);
        let names: Vec<_> = l.items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["Docs", "logs", "A.txt", "b.txt"]);
        assert_eq!(l.items[0].kind, Kind::Dir);
        assert_eq!(l.items[2].kind, Kind::File);
        assert_eq!(l.skipped, 6);
        assert!(!l.more);
    }

    #[test]
    fn a_full_listing_says_there_may_be_more() {
        let text: String = (0..MAX_ITEMS + 50).map(|i| format!("f{i}\n")).collect();
        let l = parse_listing(&text);
        assert_eq!(l.items.len(), MAX_ITEMS);
        assert_eq!(l.skipped, 50);
        assert!(l.more);
    }
}
