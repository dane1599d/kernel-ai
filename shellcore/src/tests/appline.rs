//! `shell_line` (P4 task 4): the Terminal's lines run in a terminal of the app's
//! own, cannot ask, and come back as one escaped line. A question goes on the
//! desktop's card, and `shell_card` brings its answer back to the Terminal.

use super::act::with_calls;
use super::*;
use crate::appline::TERMINAL_APP;
use alloc::vec;

const RR: &str = r#"{"name":"set_policy","arguments":{"name":"round-robin"}}"#;
const STOP: &str = r#"{"name":"process_stop","arguments":{"name":"notes"}}"#;

fn quote(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The effect line the Terminal queues for `text`.
fn effect(ticket: u32, text: &str) -> String {
    let args = format!("{{line: {}}}", quote(text));
    format!(
        "effect {ticket} call({{name: \"shell_line\", args: {}}})",
        quote(&args)
    )
}

/// What the terminal app gets for `text` typed by a person.
fn typed(shell: &mut Shell, text: &str) -> Result<String, String> {
    let done = shell.run_effects_from(TERMINAL_APP, &effect(1, text), true);
    assert_eq!(done.len(), 1);
    done[0].outcome.clone()
}

/// The effect line the Terminal queues to ask what came of question `id`.
fn card(ticket: u32, id: u64) -> String {
    let args = format!("{{id: {id}}}");
    format!(
        "effect {ticket} call({{name: \"shell_card\", args: {}}})",
        quote(&args)
    )
}

/// `app`'s `shell_card` with `ticket` for question `id`, not by a person: the
/// Terminal queues it when its line's answer is a question.
fn ask_card(shell: &mut Shell, app: &str, ticket: u32, id: u64) -> Vec<EffectDone> {
    shell.run_effects_from(app, &card(ticket, id), false)
}

/// A one-line answer read back as the wire reply it was (the Terminal's
/// `unescape`).
fn unescaped(line: &str) -> String {
    let mut out = String::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// A Terminal line that asks first: the card's menu it was told, decoded.
fn asked(shell: &mut Shell, text: &str) -> termcore::menu::Menu {
    let said = typed(shell, text).unwrap();
    termcore::menu::Menu::decode(&unescaped(&said)).unwrap()
}

/// The card's answer `word` to question `id`, as the kernel's terminal sends
/// it once it took the card.
fn on_the_card(host: &FakeHost, shell: &mut Shell, id: u64, word: &str) -> Reply {
    let card = shell.eval_console(termcore::menu::TAKE_CARD);
    let menu = termcore::menu::Menu::decode(&card.to_wire()).unwrap();
    assert_eq!(menu.id, id);
    let once = host.mint(menu.id, menu.call());
    shell.eval_console(&format!("answer {id} {word} once={once}"))
}

/// The results the shell gives back to the Terminal outside a take.
fn given_back(shell: &mut Shell) -> Vec<EffectDone> {
    let batches = shell.take_effect_results();
    assert!(
        batches.iter().all(|(app, _)| app == TERMINAL_APP),
        "{batches:?}"
    );
    batches.into_iter().flat_map(|(_, done)| done).collect()
}

#[test]
fn a_line_runs_as_a_terminal_line_and_comes_back_as_its_wire_reply() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    assert_eq!(typed(&mut shell, "1 + 2"), Ok(String::from("ok 3")));
    assert_eq!(typed(&mut shell, "("), Ok(String::from("more")));
}

#[test]
fn the_apps_open_entry_is_its_own_and_the_consoles_is_untouched() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    // The console opens an entry; the app has its own.
    let opened = shell.eval("let a = [");
    assert!(opened.more);
    assert_eq!(typed(&mut shell, "fn f() {"), Ok(String::from("more")));
    // The console's next line still continues the console's entry.
    let closed = shell.eval("1]");
    assert!(closed.ok && !closed.more, "{closed:?}");
    // And the app's continues the app's. Rhai's functions are the shell's own,
    // shared by every terminal, as its variables are.
    assert_eq!(typed(&mut shell, "7 }"), Ok(String::from("ok ")));
    assert_eq!(typed(&mut shell, "f()"), Ok(String::from("ok 7")));
    assert!(shell.eval("f()").ok);
}

