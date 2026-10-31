//! The Terminal model against the shell's wire, as the kernel terminal and the
//! AI would use it.

use comp_terminal::span::{Kind, MAX_LINE_BYTES, MAX_SPANS, Span};
use comp_terminal::state::State;
use comp_terminal::{
    Ask, CANCEL_ENTRY, CONTINUE_PROMPT, DEFAULT_PROMPT, GREETING, MAX_LINE, MAX_SENT_BYTES, Phase,
    READ_MAX_SPANS, Refusal, Source, Submitted, Terminal, valid_prompt,
};
use termcore::menu::{Menu, Origin};

fn sent(t: &mut Terminal, line: &str) -> String {
    match t.submit(line, Source::Typed) {
        Submitted::Send(line) => line,
        other => panic!("expected a send, got {other:?}"),
    }
}

fn kinds(t: &Terminal) -> Vec<(Kind, String)> {
    t.spans().iter().map(|s| (s.kind, s.text.clone())).collect()
}

fn menu_wire(id: u64) -> String {
    let call = "process_stop(\"build-3\")";
    let question = format!("Stop {call}? It has no undo.");
    Menu::encode(id, Origin::Asked, &question, "", "", call)
}

#[test]
fn it_starts_with_a_greeting_and_the_default_prompt() {
    let t = Terminal::new();
    assert_eq!(kinds(&t), [(Kind::Notice, GREETING.to_string())]);
    assert_eq!((t.prompt(), t.phase()), (DEFAULT_PROMPT, Phase::Ready));
    assert!(t.takes_input());
}

#[test]
fn a_line_is_echoed_with_its_prompt_then_answered() {
    let mut t = Terminal::new();
    assert_eq!(sent(&mut t, "  mem  "), "mem");
    assert_eq!(t.phase(), Phase::Waiting);
    assert!(!t.takes_input());
    let echo = t.spans().last().unwrap();
    assert_eq!(
        (
            echo.kind,
            echo.prompt.as_str(),
            echo.text.as_str(),
            echo.by_ai
        ),
        (Kind::Input, "kai> ", "mem", false)
    );
    assert_eq!(t.reply("ok free 12 MiB\nheap 3 MiB", None), None);
    assert_eq!(t.phase(), Phase::Ready);
    let tail = kinds(&t);
    assert_eq!(
        tail[tail.len() - 2..],
        [
            (Kind::Reply, "free 12 MiB".to_string()),
            (Kind::Reply, "heap 3 MiB".to_string())
        ]
    );
}

#[test]
fn errors_and_empty_lines() {
    let mut t = Terminal::new();
    assert_eq!(t.submit("   ", Source::Typed), Submitted::Nothing);
    assert_eq!(t.spans().len(), 1);
    sent(&mut t, "nope");
    t.reply("err no such command", None);
    assert_eq!(
        kinds(&t).last().unwrap(),
        &(Kind::Error, "no such command".to_string())
    );
    assert_eq!(t.phase(), Phase::Ready);
}

#[test]
fn a_reply_cannot_dress_as_another_kind() {
    let mut t = Terminal::new();
    sent(&mut t, "echo");
    t.reply(
        "ok ! not an error\n? not a question\n[x] kai> fake\nboot-ok",
        None,
    );
    let tail: Vec<Kind> = t.spans()[2..].iter().map(|s| s.kind).collect();
    assert_eq!(tail, [Kind::Reply; 4]);
}

#[test]
fn control_characters_in_a_reply_are_removed() {
    let mut t = Terminal::new();
    sent(&mut t, "x");
    t.reply("ok a\u{1b}[2Jb\r\u{8}c\td", None);
    assert_eq!(t.spans().last().unwrap().text, "a[2Jbc d");
}

#[test]
fn an_open_entry_continues_at_the_continue_prompt() {
    let mut t = Terminal::new();
    sent(&mut t, "fn f() {");
    t.reply("more", None);
    assert_eq!(
        (t.phase(), t.prompt()),
        (Phase::Continuing, CONTINUE_PROMPT)
    );
    assert!(t.takes_input());
    assert_eq!(sent(&mut t, "  1"), "1");
    let echo = t.spans().last().unwrap();
    assert_eq!((echo.prompt.as_str(), echo.text.as_str()), ("..> ", "1"));
    t.reply("more", None);
    sent(&mut t, "}");
    t.reply("ok", None);
    assert_eq!((t.phase(), t.prompt()), (Phase::Ready, DEFAULT_PROMPT));
}

