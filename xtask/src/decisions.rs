//! `cargo xtask decisions`: the decision log's new rows, one file each.
//!
//! `docs/decisions.md` is one table that every pull request appends to, so GitHub
//! finds nearly every open pull request in conflict with main (it ignores the
//! `merge=union` attribute that makes the batcher's own merges clean), runs no CI
//! on a conflicting one, and the lanes keep merging main in. A new row is now a
//! file of its own, `docs/decisions/rows/<date>-<slug>.md`, so two pull requests
//! never touch the same file; the existing rows stay where they are. Nothing
//! generated is committed: the combined table is printed on demand, so there is no
//! shared file for two pull requests to conflict on.
//!
//! ```text
//! needed-by: P5, built 2026-11-04 (AP-1; K reviews)
//! decision: What was decided, in a sentence
//! ---
//! The detail: what was decided and why. Paragraphs are joined into one table
//! cell. Write `\|` for a literal pipe.
//! ```
//!
//! - no argument, or `render`, prints the table of the row files, in file order.
//! - `check` refuses a malformed row file (CI runs it). With `--base REV` it also
//!   compares the rows of `decisions.md` with those of `REV`'s: a change that adds
//!   one is a warning, or an error with `--deny-new-rows`, which CI passes once the
//!   pull requests that still append a row have landed. It holds no count, so
//!   nothing in it goes stale as rows land.
//! - `new <slug>` makes a row file from the template, dated today.

use kaipkg::generation::Time;
use std::fs;
use std::path::Path;
use std::process::Command;

/// The directory of row files.
pub const ROWS_DIR: &str = "docs/decisions/rows";
/// The log whose table the rows came out of.
pub const DECISIONS: &str = "docs/decisions.md";

/// A row file's text may be no longer than this, as one table line.
const MAX_ROW: usize = 16 * 1024;
/// The README that sits among the row files.
const README: &str = "README.md";
/// The table's three columns.
const HEADER: &str = "| Needed by | Decision | Recommended default |";
const RULE: &str = "|---|---|---|";
/// The heading of the `decisions.md` table that rows came out of.
const TABLE_HEADING: &str = "## Open decisions";

const TEMPLATE: &str = "needed-by: <the phase or task, built YYYY-MM-DD (lane; who reviews)>
decision: <what was decided, in a sentence>
---
<The detail: what was decided and why. Paragraphs are joined into one table cell. Write \\| for a literal pipe.>
";

const USAGE: &str =
    "usage: cargo xtask decisions [render | check [--base REV [--deny-new-rows]] | new <slug>]";

/// One row, from its file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// The file's name, which is the row's place in the order.
    pub file: String,
    pub needed_by: String,
    pub decision: String,
    pub detail: String,
}

impl Row {
    /// The row as a table line.
    pub fn line(&self) -> String {
        format!(
            "| {} | {} | {} |",
            self.needed_by, self.decision, self.detail
        )
    }
}

/// Whether `name` is `YYYY-MM-DD-slug.md`: a real-looking date and a slug of
/// lower-case letters, digits and hyphens that neither starts nor ends with one.
fn name_ok(name: &str) -> Result<(), String> {
    let stem = name
        .strip_suffix(".md")
        .ok_or_else(|| format!("{name}: a row file ends in .md"))?;
    let bad = || {
        format!("{name}: a row file is named YYYY-MM-DD-slug.md, the slug of a-z, 0-9 and hyphens")
    };
    let (date, slug) = stem.split_at_checked(10).ok_or_else(bad)?;
    let slug = slug.strip_prefix('-').ok_or_else(bad)?;
    let parts: Vec<&str> = date.split('-').collect();
    let number = |part: &str, len: usize| {
        (part.len() == len && part.bytes().all(|b| b.is_ascii_digit()))
            .then(|| part.parse::<u32>().ok())
            .flatten()
    };
    let ok = matches!(
        parts.as_slice(),
        [y, m, d] if number(y, 4).is_some_and(|y| y >= 2026)
            && number(m, 2).is_some_and(|m| (1..=12).contains(&m))
            && number(d, 2).is_some_and(|d| (1..=31).contains(&d))
    );
    let slug_ok = !slug.is_empty()
        && slug.len() <= 48
        && !slug.starts_with('-')
        && !slug.ends_with('-')
        && !slug.contains("--")
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    match ok && slug_ok {
        true => Ok(()),
        false => Err(bad()),
    }
}

/// A table cell: one paragraph, no padding, no control character, and every pipe
/// escaped, since an unescaped one would end the cell.
fn cell_ok(what: &str, text: &str) -> Result<(), String> {
    if text.is_empty() {
        return Err(format!("{what} is empty"));
    }
    if text.starts_with('<') && text.contains('>') && text.len() < 200 {
        return Err(format!("{what} is still the template's"));
    }
    if text.chars().any(|c| c.is_control() && c != '\t') {
        return Err(format!("{what} has a control character"));
    }
    let mut escaped = false;
    for c in text.chars() {
        match (escaped, c) {
            (false, '\\') => escaped = true,
            (false, '|') => {
                return Err(format!("{what} has a | that is not escaped: write \\|"));
            }
            _ => escaped = false,
        }
    }
    Ok(())
}

