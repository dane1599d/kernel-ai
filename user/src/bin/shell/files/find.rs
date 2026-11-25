//! `fs_find`: a breadth-first search of `/home/user` by name.

use super::{in_dir, list_cut};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use kabi::handle::{Handle, Rights};

/// The most folders `fs_find` looks in, and the most paths it gives back.
const FIND_DIRS: usize = 64;
const FIND_MAX: usize = 40;

/// `fs_find`: paths under home whose last part holds `word`, case aside,
/// looking in home and the folders below it, breadth first, within
/// [`FIND_DIRS`] folders and [`FIND_MAX`] paths. A folder it may not read is
/// skipped, not the whole search; last lines say when more matched than it
/// gives, when it left folders or names unvisited, and how many it skipped
/// (#899's review), so a short list is never taken as all there is.
pub(super) fn find(home: Handle, word: &str) -> Result<Vec<String>, String> {
    let word = word.trim().to_lowercase();
    if word.is_empty() {
        return Err(String::from("say what the name holds"));
    }
    let mut dirs = alloc::collections::VecDeque::from([String::new()]);
    let (mut seen, mut skipped, mut found) = (0, 0, Vec::new());
    // More matched than FIND_MAX; folders left unvisited (FIND_DIRS, or a
    // folder longer than LIST_MAX names).
    let (mut more_matched, mut cut) = (false, false);
    while let Some(dir) = dirs.pop_front() {
        seen += 1;
        let names = match in_dir(home, &dir, Rights::READ | Rights::ENUMERATE, list_cut) {
            Ok((names, long)) => {
                cut |= long;
                names
            }
            // Home itself unreadable is the search failing.
            Err(why) if dir.is_empty() => return Err(why),
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        for name in names {
            let (bare, folder) = match name.strip_suffix('/') {
                Some(bare) => (bare, true),
                None => (name.as_str(), false),
            };
            let path = match dir.is_empty() {
                true => String::from(bare),
                false => format!("{dir}/{bare}"),
            };
            if bare.to_lowercase().contains(&word) {
                match found.len() < FIND_MAX {
                    true => found.push(format!(
                        "/home/user/{path}{}",
                        if folder { "/" } else { "" }
                    )),
                    false => more_matched = true,
                }
            }
            if folder {
                match seen + dirs.len() < FIND_DIRS {
                    true => dirs.push_back(path),
                    false => cut = true,
                }
            }
        }
    }
    if more_matched {
        found.push(format!("(the first {FIND_MAX} matches; more match)"));
    }
    if cut {
        found.push(format!(
            "(stopped early after {seen} folders: more may match)"
        ));
    }
    if skipped > 0 {
        found.push(format!("({skipped} folders could not be read)"));
    }
    Ok(found)
}
