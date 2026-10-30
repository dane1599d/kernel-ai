//! Names packages may not take (ADR 0011 §2). A package's actions are named
//! `<package>_<function>`, so a package named like an OS row group would put its
//! actions among the OS's own: package `fs` exporting `list` gives `fs_list`.
//! CI, `pkg` and the registry check this one list.

use crate::{HELPERS, ROWS, TYPED_ONLY};

/// Every OS row group, reserved as a package name: ADR 0011 §2's groups, then
/// the first word of every row today that is not one of them. A test holds
/// every row's first word to this list, so a new group cannot be missed.
pub const RESERVED_GROUPS: &[&str] = &[
    // ADR 0011 §2.
    "fs",
    "ui",
    "pkg",
    "vm",
    "app",
    "browser",
    "window",
    "media",
    "system",
    "device",
    "power",
    "update",
    "model",
    "linux",
    "net",
    "appearance",
    // Today's rows (M8's names and P1's).
    "cpus",
    "memory",
    "uptime",
    "processes",
    "threads",
    "get",
    "set",
    "process",
    "service",
    "fixed",
    "floor",
    "tune",
    "component",
    "rules",
    // P4 task 7.
    "grant",
];

/// Whether `package` is reserved: an OS row group.
pub fn is_reserved(package: &str) -> bool {
    RESERVED_GROUPS.contains(&package)
}

/// Whether package `package`'s function `function` would take an OS row's name:
/// `<package>_<function>`, a `-` read as `_` (`fs-x` + `y` is `fs_x_y`). Such a
/// package is refused.
pub fn takes_os_name(package: &str, function: &str) -> bool {
    let name = alloc::format!("{package}_{function}").replace('-', "_");
    ROWS.iter()
        .chain(&TYPED_ONLY)
        .chain(&HELPERS)
        .any(|row| row.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BOUND, CALL_ONLY, COMMANDS, SCRIPT_API};

    #[test]
    fn every_rows_first_word_is_reserved() {
        let all = ROWS
            .iter()
            .chain(&TYPED_ONLY)
            .chain(&HELPERS)
            .chain(&BOUND)
            .chain(&CALL_ONLY)
            .chain(&COMMANDS)
            .chain(&SCRIPT_API);
        for row in all {
            let group = row.name.split('_').next().unwrap_or(row.name);
            assert!(is_reserved(group), "{} is not in RESERVED_GROUPS", row.name);
        }
    }

    #[test]
    fn reserved_names_are_package_names_and_listed_once() {
        for (i, name) in RESERVED_GROUPS.iter().enumerate() {
            assert!(!RESERVED_GROUPS[..i].contains(name), "{name} twice");
            let ok = (2..=12).contains(&name.len())
                && name.starts_with(|c: char| c.is_ascii_lowercase())
                && name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
            assert!(ok, "{name} is not a package name");
        }
    }

    #[test]
    fn a_function_named_like_an_os_row_is_caught() {
        assert!(takes_os_name("set", "policy"));
        assert!(takes_os_name("pkg", "install"));
        assert!(takes_os_name("process", "stop"));
        assert!(takes_os_name("component", "revert"));
        assert!(!takes_os_name("notes", "open"));
        assert!(!takes_os_name("pdf", "view_open"));
    }
}
