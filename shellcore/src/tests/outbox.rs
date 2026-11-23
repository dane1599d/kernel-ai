//! The outbox (ADR 0002 `Hold`, P5 close): a send allowed is held with
//! `undo` live, goes once when due with its `sending` on disk first, and never
//! goes after a restart. Driven by `hold_send`, as the send row's Allow will.

use super::*;
use crate::actlog::State;
use crate::outbox::{MAX_PENDING, Send, SendFailed, Stage};

const HOLD_MS: u32 = 5_000;
const MS: u64 = 1_000_000;

fn send(host: &str) -> Send {
    Send {
        host: String::from(host),
        path: String::from("/hook"),
        body: String::from("hello"),
    }
}

/// Allows a send to `host` as a new entry at about `at_ms`: its entry.
fn hold(host: &Rc<FakeHost>, shell: &mut Shell, to: &str, at_ms: u64) -> u64 {
    host.clock.set(at_ms * MS);
    let id = shell.next_id;
    shell.next_id += 1;
    let reply = shell.hold_send(
        id,
        &format!("send hello to {to}"),
        &format!("net_send(\"{to}\", \"/hook\", \"hello\")"),
        send(to),
        HOLD_MS,
    );
    assert_eq!(
        reply,
        ok(&format!("Sending to {to} in 5 s. undo {id}")),
        "{reply:?}"
    );
    id
}

fn sent(host: &FakeHost) -> usize {
    host.sends.borrow().len()
}

fn after_of(shell: &Shell, id: u64) -> String {
    shell.log.get(id).unwrap().changes[0].after.clone()
}

#[test]
fn a_held_send_waits_then_goes_exactly_once() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let id = hold(&host, &mut shell, "a.example", 0);
    assert_eq!(shell.next_send_ms(1_000), Some(4_000));
    shell.drain_sends(4_999);
    assert_eq!(sent(&host), 0, "it went during the hold");
    shell.drain_sends(5_000);
    assert_eq!(sent(&host), 1);
    shell.drain_sends(60_000);
    assert_eq!(sent(&host), 1, "it went twice");
    assert_eq!(host.sends.borrow()[0].0, send("a.example"));
    assert_eq!(after_of(&shell, id), "sent");
    assert_eq!(shell.log.get(id).unwrap().outcome, "Sent to a.example.");
    // The terminal's next prompt says it, once.
    assert!(
        shell
            .eval_console("1 + 1")
            .text
            .contains("Sent to a.example.")
    );
    assert!(!shell.eval_console("1 + 1").text.contains("Sent"));
}

/// Write-ahead: the log on disk said `sending` before the network was called.
#[test]
fn sending_is_on_disk_before_the_send_goes() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    hold(&host, &mut shell, "a.example", 0);
    let disk = host.log_disk.borrow().clone().unwrap();
    assert!(disk.contains("after=held inv=none"), "{disk}");
    shell.drain_sends(5_000);
    let at_send = host.sends.borrow()[0].1.clone().unwrap();
    assert!(
        at_send.contains("res=a.example before=- after=sending inv=none"),
        "{at_send}"
    );
    let disk = host.log_disk.borrow().clone().unwrap();
    assert!(disk.contains("after=sent inv=none"), "{disk}");
}

#[test]
fn a_send_whose_sending_cannot_be_written_never_goes() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let id = hold(&host, &mut shell, "a.example", 0);
    host.log_fails.set(1);
    shell.drain_sends(5_000);
    assert_eq!(sent(&host), 0);
    assert_eq!(after_of(&shell, id), "unsent");
    assert_eq!(shell.log.get(id).unwrap().state, State::Nothing);
    assert_eq!(
        shell.log.get(id).unwrap().outcome,
        "Not sent to a.example: the log could not be written first"
    );
    assert_eq!(shell.next_send_ms(5_000), None);
}