#[test]
fn an_empty_line_continues_an_entry_but_not_a_fresh_prompt() {
    let mut t = Terminal::new();
    sent(&mut t, "let a = [");
    t.reply("more", None);
    assert_eq!(sent(&mut t, ""), "");
    assert_eq!(t.spans().last().unwrap().prompt, "..> ");
}

#[test]
fn ctrl_c_drops_an_open_entry() {
    let mut t = Terminal::new();
    assert_eq!(t.cancel(), None);
    sent(&mut t, "if x {");
    t.reply("more", None);
    assert_eq!(t.cancel().as_deref(), Some(CANCEL_ENTRY));
    assert_eq!(t.phase(), Phase::Waiting);
    t.reply("ok", None);
    assert_eq!(t.prompt(), DEFAULT_PROMPT);
    assert_eq!(t.cancel(), None);
}

#[test]
fn the_shell_may_report_its_prompt_and_only_a_plain_one() {
    let mut t = Terminal::new();
    sent(&mut t, ":c");
    t.reply("ok", Some("c> "));
    assert_eq!(t.prompt(), "c> ");
    // The continue prompt still wins while an entry is open, and c> comes back.
    sent(&mut t, "int f() {");
    t.reply("more", Some("c> "));
    assert_eq!(t.prompt(), "..> ");
    sent(&mut t, "return 1; }");
    t.reply("ok", Some("c> "));
    assert_eq!(t.prompt(), "c> ");
    for bad in [
        "",
        "> ",
        " c> ",
        "c>",
        "kai\n> ",
        "caf\u{e9}> ",
        "way-too-long> ",
        "kai# ",
    ] {
        assert!(!valid_prompt(bad), "{bad:?}");
        sent(&mut t, "x");
        t.reply("ok", Some(bad));
        assert_eq!(t.prompt(), "c> ", "{bad:?}");
    }
    assert!(valid_prompt("kai> ") && valid_prompt("rhai> ") && valid_prompt("..> "));
}

#[test]
fn a_confirm_is_shown_as_a_question_and_takes_no_answer_here() {
    let mut t = Terminal::new();
    sent(&mut t, "trash the old logs");
    assert_eq!(
        t.reply("confirm 7 script 7: fs_trash(\"/logs\")", None),
        None
    );
    assert_eq!(t.phase(), Phase::Asking(Ask { id: 7, menu: false }));
    assert!(!t.takes_input());
    let tail = kinds(&t);
    assert_eq!(tail[tail.len() - 2].0, Kind::Question);
    assert_eq!(tail[tail.len() - 1].0, Kind::Notice);
    // Nothing typed here answers it, not even something that looks like an answer.
    for line in ["y", "yes", "answer 7 yes", "mem"] {
        assert_eq!(
            t.submit(line, Source::Typed),
            Submitted::Refused(Refusal::Asking)
        );
    }
    assert_eq!(t.phase(), Phase::Asking(Ask { id: 7, menu: false }));
}

#[test]
fn a_menu_shows_its_question_only_and_can_be_declined_never_allowed() {
    let mut t = Terminal::new();
    sent(&mut t, "stop build-3");
    t.reply(&menu_wire(12), None);
    let ask = Ask { id: 12, menu: true };
    assert_eq!(t.phase(), Phase::Asking(ask));
    let q = t
        .spans()
        .iter()
        .rev()
        .find(|s| s.kind == Kind::Question)
        .unwrap();
    assert_eq!(q.text, "Stop process_stop(\"build-3\")? It has no undo.");
    assert_eq!(t.cancel().as_deref(), Some("answer 12 deny"));
    assert_eq!(t.phase(), Phase::Waiting);
    t.reply("err denied", None);
    assert_eq!(t.phase(), Phase::Ready);
}