/// Lines folded into one cell: trimmed, empty ones dropped, joined by a space.
fn fold(lines: &[&str]) -> String {
    let mut out = String::new();
    for line in lines.iter().map(|l| l.trim()).filter(|l| !l.is_empty()) {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(line);
    }
    out
}

/// The row a file holds, or why it is not one.
pub fn parse_row(file: &str, text: &str) -> Result<Row, String> {
    let fail = |why: String| format!("{file}: {why}");
    name_ok(file)?;
    if text.contains('\r') {
        return Err(fail(String::from(
            "it has a carriage return: save it with LF",
        )));
    }
    if !text.ends_with('\n') {
        return Err(fail(String::from("it does not end with a newline")));
    }
    if text.len() > MAX_ROW {
        return Err(fail(format!("it is over {} KiB", MAX_ROW / 1024)));
    }
    let (head, detail) = text.split_once("\n---\n").ok_or_else(|| {
        fail(String::from(
            "it has no `---` line between its fields and its detail",
        ))
    })?;
    let mut needed_by: Option<Vec<&str>> = None;
    let mut decision: Option<Vec<&str>> = None;
    let mut current: Option<bool> = None;
    for line in head.lines() {
        if let Some(rest) = line.strip_prefix("needed-by:") {
            if needed_by.replace(vec![rest]).is_some() {
                return Err(fail(String::from("`needed-by:` twice")));
            }
            current = Some(true);
        } else if let Some(rest) = line.strip_prefix("decision:") {
            if decision.replace(vec![rest]).is_some() {
                return Err(fail(String::from("`decision:` twice")));
            }
            current = Some(false);
        } else if is_field(line) {
            return Err(fail(format!(
                "`{}` is not a field: a row has `needed-by:` and `decision:`",
                line.split(':').next().unwrap_or("")
            )));
        } else {
            match (current, line.trim().is_empty()) {
                (_, true) => {}
                (Some(true), false) => needed_by.as_mut().map(|v| v.push(line)).unwrap_or(()),
                (Some(false), false) => decision.as_mut().map(|v| v.push(line)).unwrap_or(()),
                (None, false) => {
                    return Err(fail(String::from(
                        "the file starts with something that is not a field",
                    )));
                }
            }
        }
    }
    let needed_by = fold(&needed_by.ok_or_else(|| fail(String::from("no `needed-by:`")))?);
    let decision = fold(&decision.ok_or_else(|| fail(String::from("no `decision:`")))?);
    let detail = fold(&detail.lines().collect::<Vec<_>>());
    cell_ok("needed-by", &needed_by).map_err(fail)?;
    cell_ok("decision", &decision).map_err(fail)?;
    cell_ok("the detail", &detail).map_err(fail)?;
    Ok(Row {
        file: file.to_string(),
        needed_by,
        decision,
        detail,
    })
}

/// A line like `word: text` with a lower-case, hyphenated word: a field name.
fn is_field(line: &str) -> bool {
    line.split_once(": ").is_some_and(|(word, _)| {
        !word.is_empty()
            && word.len() <= 16
            && word.bytes().all(|b| b.is_ascii_lowercase() || b == b'-')
    })
}

/// The table of `rows`, which are in file order, under the log's own header.
pub fn render(rows: &[Row]) -> String {
    let mut out = format!("{HEADER}\n{RULE}\n");
    for row in rows {
        out.push_str(&row.line());
        out.push('\n');
    }
    out
}

/// The data rows of `decisions.md`'s open-decisions table: lines that start with a
/// `|`, other than the table's header and rule.
pub fn table_rows(decisions: &str) -> usize {
    let mut inside = false;
    let mut count = 0;
    for line in decisions.lines() {
        if line.starts_with("## ") {
            inside = line.starts_with(TABLE_HEADING);
        } else if inside && line.starts_with('|') && line != HEADER && line != RULE {
            count += 1;
        }
    }
    count
}

/// The row files (not the README) in name order.
fn sorted(files: &[(String, String)]) -> Vec<&(String, String)> {
    let mut out: Vec<&(String, String)> = files.iter().filter(|(n, _)| n != README).collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// What `check` found.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

/// `decisions.md` as `REV` has it, and whether a row added to it is an error.
pub struct Base<'a> {
    pub rev: &'a str,
    pub decisions: &'a str,
    pub deny_new_rows: bool,
}

/// The whole check over what is on disk, as text: `files` are the row directory's
/// `(name, text)`, `decisions` is `decisions.md`.
pub fn check_all(files: &[(String, String)], decisions: &str, base: Option<Base>) -> Report {
    let mut report = Report::default();
    for (name, text) in sorted(files) {
        if let Err(why) = parse_row(name, text) {
            report.errors.push(why);
        }
    }
    let Some(base) = base else {
        return report;
    };
    let (have, had) = (table_rows(decisions), table_rows(base.decisions));
    if have > had {
        let message = format!(
            "{DECISIONS} gains {} row(s) over {}: a new row is a file in {ROWS_DIR} (cargo xtask decisions new <slug>)",
            have - had,
            base.rev
        );
        match base.deny_new_rows {
            true => report.errors.push(message),
            false => report.warnings.push(message),
        }
    }
    report
}

