//! `effects`' tests: the queue, the take and back wire, results and stops.

use super::*;

/// #892's review: a Stop before the shell took the person's line takes it
/// back; the app's other lines, and other apps', stay.
#[test]
fn a_persons_queued_lines_are_withdrawn_and_the_rest_stay() {
    let mut queue = Queue::default();
    queue.put("terminal-window", &line(1));
    queue.put_as("terminal-window", &line(2), true);
    queue.put_as("files", &line(3), true);
    assert_eq!(queue.withdraw_users("terminal-window"), [2]);
    assert!(queue.withdraw_users("terminal-window").is_empty());
    assert!(queue.withdraw_users("monitor").is_empty());
    assert_eq!(
        queue.take(4096),
        Some(taken("terminal-window", false, &[line(1)]))
    );
    assert_eq!(queue.take(4096), Some(taken("files", true, &[line(3)])));
}

/// The Terminal's Stop is taken out of its lines; nothing else is.
#[test]
fn a_stop_is_taken_out_and_other_lines_stay() {
    let lines = "effect 4 call({name: \"shell_line\", args: \"1 + 1\"})\n\
                     effect 5 call({name: \"shell_stop\", args: \"\"})\n";
    let (stops, rest) = split_stops(lines);
    assert_eq!(stops, [5]);
    assert_eq!(
        rest,
        "effect 4 call({name: \"shell_line\", args: \"1 + 1\"})\n"
    );
    // A name that only starts like it is not a stop.
    let (stops, _) = split_stops("effect 6 call({name: \"shell_stopper\", args: \"\"})\n");
    assert!(stops.is_empty());
}

fn line(ticket: u32) -> String {
    format!("effect {ticket} notify(\"n\")")
}

fn taken(app: &str, user: bool, lines: &[String]) -> Taken {
    Taken {
        app: String::from(app),
        user,
        lines: lines.to_vec(),
    }
}

fn done_text(ticket: u32, text: &str) -> Done {
    Done {
        ticket,
        outcome: Ok(text.to_string()),
    }
}

/// Shell to desktop to app, as the three of them pass a result.
fn through(app: &str, user: bool, results: &[Done]) -> Vec<Done> {
    let request = encode_done_as(app, user, results);
    assert!(
        request.len() <= kabi::MAX_MESSAGE,
        "{} bytes",
        request.len()
    );
    let (name, parsed) = parse_done(&request).unwrap();
    let mut queue = Queue::default();
    for done in parsed {
        queue.finish(name, done);
    }
    let mut back = Vec::new();
    loop {
        let batch = queue.results(name, MESSAGE_BUDGET);
        if batch.is_empty() {
            return back;
        }
        let text = encode_results(&batch);
        assert!(text.len() <= MESSAGE_BUDGET + 64, "{} bytes", text.len());
        back.extend(parse_results(&text));
    }
}

#[test]
fn a_two_kib_result_from_a_persons_batch_arrives_whole() {
    let text = "x".repeat(MAX_LINE_TEXT);
    let back = through("terminal-window", true, &[done_text(1, &text)]);
    assert_eq!(back, [done_text(1, &text)]);
    // Over the ceiling it is cut there, at a character.
    let over = "\u{e9}".repeat(MAX_LINE_TEXT);
    let back = through("terminal-window", true, &[done_text(2, &over)]);
    let Ok(cut) = &back[0].outcome else { panic!() };
    assert!(cut.len() <= MAX_LINE_TEXT && over.starts_with(cut.as_str()));
}

#[test]
fn any_other_result_is_still_cut_at_two_hundred() {
    let text = "y".repeat(MAX_LINE_TEXT);
    let back = through("files", false, &[done_text(1, &text)]);
    assert_eq!(back[0].outcome, Ok("y".repeat(MAX_TEXT)));
}

#[test]
fn results_over_one_message_come_back_across_calls_and_none_is_lost() {
    let mut queue = Queue::default();
    let text = "z".repeat(MAX_LINE_TEXT);
    for ticket in 1..=6 {
        queue.finish("terminal-window", done_text(ticket, &text));
    }
    let first = queue.results("terminal-window", MESSAGE_BUDGET);
    assert!(!first.is_empty() && first.len() < 6, "{}", first.len());
    assert!(encode_results(&first).len() <= MESSAGE_BUDGET);
    let rest = queue.results("terminal-window", MESSAGE_BUDGET);
    let tickets: Vec<u32> = first.iter().chain(&rest).map(|d| d.ticket).collect();
    assert_eq!(tickets, [1, 2, 3, 4, 5, 6][..tickets.len()]);
    // However small the budget, one result still moves.
    queue.finish("terminal-window", done_text(7, "a"));
    queue.finish("terminal-window", done_text(8, "b"));
    assert_eq!(queue.results("terminal-window", 0).len(), 1);
    assert_eq!(queue.results("terminal-window", 0).len(), 1);
    assert!(queue.results("terminal-window", 0).is_empty());
}