#[test]
fn a_line_that_asks_waits_for_the_card_and_only_its_allow_runs_it() {
    let (host, mut shell) = with_calls(vec![Ok(llmproto::act::parse(STOP).unwrap())]);
    *host.procs.borrow_mut() = vec![String::from("notes")];
    // The window is told the card's menu, to show as its question.
    let asked = asked(&mut shell, "stop notes");
    assert_eq!(asked.call(), "process_stop(\"notes\")");
    assert_eq!(*host.procs.borrow(), vec![String::from("notes")]);
    // It asks what came of it: held while the card has not answered.
    assert_eq!(ask_card(&mut shell, TERMINAL_APP, 2, asked.id), []);
    // The console's own line answers nothing.
    let later = shell.eval("y");
    assert!(!later.text.contains("script"), "{later:?}");
    assert_eq!(*host.procs.borrow(), vec![String::from("notes")]);
    // The card: code's call, its highlight on Deny, and its Allow's ONCE.
    assert!(shell.card_waiting());
    let card = shell.eval_console(termcore::menu::TAKE_CARD);
    assert!(card.to_wire().contains(" added\n"), "{card:?}");
    let menu = termcore::menu::Menu::decode(&card.to_wire()).unwrap();
    assert_eq!(menu.call(), "process_stop(\"notes\")");
    let once = host.mint(menu.id, menu.call());
    let allowed = shell.eval_console(&format!("answer {} allow once={once}", menu.id));
    assert!(allowed.ok, "{allowed:?}");
    assert!(host.procs.borrow().is_empty());
    // What it came to goes to the Terminal (#922's walk), not the panel's bar.
    assert_eq!(shell.take_card_result(), None);
    let back = given_back(&mut shell);
    assert_eq!(back.len(), 1, "{back:?}");
    assert_eq!(back[0].ticket, 2);
    let told = back[0].outcome.clone().unwrap();
    assert!(told.starts_with("ok ") && !told.contains("undo"), "{told}");
}

#[test]
fn an_answer_the_card_gave_before_the_terminal_asked_waits_for_its_ask() {
    let (host, mut shell) = with_calls(vec![Ok(llmproto::act::parse(STOP).unwrap())]);
    *host.procs.borrow_mut() = vec![String::from("notes")];
    let id = asked(&mut shell, "stop notes").id;
    on_the_card(&host, &mut shell, id, "deny");
    assert_eq!(*host.procs.borrow(), vec![String::from("notes")]);
    assert_eq!(given_back(&mut shell), []);
    // Don't is no failure: a reply row, not an error.
    let told = ask_card(&mut shell, TERMINAL_APP, 2, id);
    let dont = Ok(String::from("ok Not run: you said Don't."));
    assert_eq!(
        told,
        [EffectDone {
            ticket: 2,
            outcome: dont
        }]
    );
    // Given once: a second ask finds no question.
    let again = ask_card(&mut shell, TERMINAL_APP, 3, id);
    assert!(matches!(&again[0].outcome, Err(why) if why.contains("waits for no card")));
}

#[test]
fn the_window_may_decline_its_question_only_while_it_waits_for_the_card() {
    let (host, mut shell) = with_calls(vec![
        Ok(llmproto::act::parse(STOP).unwrap()),
        Ok(llmproto::act::parse(STOP).unwrap()),
    ]);
    *host.procs.borrow_mut() = vec![String::from("notes")];
    let deny = |id| termcore::menu::answer_line(id, termcore::menu::Answer::Deny);
    // Waiting: Don't lets it go, and the window's ask is told so.
    let first = asked(&mut shell, "stop notes").id;
    assert_eq!(ask_card(&mut shell, TERMINAL_APP, 2, first), []);
    assert_eq!(typed(&mut shell, &deny(first)), Ok(String::from("ok ")));
    assert!(!shell.card_waiting());
    let dont = Ok(String::from("ok Not run: you said Don't."));
    assert_eq!(
        given_back(&mut shell),
        [EffectDone {
            ticket: 2,
            outcome: dont
        }]
    );
    // Only Don't: an Allow typed in the window is still refused.
    let second = asked(&mut shell, "stop notes").id;
    let allow = termcore::menu::answer_line(second, termcore::menu::Answer::Allow);
    assert_eq!(
        typed(&mut shell, &allow),
        Ok(String::from("err answers go on the question's own prompt"))
    );
    // On the card, only the card answers; the window is told where.
    assert_eq!(ask_card(&mut shell, TERMINAL_APP, 3, second), []);
    let card = shell.eval_console(termcore::menu::TAKE_CARD);
    let menu = termcore::menu::Menu::decode(&card.to_wire()).unwrap();
    assert_eq!(menu.id, second);
    assert_eq!(
        typed(&mut shell, &deny(second)),
        Ok(String::from(
            "err it is on the card now: answer Run or Don't there"
        ))
    );
    assert_eq!(given_back(&mut shell), []);
    let once = host.mint(menu.id, menu.call());
    let allowed = shell.eval_console(&format!("answer {second} allow once={once}"));
    assert!(allowed.ok, "{allowed:?}");
    assert!(host.procs.borrow().is_empty());
    let back = given_back(&mut shell);
    assert_eq!(back.len(), 1, "{back:?}");
    assert_eq!(back[0].ticket, 3);
    assert!(back[0].outcome.as_ref().is_ok_and(|t| t.starts_with("ok ")));
}

