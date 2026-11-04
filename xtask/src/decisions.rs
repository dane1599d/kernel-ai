//! `cargo xtask decisions`: the decision log's new rows, one file each.
//!
//! `docs/decisions.md` is one table that every pull request appends to, so GitHub
//! finds nearly every open pull request in conflict with main (it ignores the
//! `merge=union` attribute that makes the batcher's own merges clean), runs no CI
//! on a conflicting one, and the lanes keep merging main in. A new row is now a
//! file of its own, `docs/decisions/rows/<date>-<slug>.md`, so two pull requests
//! never touch the same file; the existing rows stay where they are.
//!
//! ```text
//! needed-by: P5, built 2026-11-04 (AP-1; K reviews)
//! decision: What was decided, in a sentence
//! ---
//! The detail: what was decided and why. Paragraphs are joined into one table
//! cell. Write `\|` for a literal pipe.
//! ```
//!
//! - `check` (the default; CI runs it) refuses a malformed row file, a rendered
//!   `docs/decisions/ROWS.md` that holds a row no file has, and, once the rule is
//!   `strict` (`docs/decisions/LEGACY`), a `decisions.md` that has grown past the
//!   rows it had. Until then the last is a warning, so the pull requests already
//!   open and their union merges are unaffected. `check --exact` also requires
//!   `ROWS.md` to be exactly the render of the files: the batch that merges runs it.
//! - `render` prints `ROWS.md`; `write` writes it. A pull request does not: the
//!   file is the batch's to keep, or nothing would stop two pull requests that
//!   each add a row from conflicting on it.
//! - `new <slug>` makes a row file from the template, dated today.

use kaipkg::generation::Time;
use std::fs;
use std::path::Path;

/// The directory of row files.
pub const ROWS_DIR: &str = "docs/decisions/rows";
/// The rendered table of every row file, kept by the batch that merges them.
pub const ROWS_FILE: &str = "docs/decisions/ROWS.md";
/// The rule for `decisions.md`: how many rows it had, and whether more are refused.
pub const LEGACY_FILE: &str = "docs/decisions/LEGACY";
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
const LEGACY_HEADING: &str = "## Open decisions";

const TEMPLATE: &str = "needed-by: <the phase or task, built YYYY-MM-DD (lane; who reviews)>
decision: <what was decided, in a sentence>
---
<The detail: what was decided and why. Paragraphs are joined into one table cell. Write \\| for a literal pipe.>
";

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

/// `ROWS.md` for `rows`, which are in file order.
pub fn render(rows: &[Row]) -> String {
    let mut out = String::from(
        "# Kernel AI: decision rows, one file each\n\n\
<!-- Generated by `cargo xtask decisions write` from docs/decisions/rows/*.md. Do not edit it, and a pull request does not write it: the batch that merges the rows does. -->\n\n\
Decisions made since each was its own file in `docs/decisions/rows/` (see its README). Earlier ones are rows of the table in [decisions.md](../decisions.md).\n\n",
    );
    out.push_str(HEADER);
    out.push('\n');
    out.push_str(RULE);
    out.push('\n');
    for row in rows {
        out.push_str(&row.line());
        out.push('\n');
    }
    out
}

/// The data rows of `decisions.md`'s open-decisions table: lines that start with a
/// `|`, other than the table's header and rule.
pub fn legacy_rows(decisions: &str) -> usize {
    let mut inside = false;
    let mut count = 0;
    for line in decisions.lines() {
        if line.starts_with("## ") {
            inside = line.starts_with(LEGACY_HEADING);
        } else if inside && line.starts_with('|') && line != HEADER && line != RULE {
            count += 1;
        }
    }
    count
}

/// `docs/decisions/LEGACY`: the rows `decisions.md` had when new rows became
/// files, and whether more are refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Legacy {
    pub rows: usize,
    pub strict: bool,
}

pub fn parse_legacy(text: &str) -> Result<Legacy, String> {
    let mut rows = None;
    let mut strict = None;
    for line in text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
    {
        match line.split_once(' ') {
            Some(("rows", n)) => {
                rows = Some(
                    n.trim()
                        .parse::<usize>()
                        .map_err(|_| format!("{LEGACY_FILE}: `rows` is a number"))?,
                )
            }
            Some(("mode", "transition")) => strict = Some(false),
            Some(("mode", "strict")) => strict = Some(true),
            _ => {
                return Err(format!(
                    "{LEGACY_FILE}: `{line}` is not `rows N` or `mode transition|strict`"
                ));
            }
        }
    }
    Ok(Legacy {
        rows: rows.ok_or_else(|| format!("{LEGACY_FILE}: no `rows N`"))?,
        strict: strict.ok_or_else(|| format!("{LEGACY_FILE}: no `mode`"))?,
    })
}

