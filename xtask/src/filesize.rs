//! `cargo xtask filesize [check | update]`: no tracked source file grows past
//! [`LIMIT`] lines.
//!
//! A file that was already longer when the check came in is listed in
//! [`ALLOW`] with its count then. `check` (CI's lint runs it) fails when a file
//! not on the list passes the limit, or a listed one grows past its count. A
//! file that shrank, or came under the limit, only gets a note: its line may be
//! lowered or dropped, but nothing forces two lanes to edit the list at once.
//! `update` rewrites the list from the tree, for a PR that splits a file.
//! Raising a count is a reviewed change to the list, in the PR that needs it.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;

/// The longest a source file may grow.
pub const LIMIT: usize = 800;
/// The files over [`LIMIT`] and the count each may reach.
pub const ALLOW: &str = "docs/filesize-allow.txt";
/// What counts as source: code people edit.
const EXTENSIONS: [&str; 5] = ["rs", "c", "h", "sh", "wit"];
/// Trees that are not ours to split.
const SKIPPED: [&str; 1] = ["third_party/"];

const USAGE: &str = "usage: cargo xtask filesize [check | update]";

const HEADER: &str = "\
# Source files over 800 lines, each with the count it may reach (`cargo xtask
# filesize`). A file here may shrink; growing past its count fails CI's lint.
# Split a file rather than raise its count; `cargo xtask filesize update`
# rewrites this list from the tree.
";

/// What `check` found: errors fail it, notes do not.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub errors: Vec<String>,
    pub notes: Vec<String>,
}

pub fn main(args: &[String]) -> Result<(), String> {
    let root = crate::workspace_root();
    match args {
        [] => run_check(&root),
        [word] if word == "check" => run_check(&root),
        [word] if word == "update" => run_update(&root),
        _ => Err(String::from(USAGE)),
    }
}

fn run_check(root: &Path) -> Result<(), String> {
    let counts = count_tree(root)?;
    let text = fs::read_to_string(root.join(ALLOW)).map_err(|e| format!("{ALLOW}: {e}"))?;
    let report = check(&counts, &parse_allow(&text)?);
    for note in &report.notes {
        println!("xtask: filesize: note: {note}");
    }
    if report.errors.is_empty() {
        println!(
            "xtask: filesize: {} source files, none grown past {LIMIT} lines or its count in {ALLOW}",
            counts.len()
        );
        return Ok(());
    }
    for error in &report.errors {
        eprintln!("xtask: filesize: {error}");
    }
    Err(format!(
        "{} file(s) too long: split them, or raise a count in {ALLOW} with the reviewer's OK",
        report.errors.len()
    ))
}

fn run_update(root: &Path) -> Result<(), String> {
    let text = render_allow(&count_tree(root)?);
    fs::write(root.join(ALLOW), &text).map_err(|e| format!("{ALLOW}: {e}"))?;
    println!(
        "xtask: filesize: wrote {ALLOW} ({} files)",
        text.lines().filter(|l| !l.starts_with('#')).count()
    );
    Ok(())
}

/// Every tracked source file and its line count, by path.
fn count_tree(root: &Path) -> Result<BTreeMap<String, usize>, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-z"])
        .output()
        .map_err(|e| format!("git ls-files: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git ls-files: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let mut counts = BTreeMap::new();
    for path in out.stdout.split(|&b| b == 0).filter(|p| !p.is_empty()) {
        let path = String::from_utf8_lossy(path).into_owned();
        if !is_source(&path) {
            continue;
        }
        // A tracked file deleted in the work tree is not there to count.
        let Ok(bytes) = fs::read(root.join(&path)) else {
            continue;
        };
        counts.insert(path, lines(&bytes));
    }
    Ok(counts)
}

fn is_source(path: &str) -> bool {
    !SKIPPED.iter().any(|dir| path.starts_with(dir))
        && path
            .rsplit_once('.')
            .is_some_and(|(_, ext)| EXTENSIONS.contains(&ext))
}

/// Lines as `wc -l` counts them, plus a last line with no newline.
fn lines(bytes: &[u8]) -> usize {
    let newlines = bytes.iter().filter(|&&b| b == b'\n').count();
    newlines + usize::from(bytes.last().is_some_and(|&b| b != b'\n'))
}

/// The allow-list's `<count> <path>` lines; `#` starts a comment line.
pub fn parse_allow(text: &str) -> Result<BTreeMap<String, usize>, String> {
    let mut allow = BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let at = || format!("{ALLOW} line {}", index + 1);
        let (count, path) = line
            .split_once(' ')
            .ok_or_else(|| format!("{}: want `<count> <path>`, got `{line}`", at()))?;
        let count: usize = count
            .parse()
            .map_err(|_| format!("{}: `{count}` is not a line count", at()))?;
        if allow.insert(path.trim().to_string(), count).is_some() {
            return Err(format!("{}: {} is listed twice", at(), path.trim()));
        }
    }
    Ok(allow)
}