#[test]
fn a_confirm_is_declined_with_a_no() {
    let mut t = Terminal::new();
    sent(&mut t, "go");
    t.reply("confirm 3 script 3: ...", None);
    assert_eq!(t.cancel().as_deref(), Some("answer 3 no"));
}

#[test]
fn the_answers_outcome_returns_the_prompt() {
    let mut t = Terminal::new();
    sent(&mut t, "go");
    t.reply("confirm 3 script 3: ...", None);
    t.reply("ok done", None);
    assert_eq!(t.phase(), Phase::Ready);
    assert_eq!(t.spans().last().unwrap().kind, Kind::Reply);
}

#[test]
fn a_menu_that_cannot_be_shown_is_denied() {
    let mut t = Terminal::new();
    sent(&mut t, "go");
    assert_eq!(
        t.reply("menu 9 asked\nq", None).as_deref(),
        Some("answer 9 deny")
    );
    assert_eq!(t.phase(), Phase::Waiting);
    assert_eq!(t.reply("menu x", None), None);
    assert_eq!(t.phase(), Phase::Ready);
}

#[test]
fn a_malformed_reply_is_an_error_row() {
    let mut t = Terminal::new();
    sent(&mut t, "go");
    t.reply("wat", None);
    assert_eq!(
        kinds(&t).last().unwrap(),
        &(Kind::Error, "the shell sent a malformed reply".to_string())
    );
    assert_eq!(t.phase(), Phase::Ready);
}

#[test]
fn a_busy_terminal_takes_no_second_line() {
    let mut t = Terminal::new();
    sent(&mut t, "slow");
    assert_eq!(
        t.submit("mem", Source::Ai),
        Submitted::Refused(Refusal::Busy)
    );
    assert!(t.spans().last().unwrap().text.contains("still running"));
}

#[test]
fn bad_lines_are_refused_with_a_notice() {
    let mut t = Terminal::new();
    let cases = [
        ("a\nb", Refusal::Control),
        ("a\u{1b}[2J", Refusal::Control),
        (&"x".repeat(MAX_LINE + 1), Refusal::TooLong),
        ("answer 4 allow", Refusal::Answer),
        ("answer 18446744073709551615 yes", Refusal::Answer),
    ];
    for (line, why) in cases {
        assert_eq!(
            t.submit(line, Source::Ai),
            Submitted::Refused(why),
            "{line:.20}"
        );
        assert_eq!(t.spans().last().unwrap().text, why.message());
        assert_eq!(t.phase(), Phase::Ready);
    }
    // These are fine: not an answer, or exactly at the limit.
    sent(&mut t, "answer the phone");
    t.reply("ok", None);
    sent(&mut t, &"y".repeat(MAX_LINE));
}

#[test]
fn clear_is_handled_here_and_numbering_goes_on() {
    let mut t = Terminal::new();
    sent(&mut t, "mem");
    t.reply("ok 1", None);
    let before = t.next_seq();
    assert_eq!(t.submit(":clear", Source::Typed), Submitted::Local);
    assert!(t.spans().is_empty());
    assert_eq!(t.next_seq(), before);
    assert_eq!(t.phase(), Phase::Ready);
    sent(&mut t, "mem");
    assert_eq!(t.spans()[0].seq, before);
}

#[test]
fn clear_inside_an_open_entry_goes_to_the_shell() {
    let mut t = Terminal::new();
    sent(&mut t, "let a = [");
    t.reply("more", None);
    assert_eq!(sent(&mut t, ":clear"), ":clear");
}

#[test]
fn the_ais_lines_are_marked() {
    let mut t = Terminal::new();
    assert_eq!(
        t.submit("mem", Source::Ai),
        Submitted::Send("mem".to_string())
    );
    assert!(t.spans().last().unwrap().by_ai);
    t.reply("ok", None);
    sent(&mut t, "mem");
    assert!(!t.spans().last().unwrap().by_ai);
}