/// What `check` found.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

/// The whole check over what is on disk, as text: `files` are the row directory's
/// `(name, text)`, `rows_md` the rendered file if there is one.
pub fn check_all(
    files: &[(String, String)],
    rows_md: Option<&str>,
    decisions: &str,
    legacy: Result<Legacy, String>,
    exact: bool,
) -> Report {
    let mut report = Report::default();
    let mut rows = Vec::new();
    let mut sorted: Vec<&(String, String)> = files.iter().filter(|(n, _)| n != README).collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, text) in sorted {
        match parse_row(name, text) {
            Ok(row) => rows.push(row),
            Err(why) => report.errors.push(why),
        }
    }
    if let Some(shown) = rows_md {
        let lines: Vec<String> = rows.iter().map(Row::line).collect();
        let first = shown.lines().position(|l| l == RULE);
        for line in shown
            .lines()
            .skip(first.map_or(usize::MAX, |at| at + 1))
            .filter(|l| l.starts_with('|'))
        {
            if !lines.iter().any(|row| row == line) {
                report.errors.push(format!(
                    "{ROWS_FILE} has a row no file in {ROWS_DIR} has, or one that was edited by hand: {}",
                    line.chars().take(80).collect::<String>()
                ));
            }
        }
        if first.is_none() {
            report.errors.push(format!("{ROWS_FILE} has no table"));
        }
        if exact && report.errors.is_empty() && shown != render(&rows) {
            report.errors.push(format!(
                "{ROWS_FILE} is not the render of {ROWS_DIR}: run `cargo xtask decisions write`"
            ));
        }
    } else if exact {
        report.errors.push(format!(
            "{ROWS_FILE} is missing: run `cargo xtask decisions write`"
        ));
    }
    match legacy {
        Ok(legacy) => {
            let have = legacy_rows(decisions);
            if have > legacy.rows {
                let message = format!(
                    "{DECISIONS} has {have} rows and {} are allowed: a new row is a file in {ROWS_DIR} (cargo xtask decisions new <slug>)",
                    legacy.rows
                );
                match legacy.strict {
                    true => report.errors.push(message),
                    false => report.warnings.push(message),
                }
            }
        }
        Err(why) => report.errors.push(why),
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
    let mut sorted: Vec<&(String, String)> = files.iter().filter(|(n, _)| n != README).collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    sorted
        .into_iter()
        .map(|(name, text)| parse_row(name, text))
        .collect()
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

pub fn main(args: &[String]) -> Result<(), String> {
    let root = crate::workspace_root();
    match args {
        [] => run_check(&root, false),
        [word] if word == "check" => run_check(&root, false),
        [word, flag] if word == "check" && flag == "--exact" => run_check(&root, true),
        [word] if word == "render" => {
            print!("{}", render(&rows_of(&read_files(&root)?)?));
            Ok(())
        }
        [word] if word == "write" => {
            let text = render(&rows_of(&read_files(&root)?)?);
            fs::write(root.join(ROWS_FILE), &text).map_err(|e| format!("{ROWS_FILE}: {e}"))?;
            println!("xtask: wrote {ROWS_FILE}");
            Ok(())
        }
        [word, slug] if word == "new" => {
            let name = new_file(&root, slug, &today()?)?;
            println!("xtask: wrote {ROWS_DIR}/{name}: fill in its three parts");
            Ok(())
        }
        _ => Err(String::from(
            "usage: cargo xtask decisions [check [--exact] | render | write | new <slug>]",
        )),
    }
}

fn run_check(root: &Path, exact: bool) -> Result<(), String> {
    let files = read_files(root)?;
    let rows_md = fs::read_to_string(root.join(ROWS_FILE)).ok();
    let decisions =
        fs::read_to_string(root.join(DECISIONS)).map_err(|e| format!("{DECISIONS}: {e}"))?;
    let legacy = fs::read_to_string(root.join(LEGACY_FILE))
        .map_err(|e| format!("{LEGACY_FILE}: {e}"))
        .and_then(|text| parse_legacy(&text));
    let report = check_all(&files, rows_md.as_deref(), &decisions, legacy, exact);
    for warning in &report.warnings {
        println!("xtask: warning: {warning}");
    }
    match report.errors.is_empty() {
        true => {
            println!(
                "xtask: decisions: {} rows in {ROWS_DIR} are well formed",
                files.iter().filter(|(n, _)| n != README).count()
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
