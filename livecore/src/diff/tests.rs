use super::*;
use crate::artifact::bundle;
use alloc::string::ToString;

const PATH: &str = "components/notes/src/lib.rs";

fn numbered(count: usize, ending: &str) -> String {
    (1..=count).map(|n| format!("line {n}{ending}")).collect()
}

fn one(text: &str) -> Vec<u8> {
    bundle(&[(PATH, text.as_bytes())])
}

#[test]
fn a_one_line_change_shows_that_line_with_two_lines_of_context() {
    let old = numbered(200, "\n");
    let new = old.replace("line 141\n", "line 141, edited live\n");
    let lines = view(&one(&old), &one(&new), 80, 40).unwrap();
    assert_eq!(
        lines,
        [
            format!("{PATH} line 141: 1 removed, 1 added"),
            "  | line 139".to_string(),
            "  | line 140".to_string(),
            "- | line 141".to_string(),
            "+ | line 141, edited live".to_string(),
            "  | line 142".to_string(),
            "  | line 143".to_string(),
        ]
    );
}

#[test]
fn two_separate_edits_show_as_one_region() {
    let old = numbered(20, "\n");
    let new = old
        .replace("line 3\n", "line three\n")
        .replace("line 6\n", "line six\n");
    let (old, new) = (one(&old), one(&new));
    let changes = changes(&old, &new).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!((changes[0].line, changes[0].removed.len()), (3, 4));
    assert_eq!(changes[0].added[3], b"line six\n");
}

#[test]
fn crlf_files_compare_byte_for_byte() {
    let old = numbered(10, "\r\n");
    let new = old.replace("line 4\r\n", "line four\r\n");
    let (old_bundle, new_bundle) = (one(&old), one(&new));
    let change = &changes(&old_bundle, &new_bundle).unwrap()[0];
    assert_eq!((change.line, change.removed.len()), (4, 1));
    // Changing only an ending shows that one line, and says so.
    let new = old.replace("line 7\r\n", "line 7\n");
    let lines = view(&one(&old), &one(&new), 80, 40).unwrap();
    assert_eq!(
        lines[0],
        format!("{PATH} line 7: 1 removed, 1 added (line endings only)")
    );
    // So does a missing final newline.
    let old = "a\nb\n";
    let lines = view(&one(old), &one("a\nb"), 80, 40).unwrap();
    assert_eq!(
        lines[0],
        format!("{PATH} line 2: 1 removed, 1 added (line endings only)")
    );
}

#[test]
fn unchanged_bundles_show_nothing() {
    let text = numbered(5, "\n");
    assert_eq!(view(&one(&text), &one(&text), 80, 40), Ok(Vec::new()));
}

#[test]
fn added_and_removed_files_are_listed() {
    let old = bundle(&[("a.rs", b"x\n"), ("b.rs", b"y\n")]);
    let new = bundle(&[("a.rs", b"x\n"), ("c.rs", b"z\nw\n")]);
    let lines = view(&old, &new, 80, 40).unwrap();
    assert_eq!(
        lines,
        [
            "b.rs: removed, 1 lines",
            "- | y",
            "c.rs: added, 2 lines",
            "+ | z",
            "+ | w"
        ]
    );
}

#[test]
fn a_view_that_does_not_fit_is_refused() {
    let old = numbered(100, "\n");
    let new = numbered(100, "!\n");
    assert_eq!(
        view(&one(&old), &one(&new), 80, 40),
        Err(ViewError::TooLarge)
    );
    // A line wider than the screen is refused rather than wrapped.
    let long = "x".repeat(77);
    let new = old.replace("line 50\n", &format!("{long}\n"));
    assert_eq!(
        view(&one(&old), &one(&new), 80, 10),
        Err(ViewError::TooWide)
    );
    assert!(view(&one(&old), &one(&new), 81, 10).is_ok());
    assert!(matches!(
        view(b"junk", &one(&old), 80, 40),
        Err(ViewError::Bundle(_))
    ));
}

#[test]
fn lines_that_cannot_be_shown_exactly_are_refused() {
    for new in ["a\x1b[2J\n", "a\tb\n", "a\u{202e}b\n"] {
        assert_eq!(
            view(&one("a\n"), &one(new), 80, 40),
            Err(ViewError::Unshowable),
            "{new:?}"
        );
    }
    let latin = bundle(&[(PATH, b"a\n\xff\n")]);
    assert_eq!(
        view(&one("a\n"), &latin, 80, 40),
        Err(ViewError::Unshowable)
    );
    // A tab that became a space is a change of text, not of line endings.
    let lines = view(&one("a b\n"), &one("a c\n"), 80, 40).unwrap();
    assert!(!lines[0].contains("line endings only"));
}

#[test]
fn paths_in_headers_are_shown_safely() {
    let change = FileChange {
        path: "a\u{c}b.rs",
        kind: Kind::Added,
        line: 1,
        before: Vec::new(),
        removed: Vec::new(),
        added: Vec::new(),
        after: Vec::new(),
    };
    assert_eq!(header(&change), "a?b.rs: added, 0 lines");
}

#[test]
fn random_edits_never_panic() {
    let base = numbered(30, "\r\n").into_bytes();
    let mut seed: u32 = 12345;
    let mut next = || {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
        (seed >> 16) as usize
    };
    for _ in 0..300 {
        let mut edited = base.clone();
        for _ in 0..1 + next() % 4 {
            let at = next() % (edited.len() + 1);
            match next() % 3 {
                0 => edited.insert(at, b"\n\rz"[next() % 3]),
                1 if at < edited.len() => {
                    edited.remove(at);
                }
                _ => edited.truncate(at),
            }
        }
        let old = bundle(&[(PATH, &base)]);
        let new = bundle(&[(PATH, &edited)]);
        let _ = view(&old, &new, 1 + next() % 100, next() % 60);
        let mut damaged = new.clone();
        damaged.truncate(next() % (new.len() + 1));
        let _ = view(&old, &damaged, 80, 40);
    }
}
