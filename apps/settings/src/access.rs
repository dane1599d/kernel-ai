//! "What each app can do" (ADR 0001 D8, P4 task 7): the effect rows each app
//! declares, as the shell lists them (`grants`: one line a row,
//! `<app>\t<row>\tallowed` or `withheld`), grouped by app. Settings withholds a
//! row and gives it back; it never makes a grant. Both go through the shell,
//! which logs them and undoes them, and a row stays listed whichever it is.

use alloc::string::String;
use alloc::vec::Vec;

/// Longest app or row name the shell holds (`kabi::MAX_NAME`).
pub const MAX_NAME: usize = 24;
/// Most rows read from one answer.
pub const MAX_ROWS: usize = 1024;

/// One effect row of an app, and whether its effects may call it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Access {
    /// The row's name, such as `notes_add`.
    pub row: String,
    pub allowed: bool,
}

/// One app and the rows its package declares, in the order the shell lists them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppAccess {
    pub app: String,
    pub access: Vec<Access>,
}

/// An app as the shell names it: 1 to [`MAX_NAME`] lowercase letters, digits
/// and `-` (wasmhost's `grants <app>` takes the same).
pub fn app_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// An effect row as the shell names it: a lowercase letter, then lowercase
/// letters, digits and `_`, at most [`MAX_NAME`] bytes.
pub fn row_ok(name: &str) -> bool {
    name.len() <= MAX_NAME
        && name.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// One line of the shell's answer: its app, row and whether it is allowed. A
/// line out of shape, or with a name the shell would not hold, is not a row.
fn row(line: &str) -> Option<(&str, &str, bool)> {
    let mut fields = line.split('\t');
    let (app, row, state) = (fields.next()?, fields.next()?, fields.next()?);
    let allowed = match state {
        "allowed" => true,
        "withheld" => false,
        _ => return None,
    };
    (fields.next().is_none() && app_ok(app) && row_ok(row)).then_some((app, row, allowed))
}

/// Every app with at least one row, by name, from the shell's `grants` text.
/// Lines that are not rows are left out, and so is a row listed twice (the
/// first stands); `None` if there were lines and none of them was a row.
pub fn parse(text: &str) -> Option<Vec<AppAccess>> {
    let mut apps: Vec<AppAccess> = Vec::new();
    let mut seen = 0;
    let mut kept = 0;
    for line in text.lines().filter(|l| !l.is_empty()) {
        seen += 1;
        let Some((app, name, allowed)) = row(line) else {
            continue;
        };
        if kept == MAX_ROWS {
            break;
        }
        let at = match apps.binary_search_by(|a| a.app.as_str().cmp(app)) {
            Ok(at) => at,
            Err(at) => {
                apps.insert(
                    at,
                    AppAccess {
                        app: String::from(app),
                        access: Vec::new(),
                    },
                );
                at
            }
        };
        if apps[at].access.iter().any(|x| x.row == name) {
            continue;
        }
        apps[at].access.push(Access {
            row: String::from(name),
            allowed,
        });
        kept += 1;
    }
    (seen == 0 || kept > 0).then_some(apps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_the_shells_shapes() {
        let longest = "a".repeat(MAX_NAME);
        let too_long = "a".repeat(MAX_NAME + 1);
        for ok in ["notes", "files-window", "app2", "a", longest.as_str()] {
            assert!(app_ok(ok), "{ok}");
        }
        for bad in ["", "Notes", "notes_app", "a b", "a\tb", too_long.as_str()] {
            assert!(!app_ok(bad), "{bad:?}");
        }
        for ok in ["notes_add", "fs_list", "a", "files_open2", longest.as_str()] {
            assert!(row_ok(ok), "{ok}");
        }
        for bad in [
            "",
            "Notes_add",
            "_a",
            "1a",
            "notes-add",
            "a b",
            too_long.as_str(),
        ] {
            assert!(!row_ok(bad), "{bad:?}");
        }
    }

    #[test]
    fn rows_group_by_app_in_the_order_listed() {
        let got = parse(
            "notes\tnotes_add\tallowed\nfiles\tfs_list\twithheld\nnotes\tfs_write\twithheld\n",
        )
        .unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].app, "files");
        assert_eq!(got[1].app, "notes");
        let rows: Vec<(&str, bool)> = got[1]
            .access
            .iter()
            .map(|x| (x.row.as_str(), x.allowed))
            .collect();
        assert_eq!(rows, [("notes_add", true), ("fs_write", false)]);
    }

    #[test]
    fn a_line_out_of_shape_is_left_out_and_the_first_of_a_repeat_stands() {
        let got = parse(
            "notes\tnotes_add\tallowed\n\
             notes\tnotes_add\twithheld\n\
             Notes\tx\tallowed\n\
             notes\tfs_list\tmaybe\n\
             notes\tfs_list\tallowed\textra\n\
             notes fs_list allowed\n\
             notes\t\tallowed\n",
        )
        .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].access.len(), 1);
        assert!(got[0].access[0].allowed);
    }

    #[test]
    fn nothing_listed_is_an_empty_list_and_only_junk_is_refused() {
        assert_eq!(parse(""), Some(Vec::new()));
        assert_eq!(parse("\n\n"), Some(Vec::new()));
        assert_eq!(parse("not kai-grants"), None);
        assert_eq!(parse("kai-grants 1\n"), None);
    }

    #[test]
    fn rows_are_cut_to_a_count() {
        let many: String = (0..MAX_ROWS + 50)
            .map(|i| alloc::format!("app\trow{i}\tallowed\n"))
            .collect();
        let got = parse(&many).unwrap();
        assert_eq!(got[0].access.len(), MAX_ROWS);
    }
}
