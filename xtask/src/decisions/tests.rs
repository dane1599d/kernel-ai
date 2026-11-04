//! Tests for `decisions`: the row format, the table, the check and the base comparison, on text, plus the tree itself.

use super::*;

const ROW: &str = "needed-by: P5, built 2026-11-04 (AP-1; K reviews)
decision: A decision in a sentence
---
The detail, first paragraph.

The second paragraph, with `code` and a \\| pipe.
";

const OTHER: &str = "needed-by: n\ndecision: d\n---\nx\n";

fn files(items: &[(&str, &str)]) -> Vec<(String, String)> {
    items
        .iter()
        .map(|(n, t)| (n.to_string(), t.to_string()))
        .collect()
}

const DECISIONS_MD: &str = "# Log\n\n## Settled\n\n| Area | Decision |\n|---|---|\n| a | b |\n\n## Open decisions, by the milestone\n\n| Needed by | Decision | Recommended default |\n|---|---|---|\n| M0 | x | y |\n| M1 | x | y |\n";

fn base(decisions: &str, deny_new_rows: bool) -> Option<Base<'_>> {
    Some(Base {
        rev: "origin/main",
        decisions,
        deny_new_rows,
    })
}

#[test]
fn a_row_file_is_three_parts_and_renders_as_one_table_line() {
    let row = parse_row("2026-11-04-a-decision.md", ROW).unwrap();
    assert_eq!(row.needed_by, "P5, built 2026-11-04 (AP-1; K reviews)");
    assert_eq!(row.decision, "A decision in a sentence");
    assert_eq!(
        row.detail,
        "The detail, first paragraph. The second paragraph, with `code` and a \\| pipe."
    );
    assert_eq!(
        row.line(),
        "| P5, built 2026-11-04 (AP-1; K reviews) | A decision in a sentence | The detail, first paragraph. The second paragraph, with `code` and a \\| pipe. |"
    );
}

#[test]
fn a_field_may_run_on_to_more_lines_in_either_order() {
    let text = "decision: A long\n  decision\nneeded-by: P5\n  built today\n---\nDetail\n";
    let row = parse_row("2026-11-04-x.md", text).unwrap();
    assert_eq!(row.decision, "A long decision");
    assert_eq!(row.needed_by, "P5 built today");
}

#[test]
fn a_file_that_is_not_a_row_says_why() {
    let cases: [(String, &str); 13] = [
        (String::from("decision: d\n---\nx\n"), "no `needed-by:`"),
        (String::from("needed-by: n\n---\nx\n"), "no `decision:`"),
        (String::from("needed-by: n\ndecision: d\n"), "no `---` line"),
        (
            String::from("needed-by: n\ndecision: d\n---\n\n"),
            "the detail is empty",
        ),
        (
            String::from("needed-by: n\nneeded-by: m\ndecision: d\n---\nx\n"),
            "twice",
        ),
        (
            String::from("needed-by: n\ndecision: d\ndefault: z\n---\nx\n"),
            "not a field",
        ),
        (
            String::from("needed-by: n\ndecision: d\n---\na | b\n"),
            "not escaped",
        ),
        (String::from("needed-by: n\ndecision: d\n---\nx"), "newline"),
        (
            String::from("needed-by: n\r\ndecision: d\r\n---\r\nx\r\n"),
            "carriage return",
        ),
        (
            String::from("junk\nneeded-by: n\ndecision: d\n---\nx\n"),
            "not a field",
        ),
        (TEMPLATE.to_string(), "still the template's"),
        (
            format!("needed-by: n\ndecision: d\n---\n{}\n", "x".repeat(MAX_ROW)),
            "KiB",
        ),
        (
            String::from("needed-by: n\ndecision: d\u{7}\n---\nx\n"),
            "control character",
        ),
    ];
    for (text, expected) in cases {
        let why = parse_row("2026-11-04-x.md", &text).unwrap_err();
        assert!(why.contains(expected), "{expected:?} is not in {why:?}");
    }
}

