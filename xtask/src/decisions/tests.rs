//! Tests for `decisions`: the row format, the rendering and the check, on text, plus the tree itself.

use super::*;

const ROW: &str = "needed-by: P5, built 2026-11-04 (AP-1; K reviews)
decision: A decision in a sentence
---
The detail, first paragraph.

The second paragraph, with `code` and a \\| pipe.
";

fn files(items: &[(&str, &str)]) -> Vec<(String, String)> {
    items
        .iter()
        .map(|(n, t)| (n.to_string(), t.to_string()))
        .collect()
}

fn legacy(rows: usize, strict: bool) -> Result<Legacy, String> {
    Ok(Legacy { rows, strict })
}

const DECISIONS_MD: &str = "# Log\n\n## Settled\n\n| Area | Decision |\n|---|---|\n| a | b |\n\n## Open decisions, by the milestone\n\n| Needed by | Decision | Recommended default |\n|---|---|---|\n| M0 | x | y |\n| M1 | x | y |\n";

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
    let cases: [(&str, String, &str); 13] = [
        (
            "2026-11-04-x.md",
            String::from("decision: d\n---\nx\n"),
            "no `needed-by:`",
        ),
        (
            "2026-11-04-x.md",
            String::from("needed-by: n\n---\nx\n"),
            "no `decision:`",
        ),
        (
            "2026-11-04-x.md",
            String::from("needed-by: n\ndecision: d\n"),
            "no `---` line",
        ),
        (
            "2026-11-04-x.md",
            String::from("needed-by: n\ndecision: d\n---\n\n"),
            "the detail is empty",
        ),
        (
            "2026-11-04-x.md",
            String::from("needed-by: n\nneeded-by: m\ndecision: d\n---\nx\n"),
            "twice",
        ),
        (
            "2026-11-04-x.md",
            String::from("needed-by: n\ndecision: d\ndefault: z\n---\nx\n"),
            "not a field",
        ),
        (
            "2026-11-04-x.md",
            String::from("needed-by: n\ndecision: d\n---\na | b\n"),
            "not escaped",
        ),
        (
            "2026-11-04-x.md",
            String::from("needed-by: n\ndecision: d\n---\nx"),
            "newline",
        ),
        (
            "2026-11-04-x.md",
            String::from("needed-by: n\r\ndecision: d\r\n---\r\nx\r\n"),
            "carriage return",
        ),
        (
            "2026-11-04-x.md",
            String::from("junk\nneeded-by: n\ndecision: d\n---\nx\n"),
            "not a field",
        ),
        (
            "2026-11-04-x.md",
            TEMPLATE.to_string(),
            "still the template's",
        ),
        (
            "2026-11-04-x.md",
            format!("needed-by: n\ndecision: d\n---\n{}\n", "x".repeat(MAX_ROW)),
            "KiB",
        ),
        (
            "2026-11-04-x.md",
            String::from("needed-by: n\ndecision: d\u{7}\n---\nx\n"),
            "control character",
        ),
    ];
    for (name, text, expected) in cases {
        let why = parse_row(name, &text).unwrap_err();
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
fn the_rows_render_in_file_order_under_one_header() {
    let later = "needed-by: n2\ndecision: d2\n---\nx2\n";
    let rows = rows_of(&files(&[
        ("2026-11-05-b.md", later),
        ("README.md", "# the rows\n"),
        ("2026-11-04-a.md", ROW),
    ]))
    .unwrap();
    assert_eq!(rows.len(), 2);
    let text = render(&rows);
    let lines: Vec<&str> = text.lines().collect();
    let at = lines.iter().position(|l| *l == RULE).unwrap();
    assert_eq!(lines[at - 1], HEADER);
    assert!(lines[at + 1].starts_with("| P5, built 2026-11-04"));
    assert_eq!(lines[at + 2], "| n2 | d2 | x2 |");
    assert!(text.ends_with('\n'));
}

#[test]
fn the_legacy_rows_are_the_data_lines_of_the_open_decisions_table() {
    assert_eq!(legacy_rows(DECISIONS_MD), 2);
    assert_eq!(legacy_rows(""), 0);
    // A row split over lines counts once: its continuation does not start a table line.
    let split = format!("{DECISIONS_MD}| M2 | x | y\nstill row two |\n");
    assert_eq!(legacy_rows(&split), 3);
}

#[test]
fn the_legacy_settings_are_two_lines() {
    assert_eq!(
        parse_legacy("# note\nrows 371\nmode transition\n"),
        Ok(Legacy {
            rows: 371,
            strict: false
        })
    );
    assert_eq!(
        parse_legacy("rows 5\nmode strict\n"),
        Ok(Legacy {
            rows: 5,
            strict: true
        })
    );
    for bad in [
        "",
        "rows 5\n",
        "mode strict\n",
        "rows x\nmode strict\n",
        "rows 5\nmode loose\n",
        "rows 5\nmode strict\nextra 1\n",
    ] {
        assert!(parse_legacy(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn good_files_and_a_stale_rendering_pass_and_a_new_hand_added_row_warns_then_fails() {
    let two = files(&[
        ("2026-11-04-a.md", ROW),
        ("2026-11-05-b.md", "needed-by: n\ndecision: d\n---\nx\n"),
    ]);
    // No ROWS.md yet, or one that is behind the files: fine.
    let report = check_all(&two, None, DECISIONS_MD, legacy(2, false), false);
    assert_eq!(report, Report::default());
    let rows = rows_of(&two[..1]).unwrap();
    let stale = render(&rows);
    let report = check_all(&two, Some(&stale), DECISIONS_MD, legacy(2, true), false);
    assert_eq!(report, Report::default());
    // A row added to decisions.md by hand: a warning in transition, an error after.
    let grown = format!("{DECISIONS_MD}| M9 | hand | added |\n");
    let report = check_all(&two, Some(&stale), &grown, legacy(2, false), false);
    assert!(report.errors.is_empty());
    assert!(report.warnings[0].contains("3 rows and 2 are allowed"));
    let report = check_all(&two, Some(&stale), &grown, legacy(2, true), false);
    assert_eq!(report.warnings.len(), 0);
    assert!(report.errors[0].contains("a new row is a file"));
    // Editing a row that is already there changes no count.
    let edited = DECISIONS_MD.replace("| M0 | x | y |", "| M0 | x | y, corrected |");
    assert_eq!(
        check_all(&two, None, &edited, legacy(2, true), false),
        Report::default()
    );
}

#[test]
fn a_rendered_file_with_a_row_no_file_has_is_refused() {
    let one = files(&[("2026-11-04-a.md", ROW)]);
    let rows = rows_of(&one).unwrap();
    let mut shown = render(&rows);
    shown.push_str("| added | by | hand |\n");
    let report = check_all(&one, Some(&shown), DECISIONS_MD, legacy(2, true), false);
    assert!(report.errors[0].contains("no file"), "{:?}", report.errors);
    // A row of a file that was edited since is the same: it matches no file.
    let edited = render(&rows).replace("A decision in a sentence", "Another sentence");
    let report = check_all(&one, Some(&edited), DECISIONS_MD, legacy(2, true), false);
    assert_eq!(report.errors.len(), 1);
    // A file with no table at all.
    let report = check_all(
        &one,
        Some("# nothing\n"),
        DECISIONS_MD,
        legacy(2, true),
        false,
    );
    assert!(report.errors[0].contains("no table"));
}

#[test]
fn exact_wants_the_rendering_to_be_the_files() {
    let two = files(&[
        ("2026-11-04-a.md", ROW),
        ("2026-11-05-b.md", "needed-by: n\ndecision: d\n---\nx\n"),
    ]);
    let rows = rows_of(&two).unwrap();
    let full = render(&rows);
    assert_eq!(
        check_all(&two, Some(&full), DECISIONS_MD, legacy(2, true), true),
        Report::default()
    );
    let stale = render(&rows[..1]);
    let report = check_all(&two, Some(&stale), DECISIONS_MD, legacy(2, true), true);
    assert!(
        report.errors[0].contains("decisions write"),
        "{:?}",
        report.errors
    );
    let report = check_all(&two, None, DECISIONS_MD, legacy(2, true), true);
    assert!(report.errors[0].contains("is missing"));
    // Lint mode never asks for it.
    assert_eq!(
        check_all(&two, Some(&stale), DECISIONS_MD, legacy(2, true), false),
        Report::default()
    );
}

#[test]
fn every_bad_file_in_a_directory_is_reported_and_the_readme_is_not_a_row() {
    let many = files(&[
        ("README.md", "# rows\n"),
        ("2026-11-04-a.md", ROW),
        ("2026-11-04-bad.md", "nothing\n"),
        ("notes.txt", "x"),
    ]);
    let report = check_all(&many, None, DECISIONS_MD, legacy(2, true), false);
    assert_eq!(report.errors.len(), 2, "{:?}", report.errors);
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("2026-11-04-bad.md"))
    );
    assert!(report.errors.iter().any(|e| e.contains("notes.txt")));
}

#[test]
fn a_new_row_file_is_the_template_dated_today_and_never_overwrites() {
    let dir = std::env::temp_dir().join(format!("kai-decisions-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
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
fn the_rows_in_the_tree_pass_the_check_exactly_when_rendered() {
    let root = crate::workspace_root();
    let files = read_files(&root).unwrap();
    let decisions = fs::read_to_string(root.join(DECISIONS)).unwrap();
    let legacy = parse_legacy(&fs::read_to_string(root.join(LEGACY_FILE)).unwrap());
    let shown = fs::read_to_string(root.join(ROWS_FILE)).ok();
    let report = check_all(&files, shown.as_deref(), &decisions, legacy, false);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
}
