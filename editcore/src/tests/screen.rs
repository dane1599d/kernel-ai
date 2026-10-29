//! A `Screen` driven against the real edit `Service`, as the kernel would call
//! it: every key, answer and restart is the shipped code.

use super::fake::{Fake, Host, service};
use super::{LIB, numbered};
use crate::screen::{Draft, Input, Key, Screen};
use crate::service::Service;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

fn key(k: termcore::keys::Key) -> Key {
    use termcore::keys::Key as T;
    match k {
        T::Char(c) => Key::Char(c),
        T::Enter => Key::Enter,
        T::Backspace => Key::Backspace,
        T::Delete => Key::Delete,
        T::Tab => Key::Tab,
        T::Esc => Key::Esc,
        T::Up => Key::Up,
        T::Down => Key::Down,
        T::Left => Key::Left,
        T::Right => Key::Right,
        T::Home => Key::Home,
        T::End => Key::End,
        T::PageUp => Key::PageUp,
        T::PageDown => Key::PageDown,
        T::Ctrl(c) => Key::Ctrl(c),
    }
}

pub(super) struct Rig {
    pub screen: Screen,
    pub service: Service,
    pub env: Fake,
}

impl Rig {
    pub fn new(lib: &str, rows: usize, cols: usize) -> Self {
        let mut rig = Self {
            screen: Screen::new(rows, cols),
            service: service(),
            env: Fake::new(lib),
        };
        rig.screen.open("notes", None, None);
        rig.pump();
        rig
    }

    /// Answers every request the screen has, in order.
    pub fn pump(&mut self) {
        for _ in 0..2000 {
            let Some(out) = self.screen.next_request() else {
                return;
            };
            let reply = self.service.handle(&mut self.env, 0, &out.request.encode());
            self.screen.reply(out.id, &reply);
        }
        panic!("the screen never stopped asking");
    }

    /// Presses keys, answering everything after each.
    pub fn press(&mut self, tokens: &str) {
        for k in termcore::keys::parse_tokens(tokens).unwrap() {
            self.screen.input(Input::Key(key(k)));
            self.pump();
        }
    }

    /// Presses keys without answering anything until all are down.
    pub fn type_ahead(&mut self, tokens: &str) {
        for k in termcore::keys::parse_tokens(tokens).unwrap() {
            self.screen.input(Input::Key(key(k)));
        }
        self.pump();
    }

    pub fn source(&self) -> String {
        let session = self.service.session.as_ref().unwrap();
        String::from_utf8(session.current().bytes()).unwrap()
    }

    /// The text rows as `n text`, blank rows as `-`.
    pub fn rows(&self) -> Vec<String> {
        let view = self.screen.view();
        view.rows
            .iter()
            .map(|r| match r.number {
                Some(n) => format!("{n} {}", r.text),
                None => String::from("-"),
            })
            .collect()
    }

    /// Everything the screen holds of the file is what the service holds.
    pub fn assert_consistent(&mut self) {
        self.screen.commit();
        self.pump();
        let source = self.source();
        let lines: Vec<&str> = source
            .split_inclusive('\n')
            .map(|l| l.trim_end_matches('\n'))
            .collect();
        assert_eq!(self.screen.doc.len, lines.len(), "length");
        for (n, cell) in &self.screen.doc.lines {
            if cell.exact {
                assert_eq!(
                    lines.get(n - 1).copied(),
                    Some(cell.text.as_str()),
                    "line {n}"
                );
            }
        }
    }
}

#[test]
fn it_opens_and_shows_the_first_lines_with_their_numbers() {
    let rig = Rig::new(&numbered(30, "\n"), 10, 40);
    let view = rig.screen.view();
    assert_eq!(view.title, format!("notes 2: {LIB}"));
    assert_eq!(view.gutter, 3);
    assert_eq!(view.cursor, Some((0, 0)));
    assert_eq!(rig.rows()[0], "1 line 1");
    assert_eq!(rig.rows()[9], "10 line 10");
    assert_eq!(view.mode, "edit");
}

