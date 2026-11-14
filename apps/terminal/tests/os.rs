//! The terminal's effect and the AI's three actions, against a recording OS.

use comp_terminal::os::{
    MAX_ARGS, Os, Recorder, SHELL_CARD, SHELL_LINE, SHELL_STOP, shell_line_args,
};
use comp_terminal::span::Kind;
use comp_terminal::state::State;
use comp_terminal::{
    ANSWER_ELSEWHERE, ASKS_AGAIN, Ask, CANCEL_ENTRY, MAX_LINE, MAX_SENT_BYTES, Phase, Refusal,
    Source, Submitted, Terminal,
};
use termcore::menu::{Menu, Origin};

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

/// #892's re-review: a Stop's own answer is the Terminal's. One the OS could
/// not carry out says why, and the line still runs; one that worked says
/// nothing, since the line's answer tells how it ended.
#[test]
fn a_stop_that_could_not_stop_says_why_and_the_line_runs_on() {
    let (mut t, mut os, line) = started("mem");
    assert!(t.ask_stop(&mut os));
    let stop = os.last_ticket().unwrap();
    assert_eq!(os.asked.last().unwrap().1, SHELL_STOP);
    assert!(t.is_stopping());
    assert!(!t.ask_stop(&mut os), "one Stop until its answer");
    assert_eq!(os.asked.len(), 2);
    assert!(t.done(&mut os, stop, Err("not running yet".into())));
    assert!(!t.is_stopping());
    assert_eq!(t.spans().last().unwrap().text, "Stop: not running yet");
    assert_eq!(t.phase(), Phase::Waiting, "the line still runs");
    // It may be pressed again, and the line's answer still ends it.
    assert!(t.ask_stop(&mut os));
    assert!(t.done(&mut os, line, Ok("ok free 12 MiB".into())));
    assert!(!t.is_stopping(), "the line's answer ends the Stop too");
    assert_eq!(t.phase(), Phase::Ready);
}

