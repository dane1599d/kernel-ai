//! The terminal's effect and the AI's three actions, against a recording OS.

use comp_terminal::os::{MAX_ARGS, Os, Recorder, SHELL_LINE, shell_line_args};
use comp_terminal::{
    CANCEL_ENTRY, MAX_LINE, MAX_SENT_BYTES, Phase, Refusal, Source, Submitted, Terminal,
};

fn started(line: &str) -> (Terminal, Recorder, u32) {
    let mut t = Terminal::new();
    let mut os = Recorder::default();
    assert_eq!(
        t.run(&mut os, line, Source::Typed),
        Submitted::Send(line.to_string())
    );
    let ticket = os.last_ticket().unwrap();
    (t, os, ticket)
}

#[test]
fn a_line_is_one_shell_line_effect_and_its_answer_shows() {
    let (mut t, mut os, ticket) = started("mem");
    assert_eq!(os.asked.len(), 1);
    assert_eq!(os.asked[0].1, SHELL_LINE);
    assert_eq!(os.asked[0].2, r#"{line: "mem"}"#);
    assert_eq!(t.phase(), Phase::Waiting);
    assert!(t.done(&mut os, ticket, Ok("ok free 12 MiB".into())));
    assert_eq!(t.phase(), Phase::Ready);
    assert_eq!(t.spans().last().unwrap().text, "free 12 MiB");
    assert_eq!(os.asked.len(), 1);
}

#[test]
fn only_the_ticket_in_flight_is_taken() {
    let (mut t, mut os, ticket) = started("mem");
    assert!(!t.done(&mut os, ticket + 7, Ok("ok forged".into())));
    assert!(!t.done(&mut os, 0, Ok("ok forged".into())));
    assert_eq!(t.phase(), Phase::Waiting);
    assert!(t.done(&mut os, ticket, Ok("ok real".into())));
    // Once answered, the same ticket is spent.
    assert!(!t.done(&mut os, ticket, Ok("ok again".into())));
    assert_eq!(t.spans().last().unwrap().text, "real");
}

#[test]
fn an_effect_the_os_refused_is_an_error_row() {
    let (mut t, mut os, ticket) = started("mem");
    t.done(
        &mut os,
        ticket,
        Err("shell_line is not declared".to_string()),
    );
    assert_eq!(t.phase(), Phase::Ready);
    let last = t.spans().last().unwrap();
    assert_eq!(
        last.text,
        "the OS did not run it: shell_line is not declared"
    );
    assert_eq!(last.kind, comp_terminal::span::Kind::Error);
}

#[test]
fn an_open_entry_and_its_cancel_are_effects_too() {
    let (mut t, mut os, ticket) = started("if x {");
    t.done(&mut os, ticket, Ok("more".into()));
    assert_eq!(t.phase(), Phase::Continuing);
    assert!(t.stop(&mut os));
    assert_eq!(os.lines().last().unwrap(), r#"{line: "\u{3}"}"#);
    assert_eq!(CANCEL_ENTRY, "\u{3}");
    let ticket = os.last_ticket().unwrap();
    t.done(&mut os, ticket, Ok("ok".into()));
    assert_eq!(t.phase(), Phase::Ready);
    // Nothing to cancel at a fresh prompt.
    assert!(!t.stop(&mut os));
}

#[test]
fn a_menu_that_cannot_be_shown_is_denied_through_an_effect() {
    let (mut t, mut os, ticket) = started("go");
    t.done(&mut os, ticket, Ok("menu 9 asked\nq".into()));
    assert_eq!(os.lines().last().unwrap(), r#"{line: "answer 9 deny"}"#);
    assert_eq!(t.phase(), Phase::Waiting);
    let ticket = os.last_ticket().unwrap();
    t.done(&mut os, ticket, Ok("err denied".into()));
    assert_eq!(t.phase(), Phase::Ready);
}

#[test]
fn a_refused_or_local_line_asks_the_os_for_nothing() {
    let mut t = Terminal::new();
    let mut os = Recorder::default();
    assert_eq!(
        t.run(&mut os, "answer 3 yes", Source::Ai),
        Submitted::Refused(Refusal::Answer)
    );
    assert_eq!(t.run(&mut os, "  ", Source::Typed), Submitted::Nothing);
    assert_eq!(t.run(&mut os, ":clear", Source::Typed), Submitted::Local);
    assert!(os.asked.is_empty());
}

#[test]
fn lines_over_the_byte_bound_are_refused_and_the_rest_fit_the_effect() {
    let mut t = Terminal::new();
    let mut os = Recorder::default();
    // 63 four-byte characters: 252 bytes, under 200 characters.
    let wide = "\u{20000}".repeat(63);
    assert!(wide.chars().count() <= MAX_LINE && wide.len() > MAX_SENT_BYTES);
    assert_eq!(
        t.run(&mut os, &wide, Source::Typed),
        Submitted::Refused(Refusal::TooLong)
    );
    // At the bound, the worst record still fits.
    let worst = "\"".repeat(MAX_LINE);
    assert!(shell_line_args(&worst).len() <= MAX_ARGS);
    assert!(matches!(
        t.run(&mut os, &worst, Source::Typed),
        Submitted::Send(_)
    ));
    assert!(os.asked[0].2.len() <= MAX_ARGS);
}

#[test]
fn the_ai_types_a_line_and_a_person_runs_it() {
    let mut t = Terminal::new();
    let mut os = Recorder::default();
    // terminal_run only types: nothing goes to the shell.
    assert_eq!(t.act_run("  mem  "), Ok(comp_terminal::TYPED.to_string()));
    assert!(os.asked.is_empty());
    assert_eq!(t.spans().last().unwrap().text, comp_terminal::STAGED);
    assert_eq!(t.phase(), Phase::Ready);
    // A person presses Enter on it as it is: the row is the AI's.
    assert!(matches!(
        t.run(&mut os, "mem", Source::Typed),
        Submitted::Send(_)
    ));
    let echo = t.spans().last().unwrap();
    assert!(echo.by_ai && echo.text == "mem");
    let row = echo.seq;
    let ticket = os.last_ticket().unwrap();
    t.done(&mut os, ticket, Ok("ok free 12 MiB".into()));
    let read = t.act_read(row).unwrap();
    let rows: Vec<&str> = read.lines().collect();
    assert_eq!(rows[0], format!("next {} ready", t.next_seq()));
    assert_eq!(rows[1], format!("{row} input kai> mem"));
    assert_eq!(rows[2], format!("{} reply free 12 MiB", row + 1));
    assert_eq!(t.act_clear(), Ok("cleared".to_string()));
    assert!(t.spans().is_empty());
}

#[test]
fn a_line_a_person_changed_or_typed_is_theirs() {
    let mut t = Terminal::new();
    let mut os = Recorder::default();
    t.act_run("mem").unwrap();
    // Changed before Enter: the person's own line.
    t.run(&mut os, "mem -v", Source::Typed);
    assert!(!t.spans().last().unwrap().by_ai);
    let ticket = os.last_ticket().unwrap();
    t.done(&mut os, ticket, Ok("ok".into()));
    // The AI's staging is spent: the same text typed later is the person's.
    t.run(&mut os, "mem", Source::Typed);
    assert!(!t.spans().last().unwrap().by_ai);
}

#[test]
fn the_ai_cannot_type_a_line_the_terminal_would_refuse() {
    let mut t = Terminal::new();
    let mut os = Recorder::default();
    for (line, why) in [
        (" ", "there is nothing to type"),
        ("a\nb", "a line holds no control characters or newlines"),
        (
            "answer 3 yes",
            "answers are given only on a question's own card",
        ),
    ] {
        assert_eq!(t.act_run(line), Err(why.to_string()), "{line:?}");
    }
    t.run(&mut os, "slow", Source::Typed);
    assert_eq!(
        t.act_run("mem"),
        Err("wait: the last line is still running".to_string())
    );
    // Nothing was staged by any of them.
    let ticket = os.last_ticket().unwrap();
    t.done(&mut os, ticket, Ok("ok".into()));
    t.run(&mut os, "mem", Source::Typed);
    assert!(!t.spans().last().unwrap().by_ai);
}

#[test]
fn a_one_line_answer_with_escapes_shows_as_the_rows_it_was() {
    let (mut t, mut os, ticket) = started("help");
    t.done(&mut os, ticket, Ok(r"ok first\nsecond \\ third".into()));
    let rows: Vec<&str> = t
        .spans()
        .iter()
        .rev()
        .take(2)
        .map(|s| s.text.as_str())
        .collect();
    assert_eq!(rows, ["second \\ third", "first"]);
    assert_eq!(t.phase(), Phase::Ready);
}

#[test]
fn a_recorder_numbers_its_effects_from_one() {
    let mut os = Recorder::default();
    assert_eq!(os.request(SHELL_LINE, String::new()), 1);
    assert_eq!(os.request(SHELL_LINE, String::new()), 2);
}