#[test]
fn undo_during_the_hold_cancels_and_undo_again_never_sends() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let id = hold(&host, &mut shell, "a.example", 0);
    assert_eq!(
        shell.eval("undo"),
        ok("Undone: the send to a.example, not sent")
    );
    shell.drain_sends(60_000);
    assert_eq!(sent(&host), 0);
    // The undo of the cancel has no inverse: it is never a resend.
    let undo = shell.log.entries().find(|e| e.of == Some(id)).unwrap();
    assert_eq!(undo.changes[0].inverse, None);
    assert_eq!(undo.changes[0].after, "cancelled");
    assert_eq!(
        shell.eval("undo"),
        Reply::err("Can't redo the send to a.example: ask again to send it")
    );
    shell.drain_sends(120_000);
    assert_eq!(sent(&host), 0);
    let ended = shell.take_sends_ended();
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0].stage, Stage::Cancelled);
    assert_eq!(ended[0].line, "Not sent to a.example: undone.");
}

/// `undo N` of a held send cancels only that one, while a newer entry that
/// changed something else stands (D7's same-resource rule).
#[test]
fn undo_n_cancels_only_its_own_send() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let first = hold(&host, &mut shell, "a.example", 0);
    hold(&host, &mut shell, "b.example", 1);
    assert_eq!(
        shell.eval(&format!("undo {first}")),
        ok("Undone: the send to a.example, not sent")
    );
    shell.drain_sends(60_000);
    let hosts: Vec<String> = host
        .sends
        .borrow()
        .iter()
        .map(|s| s.0.host.clone())
        .collect();
    assert_eq!(hosts, ["b.example"]);
}

#[test]
fn undo_after_the_send_started_or_went_says_it_was_sent() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let id = hold(&host, &mut shell, "a.example", 0);
    // Started, its answer not in yet: no undo any more.
    assert_eq!(shell.start_due_send(5_000), Some(id));
    let refused = Reply::err("Can't undo: it was sent to a.example");
    assert_eq!(shell.eval("undo"), refused);
    assert_eq!(shell.eval(&format!("undo {id}")), refused);
    shell.run_send(id);
    assert_eq!(sent(&host), 1);
    assert_eq!(shell.eval("undo"), refused);
    assert_eq!(shell.eval(&format!("undo {id}")), refused);
    assert_eq!(after_of(&shell, id), "sent");
}

/// A restart never sends: held is logged unsent, sending as may have gone.
#[test]
fn after_a_restart_nothing_held_ever_goes() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let held = hold(&host, &mut shell, "a.example", 0);
    let going = hold(&host, &mut shell, "b.example", 1);
    shell.outbox.get(going).unwrap();
    // `a.example` is due later; `b.example` starts and the shell stops.
    shell.outbox = crate::outbox::Outbox::default();
    shell.outbox.hold(held, send("a.example"), 99_000).unwrap();
    shell.outbox.hold(going, send("b.example"), 5_001).unwrap();
    assert_eq!(shell.start_due_send(5_001), Some(going));
    drop(shell);
    let mut after = super::shell(&host);
    after.drain_sends(u64::MAX / MS);
    assert_eq!(sent(&host), 0);
    assert_eq!(after_of(&after, held), "unsent");
    assert_eq!(after_of(&after, going), "unknown");
    assert_eq!(
        after.log.get(held).unwrap().outcome,
        "Not sent to a.example: Kai restarted during the hold"
    );
    assert_eq!(
        after.log.get(going).unwrap().outcome,
        "It may have been sent to b.example: Kai restarted as it was sending"
    );
    let disk = host.log_disk.borrow().clone().unwrap();
    assert!(disk.contains("after=unsent"), "{disk}");
    assert!(!disk.contains("after=held"), "{disk}");
    assert_eq!(
        after.eval("undo"),
        Reply::err("Can't undo: it may have been sent to b.example")
    );
}

#[test]
fn a_send_that_never_connected_is_not_sent() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    host.send_answers.borrow_mut().push_back(Err(SendFailed {
        why: String::from("no connection to a.example"),
        maybe_sent: false,
    }));
    let id = hold(&host, &mut shell, "a.example", 0);
    shell.drain_sends(5_000);
    assert_eq!(after_of(&shell, id), "failed");
    assert_eq!(shell.log.get(id).unwrap().state, State::Nothing);
    assert_eq!(
        shell.log.get(id).unwrap().outcome,
        "Not sent to a.example: no connection to a.example"
    );
}