#[test]
fn a_batch_of_many_long_results_shares_one_request() {
    let text = "w".repeat(MAX_LINE_TEXT);
    let batch: Vec<Done> = (1..=16).map(|t| done_text(t, &text)).collect();
    let back = through("terminal-window", true, &batch);
    assert_eq!(back.len(), 16);
    assert!(
        back.iter()
            .all(|d| matches!(&d.outcome, Ok(t) if !t.is_empty()))
    );
}

#[test]
fn a_persons_lines_are_taken_apart_from_the_rest_and_keep_their_flag() {
    let mut queue = Queue::default();
    queue.put_as("terminal-window", &line(1), true);
    queue.put("terminal-window", &line(2));
    queue.put_as(
        "terminal-window",
        &format!("{}\n{}", line(3), line(4)),
        true,
    );
    let first = queue.take(4096).unwrap();
    assert_eq!(first, taken("terminal-window", true, &[line(1)]));
    assert_eq!(
        queue.take(4096),
        Some(taken("terminal-window", false, &[line(2)]))
    );
    assert_eq!(
        queue.take(4096),
        Some(taken("terminal-window", true, &[line(3), line(4)]))
    );
    assert_eq!(queue.take(4096), None);
}

#[test]
fn a_refused_persons_line_still_gets_its_error_result() {
    let mut queue = Queue::default();
    let long = format!("effect 7 notify(\"{}\")", "x".repeat(MAX_LINE));
    assert_eq!(queue.put_as("terminal-window", &long, true), 1);
    assert_eq!(queue.results("terminal-window", 4096)[0].ticket, 7);
    assert_eq!(queue.take(4096), None);
}

#[test]
fn the_flag_rides_the_wire_only_as_the_word_user() {
    let user = taken("terminal-window", true, &[line(1)]);
    let text = encode_take(Some(&user));
    assert!(text.starts_with("app terminal-window user\n"));
    assert_eq!(
        parse_take(&text),
        Some(Some(("terminal-window", true, line(1).as_str())))
    );
    let plain = taken("terminal-window", false, &[line(1)]);
    assert_eq!(
        parse_take(&encode_take(Some(&plain))),
        Some(Some(("terminal-window", false, line(1).as_str())))
    );
    // The word is a suffix of the app, not part of a line's text.
    let sneaky = taken(
        "files",
        false,
        &[String::from("effect 1 notify(\"x user\")")],
    );
    assert_eq!(
        parse_take(&encode_take(Some(&sneaky))).map(|t| t.map(|(a, u, _)| (a, u))),
        Some(Some(("files", false)))
    );
}

/// #890: lines the shell took and puts back go to the front of their
/// app's queue, in order and with their flag, and a take skipping that
/// app passes it by.
#[test]
fn lines_put_back_come_first_and_a_take_may_skip_their_app() {
    let mut queue = Queue::default();
    queue.put("files", &format!("{}\n{}", line(1), line(2)));
    queue.put("monitor", &line(9));
    let held = queue.take(4096).unwrap();
    assert_eq!(held, taken("files", false, &[line(1), line(2)]));
    queue.put("files", &line(3));
    let text = encode_back(&held);
    assert_eq!(queue.put_back(parse_back(&text).unwrap()), 2);
    assert_eq!(
        queue.take_but(4096, &["files"]),
        Some(taken("monitor", false, &[line(9)]))
    );
    assert_eq!(queue.take_but(4096, &["files"]), None);
    assert_eq!(
        queue.take(4096),
        Some(taken("files", false, &[line(1), line(2), line(3)]))
    );
}

