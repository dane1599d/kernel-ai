//! Where a component panicked: the text a trap's detail carries,
//! `panicked at <path>:<line>:<col>`, and reading it back. A path is shown only
//! when it is a plain relative path inside the workspace; anything else (an
//! absolute path into a registry or the toolchain, `..`, odd bytes, a long path)
//! is cut to `.../<file name>`, so no host path reaches a log and no shown path
//! can be opened outside the component's source.

use alloc::format;
use alloc::string::String;

/// What a located trap's detail starts with.
pub const PANICKED_AT: &str = "panicked at ";
/// The longest path shown whole; with the prefix, line and column the detail fits
/// [`kabi::live::MAX_DETAIL`].
pub const MAX_PATH: usize = 64;
const MAX_LINE: u32 = 99_999;
const MAX_COLUMN: u32 = 9_999;

/// A source position read back from a detail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub path: String,
    pub line: u32,
    pub column: u32,
}

/// `raw` as it may be shown: `\` as `/`, and anything but a plain relative path cut
/// to `.../<file name>`.
pub fn shown_path(raw: &str) -> String {
    let path = raw.replace('\\', "/");
    let plain = !path.is_empty()
        && path.len() <= MAX_PATH
        && !path.starts_with('/')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
        && path.bytes().all(allowed);
    if plain {
        return path;
    }
    let name: String = path
        .rsplit('/')
        .next()
        .unwrap_or("")
        .chars()
        .filter(|&c| c.is_ascii() && allowed(c as u8))
        .take(MAX_PATH)
        .collect();
    format!(".../{name}")
}

fn allowed(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'/' | b'-')
}

/// The detail of a trap that panicked at `raw:line:column`. A line or column past
/// what any source here has counts as unknown and is left out.
pub fn panicked(raw: &str, line: u32, column: u32) -> String {
    let path = shown_path(raw);
    match (line, column) {
        (1..=MAX_LINE, 1..=MAX_COLUMN) => format!("{PANICKED_AT}{path}:{line}:{column}"),
        (1..=MAX_LINE, _) => format!("{PANICKED_AT}{path}:{line}"),
        _ => format!("{PANICKED_AT}{path}"),
    }
}

/// The position in a detail [`panicked`] wrote, if it names a line in a path that
/// can be opened (not one cut to `.../`).
pub fn parse(detail: &str) -> Option<Location> {
    let rest = &detail[detail.find(PANICKED_AT)? + PANICKED_AT.len()..];
    let mut parts = rest.split(':');
    let path = parts.next()?;
    let line = parts.next()?.parse().ok()?;
    let column = match parts.next() {
        Some(column) => column.parse().ok()?,
        None => 0,
    };
    if parts.next().is_some() || path.starts_with(".../") || shown_path(path) != path {
        return None;
    }
    Some(Location {
        path: String::from(path),
        line,
        column,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_workspace_paths_are_shown_with_forward_slashes() {
        assert_eq!(
            shown_path("components\\notes\\src\\lib.rs"),
            "components/notes/src/lib.rs"
        );
        assert_eq!(shown_path("src/lib.rs"), "src/lib.rs");
    }

    #[test]
    fn host_paths_and_odd_ones_are_cut_to_the_file_name() {
        for (raw, shown) in [
            (
                "C:\\Users\\someone\\.cargo\\registry\\src\\minicbor-2.3.0\\src\\decode.rs",
                ".../decode.rs",
            ),
            ("/rustc/abc/library/core/src/slice/index.rs", ".../index.rs"),
            ("../outside/src/lib.rs", ".../lib.rs"),
            ("components/./notes/lib.rs", ".../lib.rs"),
            ("src/l\u{e9}b.rs", ".../lb.rs"),
            ("src/lib.rs\n", ".../lib.rs"),
            ("", ".../"),
        ] {
            assert_eq!(shown_path(raw), shown, "{raw:?}");
        }
        let long = format!("{}/x.rs", "a".repeat(MAX_PATH));
        assert_eq!(shown_path(&long), ".../x.rs");
    }

    #[test]
    fn a_panic_reads_back_as_its_location() {
        let detail = panicked("components\\notes\\src\\lib.rs", 58, 47);
        assert_eq!(detail, "panicked at components/notes/src/lib.rs:58:47");
        assert!(detail.len() <= kabi::live::MAX_DETAIL);
        assert_eq!(
            parse(&detail),
            Some(Location {
                path: String::from("components/notes/src/lib.rs"),
                line: 58,
                column: 47,
            })
        );
        assert_eq!(
            parse("trap: panicked at src/lib.rs:3").map(|at| (at.line, at.column)),
            Some((3, 0))
        );
    }

    #[test]
    fn unknown_lines_and_cut_paths_are_not_positions() {
        assert_eq!(panicked("src/lib.rs", 0, 5), "panicked at src/lib.rs");
        assert_eq!(
            panicked("src/lib.rs", 7, 10_000),
            "panicked at src/lib.rs:7"
        );
        assert_eq!(parse("panicked at src/lib.rs"), None);
        assert_eq!(parse("panicked at .../index.rs:12:5"), None);
        assert_eq!(parse("panicked at src/lib.rs:x:5"), None);
        assert_eq!(parse("unreachable executed"), None);
        let longest = panicked(&"a".repeat(MAX_PATH), MAX_LINE, MAX_COLUMN);
        assert!(longest.len() <= kabi::live::MAX_DETAIL, "{longest}");
    }
}
