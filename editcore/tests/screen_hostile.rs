//! Hostile input for the screen's three parsers: the source-link scanner (it
//! reads logs and file text), the service's `view` and `files` answers (they
//! cross the kernel and an app's effect result), and `kai-draft 1` (it comes
//! back from storage). Each sample, cut at every byte and mutated thousands of
//! times, parses or is refused. None panics, none allocates more than a small
//! multiple of its input, and whatever parses is inside its limits.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use editcore::screen::draft::{Draft, MAX_COMMANDS, MAX_DRAFT, MAX_TEXT};
use editcore::screen::link::{self, MAX_LINKS, MAX_SCAN};
use editcore::screen::reply::{self, Flags};
use editcore::service::view::{MAX_VIEW, MAX_VIEW_CHARS};
use editcore::{MAX_FILES, MAX_LINES};

const ALLOC_PER_BYTE: usize = 16;
const ALLOC_FLOOR: usize = 16 * 1024;

fn budget(len: usize) -> usize {
    ALLOC_FLOOR + len * ALLOC_PER_BYTE
}

/// Runs `f` on `bytes` as text (lossy, so a mutated sample is still a `str`),
/// and checks the allocation.
fn bounded(bytes: &[u8], f: impl FnOnce(&str)) {
    let text = String::from_utf8_lossy(bytes).into_owned();
    let ((), peak) = hostile::peak_alloc(|| f(&text));
    assert!(
        peak <= budget(text.len()),
        "{peak} bytes for {} bytes of input",
        text.len()
    );
}

const VIEW: &str = "view notes 2 components/notes/src/lib.rs 120 7 cu\n\
5:use alloc::vec::Vec;\n6:\n7:fn main() {\n8~a line cut for the screen\n";

const FILES: &str = "2 files; * changed, > current\n*> components/notes/src/lib.rs (120 lines)\n   components/notes/src/ui.rs (40 lines)";

fn draft() -> String {
    Draft {
        component: "notes".into(),
        label: "e1".into(),
        file: "components/notes/src/lib.rs".into(),
        line: 9,
        col: 3,
        top: 4,
        commands: vec!["c 3 let x = 1;".into(), "split 5 4 2".into(), "undo".into()],
        dirty: Some((9, "let y = ".into())),
    }
    .encode()
}

const LOG: &str = "error[E0425]: cannot find value `x`\n --> components/notes/src/lib.rs:58:9\n  |\n58 |     x + 1\nsee also src/ui.rs:7, lib.rs:12:3: and std::io::Error:5 and ../../etc/passwd:3\n";

#[test]
fn links_in_hostile_text_are_bounded_and_inside_the_text() {
    let check = |text: &str| {
        let links = link::scan(text);
        assert!(links.len() <= MAX_LINKS);
        let mut last = 0;
        for l in &links {
            assert!(l.start >= last && l.start < l.end && l.end <= text.len().min(MAX_SCAN));
            assert!(text.is_char_boundary(l.start) && text.is_char_boundary(l.end));
            let words = &text[l.start..l.end];
            assert!(words.starts_with(l.path.as_str()), "{words:?} {l:?}");
            assert!((1..=99_999).contains(&l.line) && l.column <= 9_999);
            assert!(l.path.len() <= 64 && !l.path.starts_with('/') && !l.path.contains(".."));
            last = l.end;
        }
    };
    hostile::truncations(LOG.as_bytes(), |_, bytes| bounded(bytes, check));
    hostile::fuzz(LOG.as_bytes(), 7, 4000, |bytes| bounded(bytes, check));
    for text in [
        "x".repeat(1 << 20),
        ":".repeat(1 << 16),
        "a.rs:".repeat(1 << 14),
        format!("{}lib.rs:5", "../".repeat(100_000)),
        format!("a.rs:{}", "9".repeat(100_000)),
        "\u{1f600}lib.rs:5\u{1f600}".repeat(1000),
        format!("{}a.rs:12", "p/".repeat(MAX_SCAN)),
    ] {
        bounded(text.as_bytes(), |t| check(t));
    }
}

#[test]
fn links_cut_by_the_scan_limit_never_name_a_wrong_line() {
    // `lib.rs:12` cut after `lib.rs:1` would name line 1.
    let padding = "-".repeat(MAX_SCAN - "lib.rs:1".len() - 1);
    let text = format!("{padding} lib.rs:12345");
    for link in link::scan(&text) {
        assert_ne!(link.line, 1, "{link:?}");
    }
    let whole = format!("lib.rs:12345 {}", "x".repeat(2 * MAX_SCAN));
    assert_eq!(link::scan(&whole)[0].line, 12345);
}

#[test]
fn view_replies_parse_or_are_refused_and_stay_inside_their_limits() {
    let check = |text: &str| {
        let Some(body) = text.strip_prefix("ok ") else {
            return;
        };
        if let Ok(view) = reply::parse_view(body) {
            assert!(view.len <= MAX_LINES && view.cursor <= MAX_LINES + 1);
            assert!(view.lines.len() <= MAX_VIEW);
            let mut last = 0;
            for shown in &view.lines {
                assert!(shown.n > last && shown.n <= view.len);
                assert!(shown.text.chars().count() <= MAX_VIEW_CHARS);
                assert!(!shown.text.chars().any(|c| c.is_control() && c != '\t'));
                last = shown.n;
            }
            assert!(!view.path.starts_with(".../") && !view.path.starts_with('/'));
        }
    };
    let sample = format!("ok {VIEW}");
    hostile::truncations(sample.as_bytes(), |_, bytes| bounded(bytes, check));
    hostile::fuzz(sample.as_bytes(), 11, 6000, |bytes| bounded(bytes, check));
    let many = format!(
        "ok view notes 2 a/b.rs 9999 1 -\n{}",
        "1:x\n".repeat(100_000)
    );
    bounded(many.as_bytes(), check);
    let huge = format!("ok view notes 2 a/b.rs 9999 1 -\n1:{}", "y".repeat(1 << 20));
    bounded(huge.as_bytes(), check);
}