#[test]
fn a_persons_lines_put_back_keep_their_flag_and_bad_lines_are_dropped() {
    let mut queue = Queue::default();
    queue.put_as("terminal-window", &line(1), true);
    queue.put("terminal-window", &line(2));
    assert_eq!(
        queue.take(4096),
        Some(taken("terminal-window", true, &[line(1)]))
    );
    // Its flag is the one it was taken with, whatever the back says.
    let back = Taken {
        app: String::from("terminal-window"),
        user: false,
        lines: alloc::vec![line(1), String::from("not an effect"), line(7)],
    };
    assert_eq!(queue.put_back(back), 1, "line 7 was never taken");
    assert_eq!(
        queue.take(4096),
        Some(taken("terminal-window", true, &[line(1)]))
    );
    assert_eq!(
        queue.take(4096),
        Some(taken("terminal-window", false, &[line(2)]))
    );
}

/// #913's review: a line put back twice, or after its result, is not
/// queued again.
#[test]
fn a_line_goes_back_once_and_never_after_its_result() {
    let mut queue = Queue::default();
    queue.put("files", &format!("{}\n{}", line(1), line(2)));
    let held = queue.take(4096).unwrap();
    queue.finish(
        "files",
        Done {
            ticket: 2,
            outcome: Ok(String::from("done")),
        },
    );
    assert_eq!(queue.put_back(held.clone()), 1, "line 2 was answered");
    assert_eq!(queue.put_back(held), 0, "sent twice");
    assert_eq!(queue.take(4096), Some(taken("files", false, &[line(1)])));
    assert_eq!(queue.take(4096), None);
}

/// #913's review: lines put back may fill a queue past its limit, and new
/// lines are refused until it is under it again.
#[test]
fn a_queue_full_after_a_put_back_still_refuses_new_lines() {
    let mut queue = Queue::default();
    let first: Vec<String> = (0..MAX_QUEUED as u32).map(line).collect();
    assert_eq!(queue.put("files", &first.join("\n")), 0);
    let held = queue.take(usize::MAX).unwrap();
    let more: Vec<String> = (100..100 + MAX_QUEUED as u32).map(line).collect();
    assert_eq!(queue.put("files", &more.join("\n")), 0);
    assert_eq!(queue.put_back(held), MAX_QUEUED);
    assert_eq!(queue.put("files", &line(500)), 1, "over the limit");
    let refused = queue.results("files", MESSAGE_BUDGET);
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0].ticket, 500);
    let taken = queue.take(usize::MAX).unwrap();
    assert_eq!(taken.lines.len(), 2 * MAX_QUEUED);
    assert_eq!(queue.put("files", &line(501)), 0, "room again");
}

/// #913's review: tickets start again when an app reloads, so a ticket
/// taken again goes back with the flag of its newest take.
#[test]
fn a_ticket_taken_again_goes_back_as_its_newest_take() {
    let mut queue = Queue::default();
    queue.put("notes", &line(1));
    queue.take(4096).unwrap();
    // The app reloaded and its ticket 1 is a person's this time.
    queue.put_as("notes", &line(1), true);
    let newest = queue.take(4096).unwrap();
    assert_eq!(queue.put_back(newest.clone()), 1);
    assert_eq!(queue.take(4096), Some(newest.clone()));
    assert_eq!(queue.put_back(newest), 1, "one entry, not two");
    queue.take(4096).unwrap();
    assert_eq!(queue.put_back(taken("notes", true, &[line(1)])), 1);
    assert_eq!(queue.put_back(taken("notes", true, &[line(1)])), 0);
}

/// #938: the shell gave lines back, then told the app one did not run:
/// that one is withdrawn and never runs after; another app's line with
/// the same ticket stays.
#[test]
fn a_result_for_a_line_given_back_withdraws_it() {
    let mut queue = Queue::default();
    queue.put("files", &format!("{}\n{}", line(1), line(2)));
    queue.put("monitor", &line(1));
    let held = queue.take(4096).unwrap();
    assert_eq!(held.app, "files");
    assert_eq!(queue.put_back(held), 2);
    let lost = |ticket| Done {
        ticket,
        outcome: Err(String::from("Not run")),
    };
    queue.finish("files", lost(1));
    assert_eq!(
        queue.take_app(4096, "files"),
        Some(taken("files", false, &[line(2)]))
    );
    queue.finish("files", lost(2));
    assert_eq!(queue.take_app(4096, "files"), None);
    assert_eq!(queue.results("files", MESSAGE_BUDGET).len(), 2);
    assert_eq!(
        queue.take_app(4096, "monitor"),
        Some(taken("monitor", false, &[line(1)]))
    );
}