#[test]
fn typing_is_one_change_sent_when_the_cursor_leaves_the_line() {
    let mut rig = Rig::new(&numbered(5, "\n"), 10, 40);
    rig.press("XY");
    assert!(rig.source().starts_with("line 1\n"), "nothing sent yet");
    assert_eq!(rig.rows()[0], "1 XYline 1");
    rig.press("<down>");
    assert!(rig.source().starts_with("XYline 1\nline 2\n"));
    assert_eq!(rig.screen.draft().unwrap().commands, ["c 1 XYline 1"]);
    assert!(rig.screen.view().title.ends_with(" *"));
    rig.assert_consistent();
}

#[test]
fn backspace_and_delete_edit_the_line_being_typed() {
    let mut rig = Rig::new("abcdef\nxyz\n", 10, 40);
    rig.press("<right><right><right><bs><del>");
    assert_eq!(rig.rows()[0], "1 abef");
    assert_eq!(rig.screen.view().cursor, Some((0, 2)));
    rig.press("<tab>!<down>");
    assert_eq!(rig.source(), "ab    !ef\nxyz\n");
}

#[test]
fn up_and_down_keep_the_column_they_started_from() {
    let mut rig = Rig::new("longer line\nab\nlonger line\n", 10, 40);
    rig.press("<right><right><right><right><right><right><down>");
    assert_eq!(rig.screen.view().cursor, Some((1, 2)));
    rig.press("<down>");
    assert_eq!(rig.screen.view().cursor, Some((2, 6)));
}

#[test]
fn enter_splits_the_line_indents_like_it_and_one_undo_takes_it_back() {
    let original = "fn a() {\n    x;\n}\n";
    let mut rig = Rig::new(original, 10, 40);
    rig.press("<end><enter>");
    assert_eq!(rig.source(), "fn a() {\n    \n    x;\n}\n");
    assert_eq!(rig.screen.view().cursor, Some((1, 4)));
    rig.press("y;<enter>z");
    assert_eq!(rig.rows()[1], "2     y;");
    assert_eq!(rig.rows()[2], "3     z");
    // Four edits were made: a split, a change, a split and a change.
    for _ in 0..4 {
        rig.press("<C-z>");
    }
    assert_eq!(rig.source(), original);
    rig.assert_consistent();
}

#[test]
fn backspace_at_the_start_and_delete_at_the_end_join_lines() {
    let mut rig = Rig::new("one\ntwo\nthree\n", 10, 40);
    rig.press("<down><bs>");
    assert_eq!(rig.source(), "onetwo\nthree\n");
    assert_eq!(rig.screen.view().cursor, Some((0, 3)));
    rig.press("<end><del>");
    assert_eq!(rig.source(), "onetwothree\n");
    rig.press("<C-z>");
    assert_eq!(rig.source(), "onetwo\nthree\n");
    rig.assert_consistent();
}

#[test]
fn paging_fetches_the_lines_it_has_not_got() {
    let mut rig = Rig::new(&numbered(300, "\n"), 20, 40);
    let asked = rig.service.session.is_some();
    assert!(asked);
    rig.press("<pgdn><pgdn><pgdn>");
    let rows = rig.rows();
    assert_eq!(rows[0], "58 line 58");
    assert_eq!(rows[19], "77 line 77");
    rig.press("<pgup>");
    assert_eq!(rig.rows()[0], "39 line 39");
    rig.screen.commit();
    rig.assert_consistent();
}

#[test]
fn keys_typed_while_requests_are_in_flight_give_the_same_text() {
    let script = "ab<enter>cd<up><bs><bs><down><down>x<end><enter><enter>q<up><up><del><C-z>";
    let mut slow = Rig::new("alpha beta\ngamma\ndelta\n", 8, 30);
    slow.type_ahead(script);
    let mut quick = Rig::new("alpha beta\ngamma\ndelta\n", 8, 30);
    quick.press(script);
    slow.assert_consistent();
    quick.assert_consistent();
    assert_eq!(slow.source(), quick.source());
    assert_eq!(slow.rows(), quick.rows());
}

#[test]
fn a_build_runs_in_the_background_and_the_status_follows_it() {
    let mut rig = Rig::new(&numbered(5, "\n"), 10, 40);
    rig.env.busy_polls = 3;
    rig.press("x<C-b>");
    assert!(rig.screen.view().building);
    assert!(rig.screen.view().status.starts_with("building e1 from 2"));
    // Typing goes on while the host compiles.
    rig.press("y");
    let mut now = 0;
    while rig.screen.view().building {
        rig.screen.tick(now);
        rig.pump();
        now += 500;
        assert!(now < 20_000, "the build never finished");
    }
    let view = rig.screen.view();
    assert!(
        view.status.starts_with("ok built e1") || view.status.starts_with("built e1"),
        "{}",
        view.status
    );
    assert!(!view.status_error);
    assert_eq!(rig.env.puts.len(), 1);
}