#[test]
fn a_row_file_is_named_by_its_date_and_a_plain_slug() {
    for good in [
        "2026-11-04-a.md",
        "2026-12-31-p5-task-10-host-half.md",
        "2027-01-01-x1.md",
    ] {
        assert_eq!(name_ok(good), Ok(()), "{good}");
    }
    for bad in [
        "a.md",
        "2026-11-04.md",
        "2026-11-04-.md",
        "2026-13-04-a.md",
        "2026-11-32-a.md",
        "2026-11-00-a.md",
        "2025-11-04-a.md",
        "2026-11-04-A.md",
        "2026-11-04-a_b.md",
        "2026-11-04-a--b.md",
        "2026-11-04--a.md",
        "2026-11-04-a-.md",
        "2026-11-04-a.txt",
        "2026-11-04-a b.md",
        "26-11-04-a.md",
    ] {
        assert!(name_ok(bad).is_err(), "{bad}");
    }
}

#[test]
fn the_rows_render_in_file_order_under_the_logs_own_header() {
    let rows = rows_of(&files(&[
        ("2026-11-05-b.md", "needed-by: n2\ndecision: d2\n---\nx2\n"),
        ("README.md", "# the rows\n"),
        ("2026-11-04-a.md", ROW),
    ]))
    .unwrap();
    assert_eq!(rows.len(), 2);
    let text = render(&rows);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(&lines[..2], [HEADER, RULE]);
    assert!(lines[2].starts_with("| P5, built 2026-11-04"));
    assert_eq!(lines[3], "| n2 | d2 | x2 |");
    assert_eq!(lines.len(), 4);
    assert!(text.ends_with('\n'));
    assert_eq!(render(&[]), format!("{HEADER}\n{RULE}\n"));
}

#[test]
fn the_table_rows_are_the_data_lines_of_the_open_decisions_table() {
    assert_eq!(table_rows(DECISIONS_MD), 2);
    assert_eq!(table_rows(""), 0);
    // A row split over lines counts once: its continuation does not start a table line.
    let split = format!("{DECISIONS_MD}| M2 | x | y\nstill row two |\n");
    assert_eq!(table_rows(&split), 3);
}

#[test]
fn good_files_pass_and_every_bad_one_is_reported_but_the_readme_is_not_a_row() {
    let good = files(&[
        ("README.md", "# rows\n"),
        ("2026-11-04-a.md", ROW),
        ("2026-11-05-b.md", OTHER),
    ]);
    assert_eq!(check_all(&good, DECISIONS_MD, None), Report::default());
    assert_eq!(check_all(&[], DECISIONS_MD, None), Report::default());
    let many = files(&[
        ("README.md", "# rows\n"),
        ("2026-11-04-a.md", ROW),
        ("2026-11-04-bad.md", "nothing\n"),
        ("notes.txt", "x"),
    ]);
    let report = check_all(&many, DECISIONS_MD, None);
    assert_eq!(report.errors.len(), 2, "{:?}", report.errors);
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("2026-11-04-bad.md"))
    );
    assert!(report.errors.iter().any(|e| e.contains("notes.txt")));
    assert!(report.warnings.is_empty());
}

#[test]
fn a_row_added_to_decisions_md_is_a_warning_and_with_deny_an_error() {
    let grown = format!("{DECISIONS_MD}| M9 | hand | added |\n");
    let report = check_all(&[], &grown, base(DECISIONS_MD, false));
    assert!(report.errors.is_empty());
    assert_eq!(report.warnings.len(), 1);
    assert!(
        report.warnings[0].contains("gains 1 row(s) over origin/main"),
        "{:?}",
        report.warnings
    );
    assert!(report.warnings[0].contains("cargo xtask decisions new"));
    let report = check_all(&[], &grown, base(DECISIONS_MD, true));
    assert!(report.warnings.is_empty());
    assert_eq!(report.errors.len(), 1);
    assert!(report.errors[0].contains("gains 1 row(s)"));
    let twice = format!("{grown}| M9 | hand | again |\n");
    let report = check_all(&[], &twice, base(DECISIONS_MD, true));
    assert!(report.errors[0].contains("gains 2 row(s)"));
}