/// A line queued under a ticket like `n`'s, saying `word`: an app that
/// reloaded numbers its lines again.
fn line_saying(ticket: u32, word: &str) -> String {
    format!("effect {ticket} notify(\"{word}\")")
}

/// #938's follow-up: tickets start again when an app reloads. A late
/// result for an old instance's ticket withdraws no new line of that
/// number; one for a line given back withdraws only that line; and once
/// it is taken again, it is no longer given back.
#[test]
fn only_a_line_given_back_is_withdrawn_by_its_result() {
    let mut queue = Queue::default();
    let lost = |ticket| Done {
        ticket,
        outcome: Err(String::from("Not run")),
    };
    let (old, new) = (line_saying(1, "old"), line_saying(1, "new"));
    // Taken, and the app reloads: the old line's late result.
    queue.put("files", &old);
    queue.take(4096).unwrap();
    queue.put("files", &new);
    queue.finish("files", lost(1));
    let fresh = Some(taken("files", false, core::slice::from_ref(&new)));
    assert_eq!(queue.take_app(4096, "files"), fresh, "the new line stays");
    // Given back, and the app reloads: only the given-back one goes.
    assert_eq!(
        queue.put_back(taken("files", false, core::slice::from_ref(&new))),
        1
    );
    queue.put("files", &old);
    queue.finish("files", lost(1));
    let reloaded = Some(taken("files", false, core::slice::from_ref(&old)));
    assert_eq!(queue.take_app(4096, "files"), reloaded);
    assert_eq!(queue.take_app(4096, "files"), None);
    // Given back, taken again: a later line 1 is no longer at risk.
    assert_eq!(
        queue.put_back(taken("files", false, core::slice::from_ref(&old))),
        1
    );
    queue.take_app(4096, "files").unwrap();
    queue.put("files", &new);
    queue.finish("files", lost(1));
    assert_eq!(queue.take_app(4096, "files"), fresh);
    assert_eq!(queue.results("files", MESSAGE_BUDGET).len(), 3);
}

#[test]
fn take_but_and_back_read_back() {
    assert_eq!(encode_take_but(&[]), TAKE);
    let text = encode_take_but(&["files", "notes"]);
    assert_eq!(parse_take_but(&text), Some(alloc::vec!["files", "notes"]));
    assert_eq!(parse_take_but(TAKE), None);
    let user = taken("terminal-window", true, &[line(1), line(2)]);
    assert_eq!(parse_back(&encode_back(&user)), Some(user));
    assert_eq!(parse_back("effects back none"), None);
    assert_eq!(parse_back(TAKE), None);
}

#[test]
fn effects_wait_by_app_and_the_shell_takes_one_apps_at_a_time() {
    let mut queue = Queue::default();
    assert_eq!(queue.put("files", &format!("{}\n{}", line(1), line(2))), 0);
    assert_eq!(queue.put("monitor", &line(9)), 0);
    assert_eq!(
        queue.take(4096),
        Some(taken("files", false, &[line(1), line(2)]))
    );
    assert_eq!(queue.take(4096), Some(taken("monitor", false, &[line(9)])));
    assert_eq!(queue.take(4096), None);
}

#[test]
fn apps_take_turns() {
    let mut queue = Queue::default();
    let lines: Vec<String> = (0..4).map(line).collect();
    queue.put(
        "files",
        &lines.join(
            "
",
        ),
    );
    queue.put("monitor", &line(9));
    let budget = line(0).len() + 1;
    let order: Vec<String> = core::iter::from_fn(|| queue.take(budget).map(|t| t.app)).collect();
    assert_eq!(order, ["files", "monitor", "files", "files", "files"]);
}

#[test]
fn a_take_keeps_to_its_budget_but_always_moves() {
    let mut queue = Queue::default();
    queue.put("files", &format!("{}\n{}", line(1), line(2)));
    assert_eq!(queue.take(1).map(|t| t.lines), Some(alloc::vec![line(1)]));
    assert_eq!(queue.take(1).map(|t| t.lines), Some(alloc::vec![line(2)]));
}

#[test]
fn past_the_limit_an_effect_is_refused_with_a_result() {
    let mut queue = Queue::default();
    let lines: Vec<String> = (0..=MAX_QUEUED as u32).map(line).collect();
    assert_eq!(queue.put("files", &lines.join("\n")), 1);
    let results = queue.results("files", MESSAGE_BUDGET);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].ticket, MAX_QUEUED as u32);
    assert!(results[0].outcome.is_err());
    assert_eq!(
        queue.take(usize::MAX).map(|t| t.lines.len()),
        Some(MAX_QUEUED)
    );
}