#[test]
fn a_failed_build_moves_to_the_error_and_a_ctrl_click_opens_a_link() {
    let mut rig = Rig::new("let a = 1;\nlet b = 2;\nlet c = 3;\nlet d = 4;\n", 10, 60);
    rig.env.host = Host::Fails;
    rig.press("x<C-b>");
    let mut now = 0;
    while rig.screen.view().building {
        rig.screen.tick(now);
        rig.pump();
        now += 500;
        assert!(now < 20_000);
    }
    let view = rig.screen.view();
    assert_eq!(view.mode, "pager", "the log is shown");
    assert!(view.status_error);
    assert_eq!(rig.screen.line, 2, "the cursor went to the first error");
    // Back in the text, move away, then follow the log's link.
    rig.press("<esc><down><down>");
    assert_eq!(rig.screen.line, 4);
    rig.press("<C-l>");
    let view = rig.screen.view();
    let (at, row) = view
        .rows
        .iter()
        .enumerate()
        .find(|(_, r)| !r.links.is_empty())
        .expect("a row with a source link");
    let (from, _) = row.links[0];
    rig.screen.input(Input::Click {
        row: at,
        col: from + 2,
        ctrl: true,
    });
    rig.pump();
    assert_eq!(rig.screen.view().mode, "edit");
    assert_eq!(rig.screen.line, 2);
}

#[test]
fn find_lists_hits_and_enter_on_one_goes_there() {
    let mut rig = Rig::new(&numbered(40, "\n"), 10, 40);
    rig.press("<C-f>line 3<enter>");
    let view = rig.screen.view();
    assert_eq!(view.mode, "pager");
    // Row 0 says how many lines hold it; the first hit is the next row.
    rig.press("<down>");
    rig.press("<enter>");
    assert_eq!(rig.screen.view().mode, "edit");
    assert_eq!(rig.screen.line, 3);
    // Esc leaves a prompt and a pager alike.
    rig.press("<C-f>zzz<esc>");
    assert_eq!(rig.screen.view().mode, "edit");
}

#[test]
fn go_to_line_and_the_command_prompt_work() {
    let mut rig = Rig::new(&numbered(80, "\n"), 10, 40);
    rig.press("<C-g>55<enter>");
    assert_eq!(rig.screen.line, 55);
    assert!(rig.rows().iter().any(|r| r == "55 line 55"));
    rig.press("<C-e>s |line 55|fifty five|<enter>");
    assert!(rig.source().contains("\nfifty five\n"));
    assert!(rig.rows().iter().any(|r| r == "55 fifty five"));
    rig.press("<C-e>nonsense<enter>");
    assert!(rig.screen.view().status_error);
}

#[test]
fn the_files_pager_lists_and_opens_files() {
    let mut rig = Rig::new("x\n", 10, 60);
    rig.press("<C-o>");
    assert_eq!(rig.screen.view().mode, "pager");
    assert!(
        rig.screen
            .view()
            .rows
            .iter()
            .any(|r| r.text.contains("Cargo.toml"))
    );
    // Row 0 is the heading; the first file is selected.
    rig.press("<enter>");
    rig.pump();
    assert_eq!(rig.screen.view().mode, "edit");
}

#[test]
fn a_line_too_long_for_the_screen_is_not_typed_into() {
    let long = "x".repeat(300);
    let mut rig = Rig::new(&format!("{long}\nshort\n"), 10, 40);
    rig.press("a");
    assert!(rig.screen.view().status_error);
    assert!(rig.source().starts_with(&long));
    rig.press("<down>b<down>");
    assert!(rig.source().ends_with("bshort\n") || rig.source().contains("bshort"));
}

#[test]
fn only_printable_ascii_is_typed() {
    let mut rig = Rig::new("a\n", 5, 20);
    rig.screen.input(Input::Key(Key::Char('\u{e9}')));
    rig.screen.input(Input::Key(Key::Char('\u{7}')));
    assert!(rig.screen.view().status_error);
    assert_eq!(rig.rows()[0], "1 a");
}