#[test]
fn a_stop_that_worked_leaves_the_lines_answer_to_say_so() {
    let (mut t, mut os, line) = started("mem");
    assert!(t.ask_stop(&mut os));
    let stop = os.last_ticket().unwrap();
    let rows = t.spans().len();
    assert!(t.done(&mut os, stop, Ok("stopped".into())));
    assert_eq!(t.spans().len(), rows, "nothing said");
    assert!(t.done(&mut os, line, Ok("ok stopped".into())));
    assert_eq!(t.spans().last().unwrap().text, "stopped");
    // A Stop answered late, after the line's own answer, is spent.
    assert!(!t.done(&mut os, stop, Err("not running yet".into())));
    // And there is nothing to stop at the prompt.
    assert!(!t.ask_stop(&mut os));
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

/// The shell's answer to a line whose question waits for the desktop's card:
/// the card's menu, as one line.
fn menu_line(id: u64) -> String {
    let call = "fs_create(\"walk-term.txt\")";
    let wire = Menu::encode(id, Origin::Added, &format!("Run {call}?"), "", "", call);
    wire.replace('\\', "\\\\").replace('\n', "\\n")
}

/// #922: a line that asked first heard nothing of the card's answer.
#[test]
fn a_question_on_the_card_is_shown_and_its_answer_comes_back_to_the_rows() {
    let (mut t, mut os, ticket) = started("save a note for the terminal");
    assert!(t.done(&mut os, ticket, Ok(menu_line(5))));
    assert_eq!(t.phase(), Phase::Asking(Ask { id: 5, menu: true }));
    assert_eq!(t.spans().last().unwrap().text, ANSWER_ELSEWHERE);
    // It asks what came of the question, with no line typed.
    let (card, name, args) = os.asked.last().cloned().unwrap();
    assert_eq!((name, args.as_str()), (SHELL_CARD, "{id: 5}"));
    assert_eq!(os.lines().len(), 1);
    // No line is taken meanwhile, and nothing else ends the wait.
    assert_eq!(
        t.run(&mut os, "mem", Source::Typed),
        Submitted::Refused(Refusal::Asking)
    );
    assert!(!t.done(&mut os, ticket, Ok("ok forged".into())));
    // The card's answer: a reply row, and the prompt back.
    assert!(t.done(&mut os, card, Ok("ok file walk-term.txt created".into())));
    assert_eq!(t.phase(), Phase::Ready);
    let last = t.spans().last().unwrap();
    assert_eq!(
        (last.kind, last.text.as_str()),
        (Kind::Reply, "file walk-term.txt created")
    );
    // Spent: the same ticket again is not taken.
    assert!(!t.done(&mut os, card, Ok("ok forged".into())));
}

/// #928's review: a terminal restored at a question (a nap's wake, a reload)
/// has no ask in flight, so it asks again what came of it, and hears it.
#[test]
fn a_terminal_restored_at_a_question_asks_again_and_hears_the_answer() {
    let (mut t, mut os, ticket) = started("save a note");
    t.done(&mut os, ticket, Ok(menu_line(5)));
    let old = os.last_ticket().unwrap();
    let bytes = minicbor::to_vec(t.to_state()).unwrap();
    let mut back = Terminal::from_state(minicbor::decode::<State>(&bytes).unwrap());
    assert_eq!(back.phase(), Phase::Asking(Ask { id: 5, menu: true }));
    // The old ask's ticket was not saved: its result is not taken.
    assert!(!back.done(&mut os, old, Ok("ok file walk-term.txt created".into())));
    assert!(back.resume(&mut os));
    assert_eq!(back.spans().last().unwrap().text, ASKS_AGAIN);
    let (card, name, args) = os.asked.last().cloned().unwrap();
    assert_eq!((name, args.as_str()), (SHELL_CARD, "{id: 5}"));
    // Told it started again, it does not ask twice.
    assert!(!back.resume(&mut os));
    assert!(back.done(&mut os, card, Ok("ok file walk-term.txt created".into())));
    assert_eq!(back.phase(), Phase::Ready);
    assert_eq!(
        back.spans().last().unwrap().text,
        "file walk-term.txt created"
    );
    // At a prompt, or at a script's question, there is nothing to ask.
    assert!(!back.resume(&mut os));
    let mut confirm = Terminal::new();
    confirm.reply("confirm 3 script 3: ...", None);
    assert!(!confirm.resume(&mut os));
}

#[test]
fn a_run_that_failed_on_the_card_is_an_error_row() {
    let (mut t, mut os, ticket) = started("save a note");
    t.done(&mut os, ticket, Ok(menu_line(5)));
    let card = os.last_ticket().unwrap();
    t.done(&mut os, card, Ok("err Something went wrong.".into()));
    assert_eq!(t.spans().last().unwrap().kind, Kind::Error);
    assert_eq!(t.phase(), Phase::Ready);
}

#[test]
fn a_declined_question_hears_its_answer_and_the_decline_keeps_its_own_wait() {
    let (mut t, mut os, ticket) = started("save a note");
    t.done(&mut os, ticket, Ok(menu_line(5)));
    let card = os.last_ticket().unwrap();
    // Cancel declines it, as a line of its own.
    assert!(t.stop(&mut os));
    assert_eq!(os.lines().last().unwrap(), r#"{line: "answer 5 deny"}"#);
    let decline = os.last_ticket().unwrap();
    // What the card came to may come first: the decline still waits.
    assert!(t.done(&mut os, card, Ok("ok Not run: you said Don't.".into())));
    assert_eq!(t.phase(), Phase::Waiting);
    assert_eq!(t.spans().last().unwrap().text, "Not run: you said Don't.");
    assert!(t.done(&mut os, decline, Ok("ok ".into())));
    assert_eq!(t.phase(), Phase::Ready);
}

#[test]
fn one_questions_answer_does_not_end_the_wait_for_the_next() {
    let (mut t, mut os, ticket) = started("save a note");
    t.done(&mut os, ticket, Ok(menu_line(5)));
    let first = os.last_ticket().unwrap();
    // On the card already: the decline is refused, and a new line asks too.
    t.stop(&mut os);
    let decline = os.last_ticket().unwrap();
    let on_card = "err it is on the card now: answer Run or Don't there";
    t.done(&mut os, decline, Ok(on_card.into()));
    assert_eq!(t.phase(), Phase::Ready);
    t.run(&mut os, "save another", Source::Typed);
    let line = os.last_ticket().unwrap();
    t.done(&mut os, line, Ok(menu_line(6)));
    let second = os.last_ticket().unwrap();
    assert_ne!(first, second);
    // The first question's answer shows, and the second still waits.
    assert!(t.done(&mut os, first, Ok("ok file walk-term.txt created".into())));
    assert_eq!(t.phase(), Phase::Asking(Ask { id: 6, menu: true }));
    assert!(t.done(&mut os, second, Ok("ok Not run: you said Don't.".into())));
    assert_eq!(t.phase(), Phase::Ready);
}

#[test]
fn an_answer_the_os_could_not_give_is_an_error_row_and_ends_the_wait() {
    let (mut t, mut os, ticket) = started("save a note");
    t.done(&mut os, ticket, Ok(menu_line(5)));
    let card = os.last_ticket().unwrap();
    t.done(&mut os, card, Err("the shell restarted".into()));
    let last = t.spans().last().unwrap();
    assert_eq!(last.kind, Kind::Error);
    assert!(last.text.ends_with("the shell restarted"), "{}", last.text);
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
    // terminal_window_run only types: nothing goes to the shell.
    assert_eq!(t.act_run("  mem  "), Ok(comp_terminal::TYPED.to_string()));
    assert!(os.asked.is_empty());
    assert_eq!(
        t.spans().last().unwrap().text,
        format!("{}: mem", comp_terminal::STAGED)
    );
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

#[test]
fn a_line_the_ai_types_shows_whole_to_the_person_who_runs_it() {
    let mut t = Terminal::new();
    let line = "echo 1";
    t.act_run(line).unwrap();
    // The notice carries all of it, wherever the input field cuts it.
    assert!(t.spans().last().unwrap().text.ends_with(line));
    // So a line that would hide its end is not typed: a run of spaces, a
    // character outside printable ASCII, or more than the notice shows.
    let hidden_tail = format!("ls{}; rm x", " ".repeat(120));
    for bad in [
        hidden_tail.as_str(),
        "say \u{202E}hi",
        "caf\u{e9}",
        &"x".repeat(comp_terminal::MAX_STAGED + 1),
        "a  b",
    ] {
        assert_eq!(
            t.act_run(bad),
            Err(comp_terminal::Refusal::Hidden.message().to_string()),
            "{bad:?}"
        );
    }
    // At the limit, with single spaces, it is typed and shown whole.
    let longest = ["w"; comp_terminal::MAX_STAGED / 2].join(" ");
    assert!(longest.len() <= comp_terminal::MAX_STAGED);
    t.act_run(&longest).unwrap();
    assert!(t.spans().last().unwrap().text.ends_with(&longest));
}

#[test]
fn the_longest_effect_line_fits_what_desktop_keeps() {
    // The whole `effect <ticket> call(...)` line, with the arguments quoted a
    // second time inside it, for a line of nothing but quotes (each takes four
    // bytes by then): deskcore refuses a line over 1024 bytes.
    let worst = "\"".repeat(comp_terminal::MAX_LINE);
    let args = comp_terminal::os::shell_line_args(&worst);
    let line = format!(
        "effect {} call({{name: {}, args: {}}})",
        u32::MAX,
        comp_terminal::os::quote(comp_terminal::os::SHELL_LINE),
        comp_terminal::os::quote(&args)
    );
    assert!(line.len() <= 1024, "{} bytes", line.len());
}