#[test]
fn lines_that_are_not_effects_are_ignored() {
    let mut queue = Queue::default();
    assert_eq!(queue.put("files", "result ok\neffect x y\n"), 0);
    assert_eq!(queue.take(4096), None);
}

#[test]
fn results_go_back_to_their_app_only_and_are_bounded() {
    let mut queue = Queue::default();
    for ticket in 0..MAX_RESULTS as u32 + 2 {
        queue.finish(
            "files",
            Done {
                ticket,
                outcome: Ok(String::new()),
            },
        );
    }
    assert!(queue.results("monitor", MESSAGE_BUDGET).is_empty());
    let results = queue.results("files", MESSAGE_BUDGET);
    assert_eq!(results.len(), MAX_RESULTS);
    assert_eq!(results[0].ticket, 2);
    assert!(queue.results("files", MESSAGE_BUDGET).is_empty());
}

#[test]
fn an_app_takes_results_only_for_tickets_it_sent_and_once() {
    let mut sent = Sent::default();
    assert!(!sent.awaits());
    sent.record(&format!("{}\n{}\nresult ok", line(4), line(5)));
    assert!(sent.awaits());
    let done = |ticket| Done {
        ticket,
        outcome: Ok(String::new()),
    };
    let (accepted, stray) = sent.accept(alloc::vec![done(5), done(9), done(5)]);
    assert_eq!(accepted, [done(5)]);
    assert_eq!(stray, [9, 5]);
    assert!(sent.awaits());
    let (accepted, _) = sent.accept(alloc::vec![done(4)]);
    assert_eq!(accepted, [done(4)]);
    assert!(!sent.awaits());
}

/// A call's whole reply (`kai-reply 1`, its result, its effects, its undo) is
/// what comphost notes for an agent's `files_open`: only the effect lines'
/// tickets are awaited, and the shell's results for them are then taken.
#[test]
fn the_effects_in_a_calls_reply_are_awaited_and_nothing_else() {
    let mut sent = Sent::default();
    let reply = format!(
        "kai-reply 1\nresult ok(\"Opening docs.\")\n{}\n{}\nundo 3\n",
        line(7),
        line(8)
    );
    sent.record(&reply);
    let done = |ticket| Done {
        ticket,
        outcome: Ok(String::from("a.txt\nb.txt")),
    };
    let (accepted, stray) = sent.accept(alloc::vec![done(7), done(3), done(8)]);
    assert_eq!(accepted, [done(7), done(8)]);
    assert_eq!(stray, [3]);
    assert!(!sent.awaits());
    // A reply with no effects awaits nothing.
    sent.record("kai-reply 1\nresult ok(1)\n");
    assert!(!sent.awaits());
}

#[test]
fn the_record_is_bounded() {
    let mut sent = Sent::default();
    let lines: Vec<String> = (0..=MAX_RESULTS as u32).map(line).collect();
    sent.record(&lines.join("\n"));
    let done = |ticket| Done {
        ticket,
        outcome: Ok(String::new()),
    };
    let (accepted, stray) = sent.accept(alloc::vec![done(0), done(1)]);
    assert_eq!((accepted.len(), stray), (1, alloc::vec![0]));
}

#[test]
fn the_wire_reads_back() {
    let taken = taken("files", false, &[line(1), line(2)]);
    let text = encode_take(Some(&taken));
    let lines = format!("{}\n{}", line(1), line(2));
    assert_eq!(
        parse_take(&text),
        Some(Some(("files", false, lines.as_str())))
    );
    assert_eq!(parse_take(&encode_take(None)), Some(None));
    assert_eq!(parse_take("app"), None);

    let results = alloc::vec![
        Done {
            ticket: 1,
            outcome: Ok(String::from("Done: x")),
        },
        Done {
            ticket: 2,
            outcome: Err(String::from("no\tsuch\nrow")),
        },
    ];
    let request = encode_done("files", &results);
    let (app, back) = parse_done(&request).expect("a done request");
    assert_eq!(app, "files");
    assert_eq!(back[0], results[0]);
    // A text of several lines goes whole, on one line.
    assert_eq!(back[1], results[1]);
    assert_eq!(parse_done("effects take"), None);
}