/// Fail-closed: any answer means the site got the body, a 403 too.
#[test]
fn an_answer_with_an_error_status_was_still_sent() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    host.send_answers
        .borrow_mut()
        .push_back(Ok(String::from("403 Forbidden\nmore")));
    let id = hold(&host, &mut shell, "a.example", 0);
    shell.drain_sends(5_000);
    assert_eq!(after_of(&shell, id), "sent");
    assert_eq!(
        shell.log.get(id).unwrap().outcome,
        "Sent to a.example; it answered 403 Forbidden"
    );
    host.send_answers
        .borrow_mut()
        .push_back(Ok(String::from("204 No Content")));
    let quiet = hold(&host, &mut shell, "b.example", 10_000);
    shell.drain_sends(15_000);
    assert_eq!(shell.log.get(quiet).unwrap().outcome, "Sent to b.example.");
}

/// Cancelling one hold never overwrites another to the same host (#958
/// review): `undo <older>` cancels the older send only.
#[test]
fn undo_of_an_older_send_to_the_same_host_cancels_only_it() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let older = hold(&host, &mut shell, "hooks.example", 0);
    let newer = hold(&host, &mut shell, "hooks.example", 1);
    assert_eq!(
        shell.eval(&format!("undo {older}")),
        ok("Undone: the send to hooks.example, not sent")
    );
    shell.drain_sends(60_000);
    assert_eq!(sent(&host), 1);
    assert_eq!(after_of(&shell, newer), "sent");
}

/// A second bare `undo` after a cancel names the send still waiting, with
/// its `undo`, and that send can still be cancelled.
#[test]
fn a_second_undo_names_the_send_still_waiting() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let first = hold(&host, &mut shell, "a.example", 0);
    hold(&host, &mut shell, "b.example", 1);
    assert_eq!(
        shell.eval("undo"),
        ok("Undone: the send to b.example, not sent")
    );
    assert_eq!(
        shell.eval("undo"),
        Reply::err(format!(
            "Can't redo the send to b.example: ask again to send it. a.example is still waiting: undo {first}"
        ))
    );
    assert_eq!(
        shell.eval(&format!("undo {first}")),
        ok("Undone: the send to a.example, not sent")
    );
    shell.drain_sends(60_000);
    assert_eq!(sent(&host), 0);
}

/// A sent send behind a later entry with no undo still stops a bare `undo`
/// from reaching back past it.
#[test]
fn a_sent_send_behind_an_entry_with_no_undo_still_stops_undo() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let id = hold(&host, &mut shell, "a.example", 0);
    shell.drain_sends(5_000);
    let mut stuck = shell.log.get(id).unwrap().clone();
    stuck.id = shell.next_id;
    stuck.changes[0].action = String::from("set_policy");
    shell.next_id += 1;
    shell.log.push(stuck);
    assert_eq!(
        shell.eval("undo"),
        Reply::err("Can't undo: it was sent to a.example")
    );
}

/// An entry gone from the log's 64-record ring never sends: there is no
/// `sending` to write first. Until it is due, `undo <id>` still cancels.
#[test]
fn a_send_whose_entry_left_the_log_never_goes() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let gone = hold(&host, &mut shell, "a.example", 0);
    let cancelled = hold(&host, &mut shell, "b.example", 1);
    for _ in 0..crate::actlog::KEEP {
        let id = shell.next_id;
        shell.next_id += 1;
        shell.record(id, "hello", "", "answered in words");
    }
    assert!(shell.log.get(gone).is_none());
    assert_eq!(
        shell.eval(&format!("undo {cancelled}")),
        ok("Undone: the send to b.example, not sent")
    );
    shell.drain_sends(60_000);
    assert_eq!(sent(&host), 0);
    assert_eq!(shell.next_send_ms(60_000), None);
}

#[test]
fn a_send_to_something_that_is_not_a_host_is_refused() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    for bad in ["a example", "a;b", "a/b", ""] {
        let id = shell.next_id;
        shell.next_id += 1;
        let reply = shell.hold_send(id, "send it", "net_send()", send(bad), HOLD_MS);
        assert_eq!(
            reply,
            Reply::err("Not sent: that is not a host name"),
            "{bad:?}"
        );
    }
    assert_eq!(shell.next_send_ms(0), None);
}

#[test]
fn a_log_set_aside_with_a_send_going_says_so() {
    let host = FakeHost::new();
    *host.log_disk.borrow_mut() = Some(String::from(
        "actlog 1\nE 1 t=0 reg=9 pack=- form=call tier=ask consent=asked state=done\nD 1.1 net_send owner=fetch res=a.example before=- after=sending inv=none\n",
    ));
    let _shell = shell(&host);
    assert!(
        host.audit
            .borrow()
            .iter()
            .any(|l| l.contains("held a send that may have been sent"))
    );
}