#[test]
fn editing_or_removing_a_row_of_decisions_md_is_not_a_new_row() {
    let edited = DECISIONS_MD.replace("| M0 | x | y |", "| M0 | x | y, corrected |");
    assert_eq!(
        check_all(&[], &edited, base(DECISIONS_MD, true)),
        Report::default()
    );
    assert_eq!(
        check_all(&[], DECISIONS_MD, base(DECISIONS_MD, true)),
        Report::default()
    );
    let shorter = DECISIONS_MD.replace("| M1 | x | y |\n", "");
    assert_eq!(
        check_all(&[], &shorter, base(DECISIONS_MD, true)),
        Report::default()
    );
    // A row outside the open-decisions table is not counted either.
    let other = DECISIONS_MD.replace("| a | b |", "| a | b |\n| c | d |");
    assert_eq!(
        check_all(&[], &other, base(DECISIONS_MD, true)),
        Report::default()
    );
}

#[test]
fn a_bad_row_file_and_a_new_row_are_both_reported() {
    let grown = format!("{DECISIONS_MD}| M9 | hand | added |\n");
    let bad = files(&[("2026-11-04-bad.md", "nothing\n")]);
    let report = check_all(&bad, &grown, base(DECISIONS_MD, true));
    assert_eq!(report.errors.len(), 2, "{:?}", report.errors);
}

#[test]
fn check_takes_a_base_and_then_deny_new_rows_and_nothing_else() {
    let args = |items: &[&str]| items.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(check_options(&args(&[])), Ok((None, false)));
    assert_eq!(
        check_options(&args(&["--base", "origin/main"])),
        Ok((Some("origin/main"), false))
    );
    assert_eq!(
        check_options(&args(&["--base", "FETCH_HEAD", "--deny-new-rows"])),
        Ok((Some("FETCH_HEAD"), true))
    );
    for bad in [
        &["--deny-new-rows"][..],
        &["--base"],
        &["--exact"],
        &["--deny-new-rows", "--base", "x"],
        &["--base", "x", "--deny-new-rows", "y"],
    ] {
        assert_eq!(
            check_options(&args(bad)),
            Err(String::from(USAGE)),
            "{bad:?}"
        );
    }
}

/// A scratch directory for one test, empty.
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("kai-decisions-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_new_row_file_is_the_template_dated_today_and_never_overwrites() {
    let dir = scratch("new");
    let name = new_file(&dir, "my-slug", "2026-11-04").unwrap();
    assert_eq!(name, "2026-11-04-my-slug.md");
    let made = fs::read_to_string(dir.join(ROWS_DIR).join(&name)).unwrap();
    assert_eq!(made, TEMPLATE);
    assert!(
        new_file(&dir, "my-slug", "2026-11-04")
            .unwrap_err()
            .contains("exists")
    );
    assert!(new_file(&dir, "Bad Slug", "2026-11-04").is_err());
    // The template itself is refused until it is filled in.
    assert!(parse_row(&name, &made).is_err());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn today_is_a_date() {
    let date = today().unwrap();
    assert!(name_ok(&format!("{date}-x.md")).is_ok(), "{date}");
}

#[test]
fn decisions_md_is_read_as_a_revision_has_it() {
    let dir = scratch("git");
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t.invalid"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    };
    git(&["init", "-q"]);
    fs::create_dir_all(dir.join("docs")).unwrap();
    fs::write(dir.join(DECISIONS), DECISIONS_MD).unwrap();
    git(&["add", DECISIONS]);
    git(&["commit", "-q", "-m", "first"]);
    // The working tree moves on; the revision does not.
    fs::write(dir.join(DECISIONS), "changed\n").unwrap();
    assert_eq!(decisions_at(&dir, "HEAD").unwrap(), DECISIONS_MD);
    assert!(decisions_at(&dir, "no-such-revision").is_err());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_rows_in_the_tree_pass_the_check() {
    let root = crate::workspace_root();
    let files = read_files(&root).unwrap();
    let decisions = fs::read_to_string(root.join(DECISIONS)).unwrap();
    let report = check_all(&files, &decisions, None);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(!rows_of(&files).unwrap().is_empty());
}