fn ok(text: &str) -> Done {
    Done {
        ticket: 7,
        outcome: Ok(String::from(text)),
    }
}

fn through_the_wire(done: &Done, cap: usize) -> Done {
    let wire = encode_results_at(core::slice::from_ref(done), cap);
    assert_eq!(wire.lines().count(), 1, "{wire:?}");
    parse_results(&wire).remove(0)
}

/// The desktop tour's Files window (`03-files`): `notes.txt`, `plan.txt` and
/// `ideas.txt` were created, `fs_list` answered all three, and the app was
/// told only the first line. Through the shell, `desktop` and `comphost`, an
/// app's own listing and a person's both reach it whole.
#[test]
fn a_folder_of_three_files_reaches_the_app_with_all_three() {
    let listing = "ideas.txt\nnotes.txt\nplan.txt\n";
    for user in [false, true] {
        let got = through("files-window", user, &[ok(listing)]);
        assert_eq!(got, alloc::vec![ok(listing)], "user {user}");
    }
    // With a folder in it, and among other results of the same reply.
    let mixed = alloc::vec![ok("Done: x"), ok("docs/\nnotes.txt\n"), ok("plain")];
    assert_eq!(through("files-window", false, &mixed), mixed);
}

#[test]
fn a_listing_arrives_with_every_name() {
    let listing = "ideas.txt\nnotes.txt\nplan.txt\n";
    let wire = encode_results(&[ok(listing)]);
    assert_eq!(wire, "7\tokm\tideas.txt\\nnotes.txt\\nplan.txt\\n");
    assert_eq!(parse_results(&wire), alloc::vec![ok(listing)]);
    // One line, with or without its newline, is as it was before.
    assert_eq!(encode_results(&[ok("Done: x")]), "7\tok\tDone: x");
    assert_eq!(encode_results(&[ok("Done: x\n")]), "7\tok\tDone: x");
    assert_eq!(encode_results(&[ok("")]), "7\tok\t");
}

#[test]
fn several_lines_keep_their_backslashes_tabs_returns_and_letters() {
    for text in [
        "a\\nb\nc",
        "back\\slash\nand\ttab\r\nand\rreturn",
        "é\u{2028}\nfolder/\nname with spaces ",
        "\\\n\\\\\n\\n\n\\",
        "ends in a backslash\\\nsecond",
    ] {
        let done = ok(text);
        assert_eq!(through_the_wire(&done, MAX_LINE_TEXT), done, "{text:?}");
    }
    let err = Done {
        ticket: 2,
        outcome: Err(String::from("no\nsuch\trow")),
    };
    assert_eq!(through_the_wire(&err, MAX_TEXT), err);
}

#[test]
fn a_long_listing_is_cut_between_names_never_inside_one() {
    let names: Vec<String> = (0..60).map(|i| format!("file-number-{i:03}.txt")).collect();
    let listing = format!("{}\n", names.join("\n"));
    let got = through_the_wire(&ok(&listing), MAX_TEXT);
    let Ok(text) = got.outcome else {
        panic!("an ok result");
    };
    assert!(text.len() < MAX_TEXT, "{} bytes", text.len());
    assert!(text.ends_with('\n'));
    let kept: Vec<&str> = text.lines().collect();
    assert!(kept.len() > 5 && kept.len() < names.len());
    assert!(kept.iter().zip(&names).all(|(kept, name)| kept == name));
    // A first line longer than the cap is cut inside, at a character.
    let long = format!("{}\nsecond", "é".repeat(MAX_TEXT));
    let Ok(text) = through_the_wire(&ok(&long), MAX_TEXT).outcome else {
        panic!("an ok result");
    };
    assert!(text.len() <= MAX_TEXT && text.chars().all(|c| c == 'é'));
    // Nor is an escape ever cut in two.
    let slashes = format!("{}\nx", "\\".repeat(MAX_TEXT));
    let Ok(text) = through_the_wire(&ok(&slashes), MAX_TEXT).outcome else {
        panic!("an ok result");
    };
    assert!(text.chars().all(|c| c == '\\') && text.len() == MAX_TEXT / 2);
}

#[test]
fn the_budget_counts_a_listing_as_it_goes_on_the_wire() {
    let mut queue = Queue::default();
    let big = ok(&"n
"
    .repeat(2000));
    // The budget holds the listing as cut and sent, and not a byte more.
    let size = encode_results(core::slice::from_ref(&big)).len() + 1;
    queue.finish("files", big.clone());
    queue.finish("files", ok("Done: x"));
    let taken = queue.results("files", size);
    assert_eq!(
        taken,
        alloc::vec![big],
        "the second waits for the next call"
    );
    assert_eq!(queue.results("files", size), alloc::vec![ok("Done: x")]);
}

