//! A Terminal line that asks first (#922): the window is told the card's menu,
//! asks what came of it with `shell_card`, and hears the card's answer, a
//! decline it gave while the question waited, a lapse, or a drop.

use super::act::with_calls;
use super::appline::{quote, typed, unescaped};
use super::*;
use crate::appline::TERMINAL_APP;
use crate::deskcard::{KEPT_NS, MAX_KEPT};
use alloc::vec;

const STOP: &str = r#"{"name":"process_stop","arguments":{"name":"notes"}}"#;
const DONT: &str = "ok Not run: you said Don't.";
const NOTHING_WAITS: &str =
    "nothing waits for the card under that question: if the shell restarted, ask again";

/// A shell whose model proposes `n` stops of `notes`, which runs.
fn stops(n: usize) -> (Rc<FakeHost>, Shell) {
    let calls = (0..n)
        .map(|_| Ok(llmproto::act::parse(STOP).unwrap()))
        .collect();
    let (host, shell) = with_calls(calls);
    *host.procs.borrow_mut() = vec![String::from("notes")];
    (host, shell)
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

/// What a `shell_card` answered at once: its one result.
fn answered(done: &[EffectDone]) -> Result<String, String> {
    assert_eq!(done.len(), 1, "{done:?}");
    done[0].outcome.clone()
}

/// A Terminal line that asks first: the card's menu it was told, decoded.
fn asked(shell: &mut Shell, text: &str) -> termcore::menu::Menu {
    let said = typed(shell, text).unwrap();
    termcore::menu::Menu::decode(&unescaped(&said)).unwrap()
}

/// The kernel's terminal takes the card: question `id`'s menu.
fn take(shell: &mut Shell, id: u64) -> termcore::menu::Menu {
    let card = shell.eval_console(termcore::menu::TAKE_CARD);
    let menu = termcore::menu::Menu::decode(&card.to_wire()).unwrap();
    assert_eq!(menu.id, id);
    menu
}

/// The card's answer `word` to `menu`, with the ONCE an Allow mints.
fn answer(host: &FakeHost, shell: &mut Shell, menu: &termcore::menu::Menu, word: &str) -> Reply {
    let once = host.mint(menu.id, menu.call());
    shell.eval_console(&format!("answer {} {word} once={once}", menu.id))
}

/// The results of held `shell_card`s the shell gives back, all the Terminal's.
fn given_back(shell: &mut Shell) -> Vec<EffectDone> {
    assert!(shell.take_effect_results().is_empty());
    let batches = shell.take_card_answers();
    assert!(
        batches.iter().all(|(app, _)| app == TERMINAL_APP),
        "{batches:?}"
    );
    batches.into_iter().flat_map(|(_, done)| done).collect()
}

fn ok(text: &str) -> Result<String, String> {
    Ok(String::from(text))
}

#[test]
fn a_line_that_asks_waits_for_the_card_and_only_its_allow_runs_it() {
    let (host, mut shell) = stops(1);
    // The window is told the card's menu, to show as its question.
    let asked = asked(&mut shell, "stop notes");
    assert_eq!(asked.call(), "process_stop(\"notes\")");
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
    let allowed = answer(&host, &mut shell, &menu, "allow");
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

/// `shell_card` with `ticket` for question `id`, whose answer came already:
/// no result at once, and the answer with the card's answers, as a person's
/// result, never cut as an app's (#928's review LOW).
fn hears(shell: &mut Shell, ticket: u32, id: u64) -> Result<String, String> {
    assert_eq!(ask_card(shell, TERMINAL_APP, ticket, id), []);
    let back = given_back(shell);
    assert_eq!(back.len(), 1, "{back:?}");
    assert_eq!(back[0].ticket, ticket);
    back[0].outcome.clone()
}

/// #928's review: a Terminal that napped or reloaded while its question
/// waited asks again, and hears the answer its old ask was given; so does a
/// second restore's ask, for the answer's ten minutes.
#[test]
fn a_terminal_that_asks_again_after_a_restart_still_hears_the_answer() {
    let (host, mut shell) = stops(1);
    let id = asked(&mut shell, "stop notes").id;
    assert_eq!(ask_card(&mut shell, TERMINAL_APP, 2, id), []);
    let menu = take(&mut shell, id);
    answer(&host, &mut shell, &menu, "deny");
    assert_eq!(given_back(&mut shell)[0].outcome, ok(DONT));
    assert_eq!(hears(&mut shell, 3, id), ok(DONT));
    assert_eq!(hears(&mut shell, 4, id), ok(DONT), "a second restore");
    host.clock.set(host.clock.get() + KEPT_NS);
    let gone = answered(&ask_card(&mut shell, TERMINAL_APP, 5, id));
    assert_eq!(gone, Err(String::from(NOTHING_WAITS)));
}

#[test]
fn an_answer_the_card_gave_before_the_terminal_asked_waits_for_its_ask() {
    let (host, mut shell) = stops(1);
    let id = asked(&mut shell, "stop notes").id;
    let menu = take(&mut shell, id);
    answer(&host, &mut shell, &menu, "deny");
    assert_eq!(*host.procs.borrow(), vec![String::from("notes")]);
    assert_eq!(given_back(&mut shell), []);
    // Don't is no failure: a reply row, not an error.
    assert_eq!(hears(&mut shell, 2, id), ok(DONT));
}

#[test]
fn a_run_that_failed_on_the_card_is_an_error_for_the_window() {
    let (host, mut shell) = stops(1);
    let id = asked(&mut shell, "stop notes").id;
    assert_eq!(ask_card(&mut shell, TERMINAL_APP, 2, id), []);
    let menu = take(&mut shell, id);
    // The process is gone by the time the card allows its stop.
    host.procs.borrow_mut().clear();
    let allowed = answer(&host, &mut shell, &menu, "allow");
    assert!(!allowed.ok, "{allowed:?}");
    let failed = format!("err {}", crate::panel::FAILED);
    assert_eq!(given_back(&mut shell)[0].outcome, Ok(failed));
}

#[test]
fn the_window_may_decline_its_question_only_while_it_waits_for_the_card() {
    let (host, mut shell) = stops(2);
    let deny = |id| termcore::menu::answer_line(id, termcore::menu::Answer::Deny);
    // Waiting: Don't lets it go, and the window's ask is told so.
    let first = asked(&mut shell, "stop notes").id;
    assert_eq!(ask_card(&mut shell, TERMINAL_APP, 2, first), []);
    assert_eq!(typed(&mut shell, &deny(first)), ok("ok "));
    assert!(!shell.card_waiting());
    assert_eq!(given_back(&mut shell)[0].outcome, ok(DONT));
    // Only Don't: an Allow typed in the window is still refused.
    let second = asked(&mut shell, "stop notes").id;
    let allow = termcore::menu::answer_line(second, termcore::menu::Answer::Allow);
    let refused = "err answers go on the question's own prompt";
    assert_eq!(typed(&mut shell, &allow), ok(refused));
    // On the card, only the card answers; the window is told where.
    assert_eq!(ask_card(&mut shell, TERMINAL_APP, 3, second), []);
    let menu = take(&mut shell, second);
    let on_card = "err it is on the card now: answer Run or Don't there";
    assert_eq!(typed(&mut shell, &deny(second)), ok(on_card));
    assert_eq!(given_back(&mut shell), []);
    assert!(answer(&host, &mut shell, &menu, "allow").ok);
    assert!(host.procs.borrow().is_empty());
    let back = given_back(&mut shell);
    assert_eq!(back.len(), 1, "{back:?}");
    assert_eq!(back[0].ticket, 3);
    assert!(back[0].outcome.as_ref().is_ok_and(|t| t.starts_with("ok ")));
    // A decline that matches nothing says so, as after a shell restart.
    let gone = format!("err {NOTHING_WAITS}");
    assert_eq!(typed(&mut shell, &deny(second + 40)), Ok(gone));
}

#[test]
fn only_the_terminal_asks_and_only_of_its_own_waiting_question() {
    let (_host, mut shell) = stops(2);
    let id = asked(&mut shell, "stop notes").id;
    let other = answered(&ask_card(&mut shell, "files", 2, id));
    assert!(
        matches!(&other, Err(why) if why.contains("files may not ask")),
        "{other:?}"
    );
    let unknown = answered(&ask_card(&mut shell, TERMINAL_APP, 3, id + 50));
    assert_eq!(unknown, Err(String::from(NOTHING_WAITS)));
    let args = quote("{line: \"x\"}");
    let line = format!("effect 4 call({{name: \"shell_card\", args: {args}}})");
    let bad = answered(&shell.run_effects_from(TERMINAL_APP, &line, false));
    assert!(
        matches!(&bad, Err(why) if why.contains("shell_card")),
        "{bad:?}"
    );
    // The panel's question is the panel's: the Terminal hears nothing of it.
    let shown = shell.eval_panel("stop notes");
    assert!(matches!(shown, PanelResult::Done(_)), "{shown:?}");
    let panels = shell.desk_asks.last().map(|w| w.pending.id).unwrap();
    assert_ne!(panels, id);
    let theirs = answered(&ask_card(&mut shell, TERMINAL_APP, 5, panels));
    assert_eq!(theirs, Err(String::from(NOTHING_WAITS)));
}

/// #928's review: a console line that lets the card's question go tells the
/// window, rather than leaving its ask held until the cap pushes it out.
#[test]
fn a_console_line_that_lets_the_cards_question_go_tells_the_window() {
    let (host, mut shell) = stops(1);
    let id = asked(&mut shell, "stop notes").id;
    assert_eq!(ask_card(&mut shell, TERMINAL_APP, 2, id), []);
    take(&mut shell, id);
    assert!(shell.eval_console("1 + 1").ok);
    let dropped = "ok Not run: a line typed in the console let the question go.";
    assert_eq!(given_back(&mut shell)[0].outcome, ok(dropped));
    assert_eq!(*host.procs.borrow(), vec![String::from("notes")]);
    // Its Don't, after, finds nothing waiting.
    let deny = termcore::menu::answer_line(id, termcore::menu::Answer::Deny);
    assert_eq!(typed(&mut shell, &deny), Ok(format!("err {NOTHING_WAITS}")));
}

#[test]
fn a_question_that_lapses_before_the_card_tells_the_window() {
    let (host, mut shell) = stops(1);
    let id = asked(&mut shell, "stop notes").id;
    assert_eq!(ask_card(&mut shell, TERMINAL_APP, 2, id), []);
    let later = host.clock.get() + crate::actions::CONFIRM_EXPIRY_NS + 1;
    host.clock.set(later);
    shell.eval_console(termcore::menu::TAKE_CARD);
    assert!(!shell.card_waiting());
    let lapsed = "ok Not run: it waited more than two minutes for the card.";
    assert_eq!(given_back(&mut shell)[0].outcome, ok(lapsed));
    assert_eq!(*host.procs.borrow(), vec![String::from("notes")]);
}

/// #928's review: an answer after the question's two minutes is refused as
/// expired, whoever gave it: the card's countdown, which says Deny then, or
/// a person late at the console (#928's LOW: not "the card timed out"). A
/// late Allow is no failed run either.
#[test]
fn a_question_answered_after_its_two_minutes_says_so_to_the_window_and_the_panel() {
    let (host, mut shell) = stops(3);
    let unanswered = "Not run: it was not answered within two minutes.";
    for (ticket, word) in [(2, "deny"), (3, "allow")] {
        let id = asked(&mut shell, "stop notes").id;
        assert_eq!(ask_card(&mut shell, TERMINAL_APP, ticket, id), []);
        let menu = take(&mut shell, id);
        let later = host.clock.get() + crate::actions::CONFIRM_EXPIRY_NS + 1;
        host.clock.set(later);
        answer(&host, &mut shell, &menu, word);
        let told = format!("ok {unanswered}");
        assert_eq!(given_back(&mut shell)[0].outcome, ok(&told), "{word}");
    }
    // The panel's card the same, as a refusal on its bar.
    shell.eval_panel("stop notes");
    let panels = shell.desk_asks.last().map(|w| w.pending.id).unwrap();
    let menu = take(&mut shell, panels);
    let later = host.clock.get() + crate::actions::CONFIRM_EXPIRY_NS + 1;
    host.clock.set(later);
    answer(&host, &mut shell, &menu, "deny");
    let told = Some((false, String::from(unanswered)));
    assert_eq!(shell.take_card_result(), told);
    assert_eq!(*host.procs.borrow(), vec![String::from("notes")]);
}

/// At most [`MAX_KEPT`] asks held for the card, results waiting to be given,
/// and answers kept: past each the oldest goes, an ask told why.
#[test]
fn the_held_asks_the_results_and_the_kept_answers_are_capped() {
    let (host, mut shell) = stops(MAX_KEPT + 1);
    let id = asked(&mut shell, "stop notes").id;
    let tickets: Vec<u32> = (2..).take(MAX_KEPT + 1).collect();
    for ticket in &tickets {
        assert_eq!(ask_card(&mut shell, TERMINAL_APP, *ticket, id), []);
    }
    // The oldest ask went, told why; not given yet, it waits with the rest.
    let menu = take(&mut shell, id);
    answer(&host, &mut shell, &menu, "deny");
    let back = given_back(&mut shell);
    // Nine results waited for eight places: the oldest, the refusal, went.
    let given: Vec<u32> = back.iter().map(|done| done.ticket).collect();
    assert_eq!(given, tickets[1..]);
    assert!(back.iter().all(|done| done.outcome == ok(DONT)));
    // Answers kept for the windows' asks: eight more push out the first's.
    let mut ids = Vec::new();
    for _ in 0..MAX_KEPT {
        let id = asked(&mut shell, "stop notes").id;
        let menu = take(&mut shell, id);
        answer(&host, &mut shell, &menu, "deny");
        ids.push(id);
    }
    let pushed_out = answered(&ask_card(&mut shell, TERMINAL_APP, 30, id));
    assert_eq!(pushed_out, Err(String::from(NOTHING_WAITS)));
    for (ticket, id) in (31..).zip([ids[0], ids[MAX_KEPT - 1]]) {
        assert_eq!(hears(&mut shell, ticket, id), ok(DONT));
    }
}

/// A send the Terminal window's line asked for, allowed on the card, is
/// held, not done: the window hears it waits 5 s with its own undo, as the
/// console says (#1142's review), and that undo, typed in the window,
/// cancels it: nothing is sent after the hold.
#[test]
fn a_send_allowed_from_the_terminal_window_says_it_is_held_with_its_undo() {
    let call = serde_json::json!({
        "name": "net_send",
        "arguments": {"host": "hooks.example", "path": "/hook", "text": "hello"},
    });
    let (host, mut shell) = with_calls(vec![Ok(llmproto::act::parse(&call.to_string()).unwrap())]);
    host.send_fits.set(true);
    let asked = asked(&mut shell, "send hello to hooks.example/hook");
    assert_eq!(ask_card(&mut shell, TERMINAL_APP, 2, asked.id), []);
    let menu = take(&mut shell, asked.id);
    assert!(answer(&host, &mut shell, &menu, "allow").ok);
    let back = given_back(&mut shell);
    let told = back[0].outcome.clone().unwrap();
    assert_eq!(
        told,
        format!("ok Sending to hooks.example in 5 s. undo {}", asked.id)
    );
    assert!(host.sends.borrow().is_empty(), "held, not sent");
    let undo = format!("{{line: \"undo {}\"}}", asked.id);
    let undone = shell.run_shell_line(TERMINAL_APP, &undo).unwrap();
    assert!(
        undone.contains("Undone: the send to hooks.example"),
        "{undone}"
    );
    assert!(shell.outbox.get(asked.id).is_none(), "the entry is gone");
    shell.drain_sends(host.clock.get() / 1_000_000 + 60_000);
    assert!(
        host.sends.borrow().is_empty(),
        "nothing sent after the hold"
    );
}