#[test]
fn only_the_terminal_asks_and_only_of_its_own_waiting_question() {
    let (host, mut shell) = with_calls(vec![
        Ok(llmproto::act::parse(STOP).unwrap()),
        Ok(llmproto::act::parse(STOP).unwrap()),
    ]);
    *host.procs.borrow_mut() = vec![String::from("notes")];
    let id = asked(&mut shell, "stop notes").id;
    let other = ask_card(&mut shell, "files", 2, id);
    assert!(
        matches!(&other[0].outcome, Err(why) if why.contains("files may not ask")),
        "{other:?}"
    );
    let unknown = ask_card(&mut shell, TERMINAL_APP, 3, id + 50);
    assert!(matches!(&unknown[0].outcome, Err(why) if why.contains("waits for no card")));
    let args = quote("{line: \"x\"}");
    let line = format!("effect 4 call({{name: \"shell_card\", args: {args}}})");
    let bad = shell.run_effects_from(TERMINAL_APP, &line, false);
    assert!(
        matches!(&bad[0].outcome, Err(why) if why.contains("shell_card")),
        "{bad:?}"
    );
    // The panel's question is the panel's: the Terminal hears nothing of it.
    let shown = shell.eval_panel("stop notes");
    assert!(matches!(shown, PanelResult::Done(_)), "{shown:?}");
    let panels = shell.desk_asks.last().map(|w| w.pending.id).unwrap();
    assert_ne!(panels, id);
    let theirs = ask_card(&mut shell, TERMINAL_APP, 5, panels);
    assert!(matches!(&theirs[0].outcome, Err(why) if why.contains("waits for no card")));
}

/// The console's own line let the card's proposal go: the card answers
/// nothing now, so the window's Don't ends its wait.
#[test]
fn a_decline_after_the_console_let_the_cards_proposal_go_ends_the_wait() {
    let (host, mut shell) = with_calls(vec![Ok(llmproto::act::parse(STOP).unwrap())]);
    *host.procs.borrow_mut() = vec![String::from("notes")];
    let id = asked(&mut shell, "stop notes").id;
    assert_eq!(ask_card(&mut shell, TERMINAL_APP, 2, id), []);
    let card = shell.eval_console(termcore::menu::TAKE_CARD);
    assert!(card.confirm.is_some(), "{card:?}");
    assert!(shell.eval_console("1 + 1").ok);
    let deny = termcore::menu::answer_line(id, termcore::menu::Answer::Deny);
    assert_eq!(typed(&mut shell, &deny), Ok(String::from("ok ")));
    let dont = Ok(String::from("ok Not run: you said Don't."));
    assert_eq!(
        given_back(&mut shell),
        [EffectDone {
            ticket: 2,
            outcome: dont
        }]
    );
    assert_eq!(*host.procs.borrow(), vec![String::from("notes")]);
}