#[test]
fn an_escape_the_reader_does_not_know_stays_as_it_is() {
    for (wire, text) in [
        ("7\tokm\ta\\qb\\", "a\\qb\\"),
        ("7\tokm\t\\\\n", "\\n"),
        ("7\tokm\t", ""),
        ("7\tokm\t\\\\", "\\"),
    ] {
        assert_eq!(parse_results(wire), alloc::vec![ok(text)], "{wire:?}");
    }
    // A word the reader does not know is skipped, not guessed at.
    assert_eq!(parse_results("7\tokx\ta"), Vec::new());
    assert_eq!(parse_results("7\tOKM\ta"), Vec::new());
    // Only a text of several lines is written with the m.
    assert!(!encode_results(&[ok("a b")]).contains("okm"));
}

#[test]
fn any_text_goes_through_and_comes_out_the_same_or_cut_to_whole_lines() {
    // A fixed xorshift, so a failure is the same failure next time.
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let alphabet = ['a', 'b', ' ', '\n', '\n', '\\', '\t', '\r', 'é', 'n', 't'];
    for _ in 0..2000 {
        let len = (next() % 400) as usize;
        let text: String = (0..len)
            .map(|_| alphabet[(next() % alphabet.len() as u64) as usize])
            .collect();
        let cap = 1 + (next() % 300) as usize;
        let got = through_the_wire(&ok(&text), cap);
        let Ok(got) = got.outcome else {
            panic!("an ok result");
        };
        if text.lines().nth(1).is_none() {
            // One line: as `one_line` always did.
            assert_eq!(
                got.replace(' ', "\t"),
                one_line(&text, cap).replace(' ', "\t")
            );
            continue;
        }
        assert!(text.starts_with(&got), "{text:?} {got:?}");
        // What is kept ends where a line ends, or is all there is.
        assert!(got == text || got.ends_with('\n') || !got.contains('\n'));
    }
}

fn notice() -> Done {
    Done {
        ticket: NOTICE_TICKET,
        outcome: Ok(String::from(kabi::live::NOTICE_FILES)),
    }
}

fn answer(ticket: u32) -> Done {
    Done {
        ticket,
        outcome: Ok(String::from("list")),
    }
}

#[test]
fn a_notice_waits_once_however_many_came_and_beside_the_results() {
    let mut queue = Queue::default();
    queue.finish("files-window", notice());
    queue.finish("files-window", answer(3));
    queue.finish("files-window", notice());
    queue.finish("monitor-window", notice());
    let waiting = queue.results("files-window", MESSAGE_BUDGET);
    let notices = waiting.iter().filter(|d| d.ticket == NOTICE_TICKET);
    assert_eq!(notices.count(), 1);
    assert_eq!(waiting.len(), 2);
    // Another app's notice is its own.
    assert_eq!(queue.results("monitor-window", MESSAGE_BUDGET), [notice()]);
    // Once taken, the next change is told again.
    queue.finish("files-window", notice());
    assert_eq!(queue.results("files-window", MESSAGE_BUDGET), [notice()]);
}

#[test]
fn a_notice_is_split_from_the_results_and_a_refusal_under_its_ticket_is_not_one() {
    let refused = Done {
        ticket: NOTICE_TICKET,
        outcome: Err(String::from("the effect is too long")),
    };
    let (rest, told) = split_notice(alloc::vec![answer(2), notice(), notice()]);
    assert_eq!(rest, [answer(2)]);
    assert_eq!(told.as_deref(), Some(kabi::live::NOTICE_FILES));
    let (rest, told) = split_notice(alloc::vec![refused, answer(2)]);
    assert_eq!(rest, [answer(2)]);
    assert_eq!(told, None);
}

#[test]
fn a_notice_is_no_answer_to_a_ticket_the_app_sent() {
    let mut sent = Sent::default();
    sent.record(&line(5));
    let (rest, _) = split_notice(alloc::vec![notice(), answer(5)]);
    let (accepted, stray) = sent.accept(rest);
    assert_eq!(accepted, [answer(5)]);
    assert!(stray.is_empty());
    assert!(!sent.awaits());
}