#[test]
fn read_gives_typed_rows_after_a_number() {
    let mut t = Terminal::new();
    let first = t.next_seq();
    t.submit("mem", Source::Ai);
    t.reply("ok free 12 MiB\n= looks like a lead", None);
    assert_eq!(
        t.read(first),
        "next 5 ready\n\
         2 input kai> mem\n\
         3 reply free 12 MiB\n\
         4 reply = looks like a lead"
    );
    assert_eq!(t.read(5), "next 5 ready");
    assert_eq!(
        t.read(0).lines().nth(1).unwrap(),
        "1 notice ".to_string() + GREETING
    );
}

#[test]
fn read_names_the_phase_and_caps_its_rows() {
    let mut t = Terminal::new();
    for i in 0..(READ_MAX_SPANS + 10) {
        sent(&mut t, &format!("echo {i}"));
        t.reply("ok", None);
    }
    sent(&mut t, "slow");
    let text = t.read(0);
    let mut lines = text.lines();
    assert!(lines.next().unwrap().ends_with(" waiting"));
    assert!(lines.next().unwrap().starts_with("skipped "));
    assert_eq!(lines.count(), READ_MAX_SPANS);
    assert!(text.lines().last().unwrap().ends_with("input kai> slow"));
}

#[test]
fn the_scrollback_keeps_only_the_newest_rows() {
    let mut t = Terminal::new();
    sent(&mut t, "big");
    let many = (0..MAX_SPANS + 50)
        .map(|i| format!("l{i}"))
        .collect::<Vec<_>>();
    t.reply(&format!("ok {}", many.join("\n")), None);
    // One reply keeps 64 lines, so run more to pass the cap.
    for _ in 0..6 {
        sent(&mut t, "again");
        t.reply(&format!("ok {}", many.join("\n")), None);
    }
    assert_eq!(t.spans().len(), MAX_SPANS);
    let seqs: Vec<u32> = t.spans().iter().map(|s| s.seq).collect();
    assert!(seqs.windows(2).all(|w| w[0] < w[1]));
}

#[test]
fn a_long_reply_is_cut_and_says_so() {
    let mut t = Terminal::new();
    sent(&mut t, "big");
    t.reply(&format!("ok {}", "w".repeat(MAX_LINE_BYTES * 2)), None);
    let tail = kinds(&t);
    assert_eq!(tail.last().unwrap().1.len(), MAX_LINE_BYTES);
    sent(&mut t, "lines");
    t.reply(&format!("ok {}", "x\n".repeat(200)), None);
    assert_eq!(
        kinds(&t).last().unwrap(),
        &(Kind::Notice, "(the rest was left out)".to_string())
    );
}

fn round(t: &Terminal) -> Terminal {
    let bytes = minicbor::to_vec(t.to_state()).unwrap();
    Terminal::from_state(minicbor::decode::<State>(&bytes).unwrap())
}

#[test]
fn state_round_trips_with_the_phase() {
    let mut t = Terminal::new();
    sent(&mut t, "fn f() {");
    t.reply("more", Some("c> "));
    let back = round(&t);
    assert_eq!(back.spans(), t.spans());
    assert_eq!((back.phase(), back.prompt()), (Phase::Continuing, "..> "));
    assert_eq!(back.next_seq(), t.next_seq());

    sent(&mut t, "}");
    t.reply("confirm 5 script", None);
    let back = round(&t);
    assert_eq!(back.phase(), Phase::Asking(Ask { id: 5, menu: false }));
}

#[test]
fn a_restart_while_a_line_ran_returns_the_prompt() {
    let mut t = Terminal::new();
    sent(&mut t, "slow");
    let back = round(&t);
    assert_eq!(back.phase(), Phase::Ready);
    assert!(back.spans().last().unwrap().text.contains("restarted"));
}

#[test]
fn a_full_terminal_fits_the_checkpoint() {
    let mut t = Terminal::new();
    while t.spans().len() < MAX_SPANS {
        // Four-byte characters, the most a line can weigh.
        sent(&mut t, &"\u{20000}".repeat(MAX_SENT_BYTES / 4));
        t.reply(
            &format!("ok {}", "\u{20000}".repeat(MAX_LINE_BYTES)),
            Some("rhai> "),
        );
    }
    let bytes = minicbor::to_vec(t.to_state()).unwrap();
    assert!(bytes.len() <= 64 * 1024, "{} bytes", bytes.len());
    // Not by a small margin of a lucky mix: the rows are at their byte limit.
    assert!(t.spans().iter().all(|s| s.text.len() > MAX_LINE_BYTES - 4));
}