#[test]
fn a_cut_after_the_body_went_may_have_sent_it() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    host.send_answers.borrow_mut().push_back(Err(SendFailed {
        why: String::from("cut: the server may have it"),
        maybe_sent: true,
    }));
    let id = hold(&host, &mut shell, "a.example", 0);
    shell.drain_sends(5_000);
    assert_eq!(after_of(&shell, id), "unknown");
    assert_eq!(
        shell.log.get(id).unwrap().outcome,
        "It may have been sent to a.example: cut: the server may have it"
    );
    assert_eq!(sent(&host), 1, "it was tried again");
}

/// A Stop ends a call only while no server has taken it: a send it withdrew
/// says not sent, as the host's `maybe_sent` does; one cut after it went out
/// may have been sent (#975 review).
#[test]
fn a_stop_is_taken_at_the_hosts_word() {
    for (maybe_sent, after) in [(false, "failed"), (true, "unknown")] {
        let host = FakeHost::new();
        let mut shell = shell(&host);
        host.send_answers.borrow_mut().push_back(Err(SendFailed {
            why: String::from("stopped"),
            maybe_sent,
        }));
        let id = hold(&host, &mut shell, "a.example", 0);
        host.stopped.set(true);
        shell.drain_sends(5_000);
        assert_eq!(after_of(&shell, id), after);
    }
}

#[test]
fn a_ninth_send_is_refused_and_logged() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    for n in 0..MAX_PENDING as u64 {
        hold(&host, &mut shell, "a.example", n);
    }
    let id = shell.next_id;
    shell.next_id += 1;
    let reply = shell.hold_send(id, "send it", "net_send()", send("a.example"), HOLD_MS);
    assert!(!reply.ok);
    assert!(
        reply.text.contains("8 sends are waiting already"),
        "{reply:?}"
    );
    assert_eq!(shell.log.get(id).unwrap().state, State::Nothing);
    shell.drain_sends(60_000);
    assert_eq!(sent(&host), MAX_PENDING);
}

/// Other lines between the Allow and the send leave it held.
#[test]
fn the_outbox_survives_other_lines() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let id = hold(&host, &mut shell, "a.example", 0);
    assert_eq!(shell.eval("1 + 1"), ok("2"));
    shell.eval("log");
    shell.eval("new");
    assert_eq!(shell.held_sends(1_000)[0].entry, id);
    assert_eq!(
        shell.eval(&format!("undo {id}")),
        ok("Undone: the send to a.example, not sent")
    );
}

#[test]
fn the_panel_sees_a_send_wait_go_and_end() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let id = hold(&host, &mut shell, "a.example", 0);
    let shown = shell.held_sends(1_000);
    assert_eq!(shown.len(), 1);
    assert_eq!((shown[0].entry, shown[0].sending), (id, false));
    assert_eq!(shown[0].left_ms, 4_000);
    assert_eq!(shell.start_due_send(5_000), Some(id));
    assert!(shell.held_sends(5_000)[0].sending);
    shell.run_send(id);
    assert!(shell.held_sends(5_000).is_empty());
    let ended = shell.take_sends_ended();
    assert_eq!(ended.len(), 1);
    assert_eq!((ended[0].entry, ended[0].stage), (id, Stage::Sent));
    assert!(shell.take_sends_ended().is_empty());
}

/// The OS loop starts one send a pass, so an undo taken between two passes
/// (a click during the first send's call) cancels the second (#959 review).
#[test]
fn an_undo_between_two_due_sends_cancels_the_second() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let first = hold(&host, &mut shell, "a.example", 0);
    let second = hold(&host, &mut shell, "b.example", 2_000);
    // Both due by the time the first send's call returns.
    assert_eq!(shell.start_due_send(9_000), Some(first));
    shell.run_send(first);
    assert_eq!(
        shell.eval(&format!("undo {second}")),
        ok("Undone: the send to b.example, not sent")
    );
    assert_eq!(shell.start_due_send(9_000), None);
    let hosts: Vec<String> = host
        .sends
        .borrow()
        .iter()
        .map(|s| s.0.host.clone())
        .collect();
    assert_eq!(hosts, ["a.example"]);
}