#[test]
fn a_question_that_lapses_before_the_card_tells_the_window() {
    let (host, mut shell) = with_calls(vec![Ok(llmproto::act::parse(STOP).unwrap())]);
    *host.procs.borrow_mut() = vec![String::from("notes")];
    let id = asked(&mut shell, "stop notes").id;
    assert_eq!(ask_card(&mut shell, TERMINAL_APP, 2, id), []);
    let later = host.clock.get() + crate::actions::CONFIRM_EXPIRY_NS + 1;
    host.clock.set(later);
    shell.eval_console(termcore::menu::TAKE_CARD);
    assert!(!shell.card_waiting());
    let lapsed = Ok(String::from(
        "ok Not run: it waited more than two minutes for the card.",
    ));
    assert_eq!(
        given_back(&mut shell),
        [EffectDone {
            ticket: 2,
            outcome: lapsed
        }]
    );
    assert_eq!(*host.procs.borrow(), vec![String::from("notes")]);
}

#[test]
fn what_runs_at_once_is_logged_and_undone_like_any_line() {
    let (host, mut shell) = with_calls(vec![Ok(llmproto::act::parse(RR).unwrap())]);
    let said = typed(&mut shell, "switch to round robin").unwrap();
    assert!(said.starts_with("ok "), "{said}");
    assert_eq!(host.policy.get(), Some(policy::ROUND_ROBIN));
    let undone = typed(&mut shell, "undo").unwrap();
    assert!(undone.starts_with("ok "), "{undone}");
    assert_eq!(host.policy.get(), Some(policy::INTERACTIVE));
}

#[test]
fn only_the_terminal_may_run_lines_and_never_an_answer() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    for app in ["files", "notes-window", "settings", ""] {
        let done = shell.run_effects_from(app, &effect(1, "1 + 1"), true);
        assert!(
            matches!(&done[0].outcome, Err(why) if why.contains("may not run shell lines")),
            "{app}: {done:?}"
        );
    }
    assert_eq!(
        typed(&mut shell, "answer 1 yes"),
        Ok(String::from("err answers go on the question's own prompt"))
    );
}

#[test]
fn only_a_persons_key_or_click_may_type_a_line() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    // Queued by an action call (the AI's), a reply or init: the plain entry.
    let done = shell.run_effects(TERMINAL_APP, &effect(1, "policy rr"));
    assert!(
        matches!(&done[0].outcome, Err(why) if why.contains("only for a key or click")),
        "{done:?}"
    );
    let done = shell.run_effects_from(TERMINAL_APP, &effect(2, "policy rr"), false);
    assert!(done[0].outcome.is_err());
    assert_eq!(host.policy.get(), None);
    // The refusal is audited like any refused effect.
    assert!(
        host.audit
            .borrow()
            .iter()
            .any(|l| l.starts_with("terminal-window's effect 1 refused"))
    );
    // The same line from a person runs.
    let done = shell.run_effects_from(TERMINAL_APP, &effect(3, "policy rr"), true);
    assert!(done[0].outcome.is_ok(), "{done:?}");
}

#[test]
fn bad_arguments_come_back_as_errors() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    for args in [
        "{}",
        "{line: 3}",
        "{line: \\\"a\\\", extra: \\\"b\\\"}",
        "x",
    ] {
        let line = format!("effect 1 call({{name: \"shell_line\", args: \"{args}\"}})");
        let done = shell.run_effects_from(TERMINAL_APP, &line, true);
        assert!(done[0].outcome.is_err(), "{args}: {done:?}");
    }
}

#[test]
fn the_answer_is_one_line_escaped_and_cut_at_a_character() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    assert_eq!(
        typed(&mut shell, r#""a\nb\\c""#),
        Ok(String::from(r"ok a\nb\\c"))
    );
    let long = typed(
        &mut shell,
        r#"let s = ""; for i in 0..1500 { s += "é"; } s"#,
    )
    .unwrap();
    assert!(
        long.len() <= crate::appline::MAX_ANSWER + '…'.len_utf8(),
        "{} bytes",
        long.len()
    );
    assert!(long.ends_with('…') && long.starts_with("ok é"), "{long}");
    assert!(!long.contains('\n'));
    // A cut never lands inside an escape: every backslash in what is left pairs.
    let tail = typed(
        &mut shell,
        r#"let s = ""; for i in 0..1500 { s += "\n"; } s"#,
    )
    .unwrap();
    assert!(
        tail.len() <= crate::appline::MAX_ANSWER + '…'.len_utf8(),
        "{tail}"
    );
    let body = tail.strip_suffix('…').unwrap().strip_prefix("ok ").unwrap();
    assert!(!body.contains('\n') && body.matches('\\').count() == body.matches("\\n").count());
}