/// The row files under `root`, as `(name, text)`.
fn read_files(root: &Path) -> Result<Vec<(String, String)>, String> {
    let dir = root.join(ROWS_DIR);
    let Ok(entries) = fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("{ROWS_DIR}: {e}"))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !entry.path().is_file() {
            return Err(format!("{ROWS_DIR}/{name} is not a file"));
        }
        let text =
            fs::read_to_string(entry.path()).map_err(|e| format!("{ROWS_DIR}/{name}: {e}"))?;
        out.push((name, text));
    }
    Ok(out)
}

fn rows_of(files: &[(String, String)]) -> Result<Vec<Row>, String> {
    sorted(files)
        .into_iter()
        .map(|(name, text)| parse_row(name, text))
        .collect()
}

/// `decisions.md` as `rev` has it, from git.
fn decisions_at(root: &Path, rev: &str) -> Result<String, String> {
    let spec = format!("{rev}:{DECISIONS}");
    let out = Command::new("git")
        .arg("show")
        .arg(&spec)
        .current_dir(root)
        .output()
        .map_err(|e| format!("git show {spec}: {e}"))?;
    match out.status.success() {
        true => String::from_utf8(out.stdout).map_err(|_| format!("{spec} is not UTF-8")),
        false => Err(format!(
            "git show {spec}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )),
    }
}

/// Today's date, `YYYY-MM-DD`, from the clock.
fn today() -> Result<String, String> {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("the clock is before 1970: {e}"))?
        .as_secs();
    Ok(Time(secs).format().chars().take(10).collect())
}

/// A slug is `a-z`, `0-9` and hyphens, as a row file's name needs.
fn new_file(root: &Path, slug: &str, date: &str) -> Result<String, String> {
    let name = format!("{date}-{slug}.md");
    name_ok(&name)?;
    let path = root.join(ROWS_DIR).join(&name);
    if path.exists() {
        return Err(format!("{ROWS_DIR}/{name} exists already"));
    }
    fs::create_dir_all(root.join(ROWS_DIR)).map_err(|e| format!("{ROWS_DIR}: {e}"))?;
    fs::write(&path, TEMPLATE).map_err(|e| format!("{ROWS_DIR}/{name}: {e}"))?;
    Ok(name)
}

/// The options of `check`: `--base REV` and, after it, `--deny-new-rows`.
fn check_options(args: &[String]) -> Result<(Option<&str>, bool), String> {
    match args {
        [] => Ok((None, false)),
        [flag, rev] if flag == "--base" => Ok((Some(rev), false)),
        [flag, rev, deny] if flag == "--base" && deny == "--deny-new-rows" => Ok((Some(rev), true)),
        _ => Err(String::from(USAGE)),
    }
}

pub fn main(args: &[String]) -> Result<(), String> {
    let root = crate::workspace_root();
    match args {
        [] => render_rows(&root),
        [word] if word == "render" => render_rows(&root),
        [word, options @ ..] if word == "check" => {
            let (rev, deny_new_rows) = check_options(options)?;
            run_check(&root, rev, deny_new_rows)
        }
        [word, slug] if word == "new" => {
            let name = new_file(&root, slug, &today()?)?;
            println!("xtask: wrote {ROWS_DIR}/{name}: fill in its three parts");
            Ok(())
        }
        _ => Err(String::from(USAGE)),
    }
}

fn render_rows(root: &Path) -> Result<(), String> {
    print!("{}", render(&rows_of(&read_files(root)?)?));
    Ok(())
}

fn run_check(root: &Path, rev: Option<&str>, deny_new_rows: bool) -> Result<(), String> {
    let files = read_files(root)?;
    let decisions =
        fs::read_to_string(root.join(DECISIONS)).map_err(|e| format!("{DECISIONS}: {e}"))?;
    let at_base = rev.map(|rev| decisions_at(root, rev)).transpose()?;
    let base = rev.zip(at_base.as_deref()).map(|(rev, decisions)| Base {
        rev,
        decisions,
        deny_new_rows,
    });
    let report = check_all(&files, &decisions, base);
    for warning in &report.warnings {
        println!("xtask: warning: {warning}");
    }
    match report.errors.is_empty() {
        true => {
            println!(
                "xtask: decisions: {} rows in {ROWS_DIR} are well formed",
                sorted(&files).len()
            );
            Ok(())
        }
        false => {
            for error in &report.errors {
                println!("xtask: {error}");
            }
            Err(format!(
                "{} problem(s) with the decision rows",
                report.errors.len()
            ))
        }
    }
}

#[cfg(test)]
mod tests;