#[test]
fn hostile_state_is_cut_to_the_limits() {
    let span = |seq| Span {
        seq,
        kind: Kind::Reply,
        prompt: "p".repeat(100),
        text: "t".repeat(1000),
        by_ai: false,
    };
    let state = State {
        spans: (0..500).map(span).collect(),
        next_seq: 3,
        prompt: "bad".to_string(),
        phase: 250,
        ask: 1,
    };
    let t = Terminal::from_state(state);
    assert_eq!(t.spans().len(), MAX_SPANS);
    assert!(t.spans().iter().all(|s| s.text.len() <= MAX_LINE_BYTES));
    assert!(
        t.spans()
            .iter()
            .all(|s| s.prompt.len() <= comp_terminal::MAX_PROMPT)
    );
    assert_eq!((t.prompt(), t.phase()), (DEFAULT_PROMPT, Phase::Ready));
    assert!(t.next_seq() > t.spans().last().unwrap().seq);
}

#[test]
fn rows_read_back_are_numbered_one_after_another() {
    let span = |seq| Span {
        seq,
        kind: Kind::Notice,
        prompt: String::new(),
        text: format!("row {seq}"),
        by_ai: false,
    };
    // Repeats, a jump back and a gap of exactly the window's node slots.
    let state = State {
        spans: [5, 5, 3, 16_005, 0, u32::MAX]
            .into_iter()
            .map(span)
            .collect(),
        next_seq: 1,
        prompt: DEFAULT_PROMPT.to_string(),
        phase: 0,
        ask: 0,
    };
    let t = Terminal::from_state(state);
    let seqs: Vec<u32> = t.spans().iter().map(|s| s.seq).collect();
    assert_eq!(seqs, [5, 6, 7, 8, 9, 10]);
    assert_eq!(t.next_seq(), 11);
    // An honest state is not renumbered.
    let mut honest = Terminal::new();
    for _ in 0..3 {
        honest.submit("x", Source::Typed);
        honest.reply("ok", None);
    }
    assert_eq!(round(&honest).spans(), honest.spans());
}

#[test]
fn mutated_state_bytes_never_panic() {
    let mut t = Terminal::new();
    for i in 0..30 {
        sent(&mut t, &format!("line {i}"));
        t.reply(if i % 7 == 0 { "more" } else { "ok fine\nrows" }, None);
    }
    t.reply("confirm 4 script", None);
    let good = minicbor::to_vec(t.to_state()).unwrap();
    let mut seed = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut decoded = 0;
    for _ in 0..9000 {
        let mut bytes = good.clone();
        for _ in 0..1 + next() % 4 {
            let at = (next() % bytes.len() as u64) as usize;
            bytes[at] = next() as u8;
        }
        if next() % 5 == 0 {
            bytes.truncate((next() % bytes.len() as u64) as usize);
        }
        if let Ok(state) = minicbor::decode::<State>(&bytes) {
            decoded += 1;
            let t = Terminal::from_state(state);
            assert!(t.spans().len() <= MAX_SPANS);
            assert!(t.read(0).len() < 8 * 1024 + 200);
        }
    }
    assert!(decoded > 100, "only {decoded} mutations decoded");
}

#[test]
fn saved_text_loses_characters_that_hide_or_reorder() {
    let span = Span {
        seq: 1,
        kind: Kind::Reply,
        prompt: "k\u{200B}ai> ".to_string(),
        text: "a\u{202E}b\u{2028}c".to_string(),
        by_ai: false,
    };
    let state = State {
        spans: vec![span],
        next_seq: 2,
        prompt: DEFAULT_PROMPT.to_string(),
        phase: 0,
        ask: 0,
    };
    let t = Terminal::from_state(state);
    assert_eq!(t.spans()[0].prompt, "kai> ");
    assert_eq!(t.spans()[0].text, "abc");
}