/// The files over the limit, measured against the list.
pub fn check(counts: &BTreeMap<String, usize>, allow: &BTreeMap<String, usize>) -> Report {
    let mut report = Report::default();
    for (path, &count) in counts {
        match allow.get(path) {
            None if count > LIMIT => report
                .errors
                .push(format!("{path}: {count} lines, past the {LIMIT}-line limit")),
            Some(&allowed) if count > allowed.max(LIMIT) => report.errors.push(format!(
                "{path}: grew to {count} lines, past its {allowed} in {ALLOW}"
            )),
            Some(&allowed) if count <= LIMIT => report.notes.push(format!(
                "{path} is {count} lines, under the limit: its line ({allowed}) can come off {ALLOW}"
            )),
            Some(&allowed) if count < allowed => report.notes.push(format!(
                "{path} shrank to {count} lines: its line in {ALLOW} can go down from {allowed}"
            )),
            _ => {}
        }
    }
    for (path, allowed) in allow {
        if !counts.contains_key(path) {
            report.notes.push(format!(
                "{path} ({allowed}) is not a tracked source file any more: its line can come off {ALLOW}"
            ));
        }
    }
    report
}

/// The list for this tree: every file over the limit, at its count.
pub fn render_allow(counts: &BTreeMap<String, usize>) -> String {
    let mut text = String::from(HEADER);
    for (path, count) in counts.iter().filter(|&(_, &count)| count > LIMIT) {
        text.push_str(&format!("{count} {path}\n"));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(rows: &[(&str, usize)]) -> BTreeMap<String, usize> {
        rows.iter().map(|&(p, n)| (p.to_string(), n)).collect()
    }

    #[test]
    fn a_new_file_past_the_limit_fails() {
        let report = check(&map(&[("a.rs", 801), ("b.rs", 800)]), &map(&[]));
        assert_eq!(report.errors.len(), 1);
        assert!(report.errors[0].starts_with("a.rs: 801 lines"));
    }

    #[test]
    fn a_listed_file_may_reach_its_count_but_not_pass_it() {
        let allow = map(&[("big.rs", 1000)]);
        assert_eq!(check(&map(&[("big.rs", 1000)]), &allow), Report::default());
        let report = check(&map(&[("big.rs", 1001)]), &allow);
        assert_eq!(
            report.errors,
            vec![format!(
                "big.rs: grew to 1001 lines, past its 1000 in {ALLOW}"
            )]
        );
    }

    #[test]
    fn a_file_that_shrank_or_went_only_gets_a_note() {
        let allow = map(&[("big.rs", 1000), ("split.rs", 900), ("gone.rs", 850)]);
        let report = check(&map(&[("big.rs", 950), ("split.rs", 300)]), &allow);
        assert!(report.errors.is_empty());
        assert_eq!(report.notes.len(), 3);
    }

    #[test]
    fn the_list_parses_counts_and_paths_and_refuses_the_rest() {
        let text = "# comment\n\n1042 xtask/src/main.rs\n900 a b.rs\n";
        assert_eq!(
            parse_allow(text),
            Ok(map(&[("xtask/src/main.rs", 1042), ("a b.rs", 900)]))
        );
        assert!(parse_allow("lots main.rs\n").is_err());
        assert!(parse_allow("main.rs\n").is_err());
        assert!(parse_allow("900 a.rs\n901 a.rs\n").is_err());
    }

    #[test]
    fn update_lists_only_files_past_the_limit_and_reparses() {
        let counts = map(&[("a.rs", 801), ("b.rs", 12), ("c.sh", 2000)]);
        let text = render_allow(&counts);
        assert_eq!(
            parse_allow(&text),
            Ok(map(&[("a.rs", 801), ("c.sh", 2000)]))
        );
        assert!(
            check(&counts, &parse_allow(&text).unwrap())
                .errors
                .is_empty()
        );
    }

    #[test]
    fn lines_count_a_last_line_without_a_newline() {
        assert_eq!(lines(b""), 0);
        assert_eq!(lines(b"a\nb\n"), 2);
        assert_eq!(lines(b"a\nb"), 2);
    }

    #[test]
    fn only_our_code_is_source() {
        assert!(is_source("kernel/src/main.rs"));
        assert!(is_source("scripts/batch/batch-ff.sh"));
        assert!(!is_source("Cargo.lock"));
        assert!(!is_source("third_party/xcc/src/x.c"));
        assert!(!is_source("docs/decisions.md"));
    }
}