/// The shell's host rule and the pane's wire take the same hosts.
#[test]
fn the_shell_and_the_pane_agree_on_what_a_host_is() {
    for host in [
        "hooks.example",
        "10.0.2.2",
        "[::1]",
        "a-b.c",
        "",
        "a b",
        "a;b",
        "a/b",
        "a\tb",
        "caf\u{e9}.example",
    ] {
        assert_eq!(
            crate::outbox::is_host(host),
            deskcore::sends::is_host(host),
            "{host:?}"
        );
    }
    let long = "a".repeat(crate::outbox::MAX_HOST + 1);
    assert!(!crate::outbox::is_host(&long) && !deskcore::sends::is_host(&long));
}

/// A panel line's card that allowed a send hands the panel the send's
/// entry with its result; one that held nothing hands none.
#[test]
fn a_panel_cards_result_names_the_send_it_held() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let id = hold(&host, &mut shell, "a.example", 0);
    shell.desk_told(
        id,
        crate::deskask::Asker::Panel,
        true,
        false,
        String::from("Sending"),
    );
    assert_eq!(shell.take_card_held(), Some(id));
    assert_eq!(shell.take_card_held(), None);
    shell.desk_told(
        id + 7,
        crate::deskask::Asker::Panel,
        true,
        false,
        String::from("Done."),
    );
    assert_eq!(shell.take_card_held(), None);
    shell.desk_told(
        id,
        crate::deskask::Asker::Panel,
        false,
        false,
        String::from("Not run"),
    );
    assert_eq!(shell.take_card_held(), None);
}

/// The OS loop's passes, as `take_panel` then `give_sends`: each request
/// (`Some`, a line run, `None` for a Stop) is one pass, and a pass that took
/// one starts no send. Then a pass with nothing to take.
fn passes(shell: &mut Shell, requests: &[Option<&str>], now_ms: u64) -> Vec<u64> {
    let mut started = Vec::new();
    for request in requests.iter().map(Some).chain([None]) {
        if let Some(Some(line)) = request {
            shell.eval(line);
        }
        if let Some(entry) = shell.start_send_after(request.is_some(), now_ms) {
            shell.run_send(entry);
            started.push(entry);
        }
    }
    started
}

/// An undo queued behind a Stop, or behind a Stop and a line, is taken
/// before the due send it names starts (#959 re-review).
#[test]
fn an_undo_queued_behind_a_stop_wins_over_its_due_send() {
    for before in [&[None][..], &[None, Some("echo hi")][..]] {
        let host = FakeHost::new();
        let mut shell = shell(&host);
        let id = hold(&host, &mut shell, "a.example", 0);
        let undo = format!("undo {id}");
        let requests: Vec<Option<&str>> = before
            .iter()
            .copied()
            .chain([Some(undo.as_str())])
            .collect();
        assert_eq!(passes(&mut shell, &requests, 9_000), Vec::<u64>::new());
        assert!(host.sends.borrow().is_empty(), "{before:?}");
        assert_eq!(shell.start_due_send(9_000), None);
    }
    // With nothing queued, the due send starts on the next pass.
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let id = hold(&host, &mut shell, "a.example", 0);
    assert_eq!(passes(&mut shell, &[None], 9_000), [id]);
}

/// A site's answer is said in code's fixed words, never the server's own
/// reason; a 2xx reads as plainly sent (#962's note).
#[test]
fn a_status_is_said_in_fixed_words() {
    use crate::outbox::{Stage, ended_line, status_line};
    assert_eq!(status_line(200), "200 OK");
    assert_eq!(status_line(404), "404 Not Found");
    assert_eq!(status_line(302), "302 Redirect, not followed");
    assert_eq!(status_line(299), "299");
    assert_eq!(
        ended_line("a.example", Stage::Sent, &status_line(204)),
        "Sent to a.example."
    );
    assert_eq!(
        ended_line("a.example", Stage::Sent, &status_line(503)),
        "Sent to a.example; it answered 503 Service Unavailable"
    );
}

