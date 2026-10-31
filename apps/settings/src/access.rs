//! "What each app can do" (ADR 0001 D8, P4 task 7): the effect rows each app
//! declares, as the shell lists them: `grants` names the apps that have any,
//! and `grants <app>` gives one line a row, `<app>\t<row>\tallowed` or
//! `withheld`, so no answer outgrows one message. Settings withholds a
//! row and gives it back; it never makes a grant. Both go through the shell,
//! which logs them and undoes them, and a row stays listed whichever it is.

use alloc::string::String;
use alloc::vec::Vec;

/// Longest app or row name the shell holds (`kabi::MAX_NAME`).
pub const MAX_NAME: usize = 24;
/// Most rows read from one answer.
pub const MAX_ROWS: usize = 1024;
/// Most apps read from `grants` (the shell lists at most as many).
pub const MAX_APPS: usize = 32;

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

/// The apps with effect rows, from the shell's `grants` text (one name a line).
/// A line that is not an app name is left out, and so is a name listed twice;
/// at most [`MAX_APPS`] are kept. `None` if there were lines and none of them
/// was an app name.
pub fn parse_apps(text: &str) -> Option<Vec<String>> {
    let mut apps: Vec<String> = Vec::new();
    let mut seen = 0;
    for line in text.lines().filter(|l| !l.is_empty()) {
        seen += 1;
        if app_ok(line) && apps.len() < MAX_APPS && !apps.iter().any(|a| a == line) {
            apps.push(String::from(line));
        }
    }
    (seen == 0 || !apps.is_empty()).then_some(apps)
}

/// `app`'s rows from the shell's `grants <app>` text. Only lines for `app`
/// count, so an answer never adds another app's rows. Lines that are not rows
/// are left out, and so is a row listed twice (the first stands); `None` if
/// there were lines and none of them was a row of `app`.
pub fn parse_app(app: &str, text: &str) -> Option<Vec<Access>> {
    let mut rows: Vec<Access> = Vec::new();
    let mut seen = 0;
    for line in text.lines().filter(|l| !l.is_empty()) {
        seen += 1;
        let Some((of, name, allowed)) = row(line) else {
            continue;
        };
        if of != app || rows.len() == MAX_ROWS || rows.iter().any(|x| x.row == name) {
            continue;
        }
        rows.push(Access {
            row: String::from(name),
            allowed,
        });
    }
    (seen == 0 || !rows.is_empty()).then_some(rows)
}

/// Makes `rows` the rows of `app` in `access`, which is kept by app name; no
/// rows takes the app out.
pub fn put(access: &mut Vec<AppAccess>, app: &str, rows: Vec<Access>) {
    let at = access.binary_search_by(|a| a.app.as_str().cmp(app));
    match (at, rows.is_empty()) {
        (Ok(at), true) => {
            access.remove(at);
        }
        (Ok(at), false) => access[at].access = rows,
        (Err(at), false) => access.insert(
            at,
            AppAccess {
                app: String::from(app),
                access: rows,
            },
        ),
        (Err(_), true) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

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
    fn grants_names_the_apps_and_leaves_out_what_is_not_a_name() {
        let got =
            parse_apps("notes\nfiles-window\nnotes\nNotes\nbad name\nnotes\tfs_list\tallowed\n\n");
        assert_eq!(
            got,
            Some(vec![String::from("notes"), String::from("files-window")])
        );
        assert_eq!(parse_apps(""), Some(Vec::new()));
        assert_eq!(parse_apps("Notes\nx y"), None);
        let many: String = (0..MAX_APPS + 10)
            .map(|i| alloc::format!("app{i}\n"))
            .collect();
        assert_eq!(parse_apps(&many).unwrap().len(), MAX_APPS);
    }

    #[test]
    fn an_apps_rows_are_only_lines_for_that_app() {
        let text = "notes\tnotes_add\tallowed\n\
                    files\tfs_list\tallowed\n\
                    notes\tfs_write\twithheld\n\
                    notes\tnotes_add\twithheld\n";
        let got = parse_app("notes", text).unwrap();
        let rows: Vec<(&str, bool)> = got.iter().map(|x| (x.row.as_str(), x.allowed)).collect();
        assert_eq!(rows, [("notes_add", true), ("fs_write", false)]);
        // Rows of another app are not this app's; nothing of its own is none.
        assert_eq!(parse_app("browser", text), None);
        assert_eq!(parse_app("browser", ""), Some(Vec::new()));
    }

    #[test]
    fn put_keeps_the_apps_in_order_and_an_app_with_no_rows_goes() {
        let row = |name: &str| Access {
            row: String::from(name),
            allowed: true,
        };
        let mut access = Vec::new();
        put(&mut access, "notes", vec![row("a")]);
        put(&mut access, "files", vec![row("b")]);
        let names: Vec<&str> = access.iter().map(|a| a.app.as_str()).collect();
        assert_eq!(names, ["files", "notes"]);
        put(&mut access, "notes", vec![row("c"), row("d")]);
        assert_eq!(access[1].access.len(), 2);
        put(&mut access, "files", Vec::new());
        put(&mut access, "nobody", Vec::new());
        assert_eq!(access.len(), 1);
        assert_eq!(access[0].app, "notes");
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