#[test]
fn a_well_formed_view_reads_back_exactly() {
    let view = reply::parse_view(VIEW.trim_end()).unwrap();
    assert_eq!(view.component, "notes");
    assert_eq!((view.len, view.cursor), (120, 7));
    assert_eq!(
        view.flags,
        Flags {
            changed: true,
            undo: true,
            built: false
        }
    );
    let numbers: Vec<_> = view.lines.iter().map(|l| (l.n, l.exact)).collect();
    assert_eq!(numbers, [(5, true), (6, true), (7, true), (8, false)]);
    // Lines out of order, past the end, or with no number end the reply.
    for bad in [
        "view notes 2 a/b.rs 3 1 -\n2:x\n1:y",
        "view notes 2 a/b.rs 3 1 -\n2:x\n2:y",
        "view notes 2 a/b.rs 3 1 -\n4:x",
        "view notes 2 a/b.rs 3 1 -\n0:x",
        "view notes 2 a/b.rs 3 1 -\nx",
        "view notes 2 /etc/passwd 3 1 -",
        "view notes 2 ../b.rs 3 1 -",
        "view notes 2 a/b.rs 3 1 z",
        "view Notes 2 a/b.rs 3 1 -",
        "view notes 2 a/b.rs 99999 1 -",
        "view notes 2 a/b.rs 3",
    ] {
        assert!(reply::parse_view(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn file_lists_parse_or_are_refused() {
    let check = |text: &str| {
        if let Ok(files) = reply::parse_files(text) {
            assert!(!files.is_empty() && files.len() <= MAX_FILES);
            assert!(
                files
                    .iter()
                    .all(|f| f.len <= MAX_LINES && !f.path.starts_with('/'))
            );
        }
    };
    hostile::truncations(FILES.as_bytes(), |_, bytes| bounded(bytes, check));
    hostile::fuzz(FILES.as_bytes(), 13, 6000, |bytes| bounded(bytes, check));
    let many = format!("99 files; x\n{}", "   a/b.rs (1 lines)\n".repeat(10_000));
    bounded(many.as_bytes(), check);
    assert!(reply::parse_files(FILES).is_ok());
    assert!(reply::parse_files("3 files; x\n   a/b.rs (1 lines)").is_err());
}

#[test]
fn drafts_round_trip_and_hostile_ones_are_refused() {
    let sample = draft();
    let parsed = Draft::parse(&sample).unwrap();
    assert_eq!(parsed.encode(), sample);
    let check = |text: &str| {
        if let Ok(parsed) = Draft::parse(text) {
            assert!(text.len() <= MAX_DRAFT && parsed.commands.len() <= MAX_COMMANDS);
            assert!(parsed.commands.iter().all(|c| c.len() <= MAX_TEXT));
            assert!(!parsed.file.starts_with('/') && !parsed.file.contains(".."));
            // Whatever parses encodes to something that parses again, equal.
            assert_eq!(Draft::parse(&parsed.encode()).as_ref(), Ok(&parsed));
        }
    };
    hostile::truncations(sample.as_bytes(), |_, bytes| bounded(bytes, check));
    hostile::fuzz(sample.as_bytes(), 17, 8000, |bytes| bounded(bytes, check));
    for bad in [
        // Only editing commands replay: never a build, a try or an open.
        "kai-draft 1\ncomponent notes\nlabel 2\nfile a/b.rs\nat 1 0 1\ncmd build\n",
        "kai-draft 1\ncomponent notes\nlabel 2\nfile a/b.rs\nat 1 0 1\ncmd open x\n",
        "kai-draft 1\ncomponent notes\nlabel 2\nfile a/b.rs\nat 1 0 1\ncmd c 1 a\u{7}b\n",
        "kai-draft 1\ncomponent notes\nlabel 2\nfile ../b.rs\nat 1 0 1\n",
        "kai-draft 1\ncomponent notes\nlabel 2\nfile /b.rs\nat 1 0 1\n",
        "kai-draft 1\ncomponent notes\nlabel 2\nfile a/b.rs\nat 1 0\n",
        "kai-draft 1\ncomponent notes\nlabel 2\nfile a/b.rs\nat 1 0 99999999\n",
        "kai-draft 1\ncomponent notes\nlabel 2\nfile a/b.rs\nat 1 0 1\ndirty 1 x\ncmd c 1 y\n",
        "kai-draft 1\ncomponent notes\nlabel 2\nfile a/b.rs\nat 1 0 1\ndirty 0 x\n",
        "kai-draft 2\n",
        "",
    ] {
        assert!(Draft::parse(bad).is_err(), "{bad:?}");
    }
    let many = format!(
        "kai-draft 1\ncomponent notes\nlabel 2\nfile a/b.rs\nat 1 0 1\n{}",
        "cmd undo\n".repeat(MAX_COMMANDS + 1)
    );
    assert!(Draft::parse(&many).is_err());
    let big = format!("{sample}{}", " ".repeat(MAX_DRAFT));
    assert!(Draft::parse(&big).is_err());
}