#[test]
fn a_tab_in_the_answer_is_escaped_too() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    assert_eq!(typed(&mut shell, r#""a\tb""#), Ok(String::from(r"ok a\tb")));
}

#[test]
fn the_answer_may_be_as_long_as_the_transport_carries_of_a_persons_result() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let said = typed(
        &mut shell,
        r#"let s = ""; for i in 0..3000 { s += "x"; } s"#,
    )
    .unwrap();
    assert_eq!(said.len(), crate::appline::MAX_ANSWER + '…'.len_utf8());
    assert!(said.ends_with('…'));
}

#[test]
fn every_line_it_runs_is_audited() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    typed(&mut shell, "1 + 1").unwrap();
    assert!(
        host.audit
            .borrow()
            .iter()
            .any(|l| l == r#"terminal-window ran the line "1 + 1""#),
        "{:?}",
        host.audit.borrow()
    );
}

fn c_shell(host: &Rc<FakeHost>) -> Shell {
    let service = ccserve::Service::new(
        ccserve::CC_WASM,
        &ccserve::XCC_HEADERS,
        ccserve::rows::Rows::os(),
        &super::cmode::CcClock,
    )
    .expect("cc starts");
    *host.cc.borrow_mut() = Some(service);
    let mut shell = shell(host);
    let entered = shell.eval(":c");
    assert!(entered.ok && shell.at_c(), "{entered:?}");
    shell
}

/// A line from the Terminal app runs at kai> while the console is at c>, never
/// reaches cc, and leaves the console's open entry and session.
#[test]
fn a_terminal_apps_line_runs_at_kai_and_leaves_c_alone() {
    let host = FakeHost::new();
    let mut shell = c_shell(&host);
    assert!(shell.eval("I64 Sq(I64 x) { return x * x; }").ok);
    assert!(shell.eval("I64 F(I64 x) {").more);
    let sent = host.cc_sent.borrow().len();
    assert_eq!(typed(&mut shell, "1 + 2"), Ok(String::from("ok 3")));
    for line in [":c 6 * 7", ":c", ":rhai"] {
        assert_eq!(
            typed(&mut shell, line),
            Ok(String::from(
                "err C goes in the console: press Ctrl+Alt+F12"
            )),
            "{line}"
        );
    }
    assert_eq!(host.cc_sent.borrow().len(), sent);
    assert!(shell.at_c());
    assert!(shell.eval("  return Sq(x); }").ok);
    assert_eq!(shell.eval("F(3)").text, "= 9");
}

/// While a C entry waits on its question an app line is refused, and the
/// console's answer still finishes the entry.
#[test]
fn the_consoles_answer_finishes_a_c_entry_after_an_app_line() {
    let host = FakeHost::new();
    *host.procs.borrow_mut() = vec![String::from("notes")];
    let mut shell = c_shell(&host);
    let id = shell
        .eval("process_stop(\"notes\")")
        .confirm
        .expect("a stop asks");
    let said = typed(&mut shell, "1 + 2").unwrap();
    assert!(said.starts_with("err a C entry waits"), "{said}");
    let once = host.mint(id, "process_stop(\"notes\")");
    let allowed = shell.eval(&format!("answer {id} allow once={once}"));
    assert!(allowed.text.ends_with("= 0"), "{allowed:?}");
    assert!(host.procs.borrow().is_empty());
}

#[test]
fn a_typed_models_import_is_refused_here_and_a_switch_runs_logged() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    // It has no undo, so it asks: on the card, never in the window, which is
    // told the card's menu.
    let said = typed(&mut shell, "models import").unwrap();
    let menu = termcore::menu::Menu::decode(&unescaped(&said)).unwrap();
    assert_eq!(menu.call(), "model_fetch(\"disk\")", "{said}");
    assert!(
        !host
            .model_requests
            .borrow()
            .contains(&"models import".into())
    );
    let said = typed(&mut shell, "models use qwen3-0.6b").unwrap();
    assert!(
        said.starts_with("ok Done: the next restart uses it"),
        "{said}"
    );
    assert!(shell.eval("log actlog").text.contains("consent=typed"));
    let undone = typed(&mut shell, "undo").unwrap();
    assert!(
        undone.starts_with("ok Undone: the model before"),
        "{undone}"
    );
    assert_eq!(host.models.borrow().store.next, None);
}