/// Only a call the kernel refused before the send service could take it is
/// "not sent"; a timeout, or the service dying with it, may have gone out.
#[test]
fn a_failed_call_is_not_sent_only_when_nothing_took_it() {
    use crate::outbox::call_failed;
    use kabi::Error as E;
    for error in [
        E::NotFound,
        E::Unavailable,
        E::Interrupted,
        E::BadHandle,
        E::Permission,
    ] {
        assert!(!call_failed(error).maybe_sent, "{error:?}");
    }
    for error in [
        E::TimedOut,
        E::Dead,
        E::Busy,
        E::Network,
        E::Woken,
        E::Fault,
    ] {
        assert!(call_failed(error).maybe_sent, "{error:?}");
    }
}

/// A host the transport would refuse when due (a port, an address) is
/// refused as it is held, so nobody waits out a countdown for nothing.
#[test]
fn a_host_the_transport_refuses_is_refused_at_hold_time() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    for to in ["hooks.example:8443", "[::1]", "10.0.2.2"] {
        let id = shell.next_id;
        shell.next_id += 1;
        let reply = shell.hold_send(id, "send it", "net_send()", send(to), HOLD_MS);
        assert_eq!(reply.text, "Not sent: that is not a host name", "{to}");
        assert!(!reply.ok);
    }
    assert!(shell.held_sends(0).is_empty());
}

/// A send the desktop never let anyone check for an undo is ended unsent a
/// minute past due, with code's words, and never goes (#975 review).
#[test]
fn a_send_left_unchecked_for_a_minute_is_ended_unsent() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let id = hold(&host, &mut shell, "a.example", 0);
    let due = u64::from(HOLD_MS);
    shell.end_unchecked_sends(due + crate::outbox::UNCHECKED_MS - 1);
    assert_eq!(shell.held_sends(0).len(), 1);
    shell.end_unchecked_sends(due + crate::outbox::UNCHECKED_MS);
    assert_eq!(after_of(&shell, id), "unsent");
    let ended = shell.take_sends_ended();
    assert_eq!(
        ended[0].line,
        format!("Not sent to a.example: {}", crate::outbox::UNCHECKED_WHY)
    );
    shell.drain_sends(u64::MAX / 2);
    assert_eq!(sent(&host), 0);
}

/// Every inverse this shell writes fits [`crate::undo::MAX_RAW`], so an older
/// shell keeps each one word for word.
#[test]
fn the_longest_inverse_fits_the_raw_word() {
    use crate::undo::{Inverse, MAX_RAW, Name, inverse_word};
    let name = Name::new(&"n".repeat(kabi::MAX_NAME)).unwrap();
    let longest = [
        Inverse::Model {
            back: Some([0xff; 32]),
            from: [0xff; 32],
        },
        Inverse::Appearance {
            setting: name,
            before: name,
            after: name,
        },
        Inverse::Fired {
            rule: u64::MAX,
            action: name,
            app: name,
            undo: true,
        },
        Inverse::Spec {
            policy: u64::MAX,
            preset: u64::MAX,
            after: (u64::MAX, u64::MAX),
        },
    ];
    for inverse in longest {
        let word = inverse_word(Some(inverse));
        assert!(word.len() <= MAX_RAW, "{word} is {} bytes", word.len());
        assert_eq!(
            crate::undo::Raw::new(&word).map(|raw| String::from(raw.as_str())),
            Some(word)
        );
    }
}

/// A desktop restart ends every held send unsent, in code's words, and the
/// one already sending goes on (the send-chain audit's MEDIUM).
#[test]
fn a_desktop_restart_ends_every_held_send_unsent() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let going = hold(&host, &mut shell, "a.example", 0);
    let held = hold(&host, &mut shell, "b.example", 4_000);
    assert_eq!(shell.start_due_send(5_000), Some(going));
    shell.end_held_sends_on_restart();
    assert_eq!(after_of(&shell, held), "unsent");
    assert_eq!(after_of(&shell, going), "sending");
    let ended = shell.take_sends_ended();
    assert_eq!(
        ended[0].line,
        format!("Not sent to b.example: {}", crate::outbox::RESTART_WHY)
    );
    shell.run_send(going);
    shell.drain_sends(u64::MAX / 2);
    let hosts: Vec<String> = host
        .sends
        .borrow()
        .iter()
        .map(|s| s.0.host.clone())
        .collect();
    assert_eq!(hosts, ["a.example"]);
}