#[test]
fn a_draft_brings_back_every_edit_and_the_typed_line_after_a_crash() {
    let lib = "fn a() {\n    x;\n}\n// tail\n";
    let mut rig = Rig::new(lib, 10, 40);
    rig.press("<end><enter>y;<down>");
    rig.press("<down><down>zz");
    let saved = rig.screen.draft().unwrap().encode();
    let expected = rig.source();
    let rows = rig.rows();
    // The whole machine goes down: a new service, a new store, a new screen.
    let draft = Draft::parse(&saved).unwrap();
    let mut fresh = Rig::new(lib, 10, 40);
    assert_eq!(fresh.source(), lib);
    fresh.screen = Screen::new(10, 40);
    fresh.screen.resume(draft);
    fresh.pump();
    assert_eq!(fresh.source(), expected);
    assert_eq!(fresh.rows(), rows, "the typed line is back too");
    assert_eq!(fresh.screen.line, 5);
    fresh.press("<down>");
    assert!(fresh.source().contains("zz"));
}

#[test]
fn a_draft_does_not_double_edits_a_service_still_holds() {
    let mut rig = Rig::new("a\nb\n", 10, 40);
    rig.press("<end>!<enter>?<down>");
    let draft = rig.screen.draft().unwrap();
    let before = rig.source();
    rig.screen = Screen::new(10, 40);
    rig.screen.resume(draft);
    rig.pump();
    assert_eq!(rig.source(), before);
}

#[test]
fn edits_not_yet_answered_are_in_the_draft() {
    let mut rig = Rig::new("a\nb\n", 10, 40);
    rig.screen.input(Input::Key(Key::Char('x')));
    rig.screen.input(Input::Key(Key::Down));
    assert!(rig.screen.next_request().is_some(), "an edit is in flight");
    let draft = rig.screen.draft().unwrap();
    assert_eq!(draft.commands, ["c 1 xa"]);
}

#[test]
fn a_failed_call_keeps_what_was_typed_and_asks_again() {
    let mut rig = Rig::new("a\nb\n", 10, 40);
    rig.screen.input(Input::Key(Key::Char('x')));
    rig.screen.input(Input::Key(Key::Down));
    let out = rig.screen.next_request().unwrap();
    rig.screen.failed(out.id, "TimedOut");
    assert!(rig.screen.view().status_error);
    rig.pump();
    // The edit never reached the service; the screen shows the service's text.
    assert_eq!(rig.source(), "a\nb\n");
    assert_eq!(rig.rows()[0], "1 a");
}

/// A fixed pseudo-random stream, so a failure repeats.
struct Dice(u64);

impl Dice {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

const KEYS: &[&str] = &[
    "a",
    "b",
    " ",
    "{",
    "<enter>",
    "<bs>",
    "<del>",
    "<tab>",
    "<up>",
    "<down>",
    "<left>",
    "<right>",
    "<home>",
    "<end>",
    "<pgup>",
    "<pgdn>",
    "<C-z>",
    "<esc>",
    "<C-d>",
    "<C-l>",
    "<C-o>",
    "<C-f>x<enter>",
    "<C-g>3<enter>",
];

#[test]
fn random_keys_never_leave_the_screen_and_the_service_apart() {
    for (seed, every) in [(3u64, 1), (5, 4), (9, 7), (11, 1000)] {
        let mut rig = Rig::new(&numbered(40, "\n"), 9, 30);
        let mut dice = Dice(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        for step in 0..600 {
            let tokens = KEYS[dice.below(KEYS.len())];
            for k in termcore::keys::parse_tokens(tokens).unwrap() {
                rig.screen.input(Input::Key(key(k)));
            }
            if step % every == 0 {
                rig.pump();
            }
            let view = rig.screen.view();
            assert!(view.rows.len() <= 9);
            if let Some((row, cell)) = view.cursor {
                assert!(row < 9 && cell <= 30);
            }
            assert!(rig.screen.line >= 1);
        }
        rig.pump();
        assert!(
            rig.screen.draft_version() > 20,
            "{}",
            rig.screen.draft_version()
        );
        // Leave any pager or prompt, then compare.
        rig.press("<esc><esc>");
        rig.assert_consistent();
    }
}
