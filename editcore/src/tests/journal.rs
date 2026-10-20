use super::fake::{Fake, Host, TOKEN};
use super::*;
use crate::ipc::{Ipc, decode_ipc, encode_ipc};
use crate::journal::{Journal, MAX_JOURNAL, Restore, digest};
use crate::service::Service;
use crate::wire::SyncRequest;
use livecore::uploads::Outcome;

const OPENED: &str = "ok editing notes 2: 2 files; components/notes/src/lib.rs, 12 lines";

fn service() -> Service {
    Service::new("10.0.2.2:5000", TOKEN)
}

/// Sends `message` as the kernel does, journaling the answer.
fn say(service: &mut Service, env: &mut Fake, journal: &mut Journal, message: &str) -> String {
    let reply = service.handle(env, 0, message.as_bytes());
    journal.record(message.as_bytes(), reply.as_bytes());
    reply
}

/// Gives a fresh service the journal's session, as the kernel does.
fn restore(journal: &mut Journal, env: &mut Fake) -> (Service, Restore) {
    let mut fresh = service();
    let restore = journal.restore(&mut |m| Some(fresh.handle(env, 0, m).into_bytes()));
    (fresh, restore)
}

/// The two sessions hold the same text, cursor and built upload, and undo the
/// same edits in the same order.
fn assert_same(live: &Service, replayed: &Service) {
    let (a, b) = (live.session.clone(), replayed.session.clone());
    let (mut a, mut b) = (a.unwrap(), b.unwrap());
    assert_eq!((&a.label, &a.built), (&b.label, &b.built));
    loop {
        assert_eq!(
            (a.to_bundle(), a.line(), &a.current().path),
            (b.to_bundle(), b.line(), &b.current().path)
        );
        match (a.undo(), b.undo()) {
            (true, true) => {}
            (false, false) => break,
            other => panic!("the undo depths differ: {other:?}"),
        }
    }
}

fn record(journal: &mut Journal, message: &str, reply: &str) -> Option<String> {
    journal.record(message.as_bytes(), reply.as_bytes())
}

#[test]
fn the_journal_keeps_the_open_and_each_command_answered_ok() {
    let mut journal = Journal::new();
    // Nothing before an open.
    record(&mut journal, "cmd c 1 x", "ok lib.rs:1: x");
    assert!(journal.is_empty());
    record(&mut journal, "open notes", OPENED);
    assert_eq!(journal.session(), Some(("notes", "2")));
    // Never a sync.
    let sync = encode_ipc(&Ipc::Sync {
        seq: 1,
        job: SyncRequest {
            component: "notes",
            label: "2",
            outcome: Outcome::Commit,
            generation: 1,
            source: "",
            commit: "",
        },
    });
    journal.record(&sync, b"ok untracked 1");
    assert_eq!(journal.len(), 1);
    for (message, reply) in [
        ("cmd s |a|b|", "ok lib.rs:3: b"),
        ("cmd s |a|b|", "err \"a\" is not in lib.rs"),
        ("cmd p", "ok lib.rs lines 3 to 12 of 12"),
        ("cmd help", "ok commands:"),
        ("cmd status", "ok notes 2"),
        ("cmd try", "ok try e1: import ok"),
        ("cmd build", "err nothing changed since notes 2"),
        (
            "cmd build",
            "err build failed; the cursor is on the first error\nerror[E0277]: x\ncomponents/notes/src/lib.rs:4:5: error[E0277]: y",
        ),
        (
            "cmd build",
            "ok built e1 from 2 (commit 1a2b3c4d5e6f); :apply runs it",
        ),
        ("cmd undo", "ok undone; lib.rs:1: a"),
    ] {
        record(&mut journal, message, reply);
    }
    assert_eq!(
        journal.commands(),
        [
            ":edit notes 2",
            "s |a|b|",
            "p",
            "(a build failed; the cursor went to components/notes/src/lib.rs:4)",
            "(built e1)",
            "undo",
        ]
    );
    assert_eq!((journal.edits(), journal.counted()), (2, "2 edits".into()));
}

#[test]
fn reset_and_another_version_start_it_again_and_a_resume_does_not() {
    let mut journal = Journal::new();
    record(&mut journal, "open notes", OPENED);
    record(&mut journal, "cmd c 1 x", "ok lib.rs:1: x");
    record(&mut journal, "cmd reset", "ok back to notes 2 as opened");
    record(&mut journal, "cmd c 2 y", "ok lib.rs:2: y");
    let resumed = "ok editing notes 2 at components/notes/src/lib.rs:5, with your edits";
    record(
        &mut journal,
        "open notes at components/notes/src/lib.rs:5",
        resumed,
    );
    record(&mut journal, "open notes e9", "err no version notes e9");
    assert_eq!(
        journal.commands(),
        [
            ":edit notes 2",
            "reset",
            "c 2 y",
            ":edit notes 2 (at components/notes/src/lib.rs:5)",
        ]
    );
    let other = "ok editing notes e1: 2 files; components/notes/src/lib.rs, 12 lines; dropped the unbuilt edits to 2";
    record(&mut journal, "open notes e1", other);
    assert_eq!(journal.commands(), [":edit notes e1"]);
    assert_eq!(journal.session(), Some(("notes", "e1")));
}

#[test]
fn past_128_kib_it_warns_once_and_stops_until_it_starts_again() {
    let mut journal = Journal::new();
    record(&mut journal, "open notes", OPENED);
    let line = format!("cmd c 1 {}", "x".repeat(1000));
    let mut warnings = Vec::new();
    for _ in 0..200 {
        warnings.extend(record(&mut journal, &line, "ok lib.rs:1: x"));
    }
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].starts_with("the session journal is full (128 KiB)"));
    let held = journal.len();
    assert!(held < 200 && held * 1017 < MAX_JOURNAL);
    // A reset starts it again.
    assert_eq!(
        record(&mut journal, "cmd reset", "ok back to notes 2 as opened"),
        None
    );
    assert_eq!(record(&mut journal, &line, "ok lib.rs:1: x"), None);
    assert_eq!(journal.len(), 3);
    // An entry no replay message can hold is never recorded either.
    let long = format!("cmd c 1 {}", "y".repeat(kabi::MAX_MESSAGE));
    assert!(record(&mut journal, &long, "ok lib.rs:1: y").is_some());
    assert_eq!(journal.len(), 3);
}

#[test]
fn the_edit_wire_carries_a_replay() {
    let replay = Ipc::Replay {
        from: 3,
        entries: "a\nb",
    };
    assert_eq!(encode_ipc(&replay), b"replay 3\na\nb");
    assert_eq!(decode_ipc(b"replay 3\na\nb"), Ok(replay));
    assert!(decode_ipc(b"replay x\na").is_err());
    assert!(decode_ipc(b"replay 3").is_err());
}

#[test]
fn a_replay_matches_the_live_session_text_cursor_undo_and_build() {
    let mut env = Fake::new(&numbered(12, "\n"));
    let (mut live, mut journal) = (service(), Journal::new());
    let mut say = |live: &mut Service, env: &mut Fake, message: &str| {
        say(live, env, &mut journal, message);
    };
    for message in [
        "open notes",
        "cmd s |line 2|line two|",
        "cmd p 5",
        "cmd i 3 inserted",
        "cmd d 7",
        "cmd undo",
        "cmd c 1 first",
        "cmd bogus",
        "cmd help",
    ] {
        say(&mut live, &mut env, message);
    }
    env.host = Host::Fails;
    say(&mut live, &mut env, "cmd build");
    say(&mut live, &mut env, "cmd p");
    env.host = Host::Builds;
    say(&mut live, &mut env, "cmd build");
    let built = env.puts[0].1.clone();
    env.artifacts.push(("e1", built));
    for _ in 0..300 {
        say(&mut live, &mut env, "cmd find line 1");
    }
    say(&mut live, &mut env, "cmd try");
    let messages = journal.messages(journal.len());
    assert!(messages.len() >= 2);
    assert!(messages.iter().all(|m| m.len() <= kabi::MAX_MESSAGE));
    let (requests, controls) = (env.requests.len(), env.controls.len());
    let (replayed, restore) = restore(&mut journal, &mut env);
    assert_eq!(
        restore,
        Restore {
            notice: "the edit service restarted; restored notes 2 with 5 edits".into(),
            lost: Vec::new(),
            again: false,
        }
    );
    // Nothing reached the build service or notes.
    assert_eq!(
        (env.requests.len(), env.controls.len()),
        (requests, controls)
    );
    assert_same(&live, &replayed);
}

#[test]
fn a_replay_stops_at_a_digest_that_differs_and_keeps_what_came_before() {
    let mut env = Fake::new(&numbered(12, "\n"));
    let (mut live, mut journal) = (service(), Journal::new());
    for message in ["open notes", "cmd c 1 one", "cmd c 2 two", "cmd c 3 three"] {
        say(&mut live, &mut env, &mut journal, message);
    }
    let mut fresh = service();
    let mut send = |message: &[u8]| {
        // The third entry's digest is corrupted on the way.
        let text = String::from_utf8_lossy(message).replace(
            &format!(
                "{:016x} cmd c 2",
                digest(format!("ok {LIB}:2: two").as_bytes())
            ),
            "0000000000000000 cmd c 2",
        );
        Some(fresh.handle(&mut env, 0, text.as_bytes()).into_bytes())
    };
    let restore = journal.restore(&mut send);
    assert!(
        restore.notice.starts_with(
            "the edit service restarted; restored notes 2 with 1 edit; the rest did not replay (it now answers \"ok "
        ),
        "{}",
        restore.notice
    );
    assert_eq!(restore.lost, ["c 2 two", "c 3 three"]);
    assert_eq!(journal.commands(), [":edit notes 2", "c 1 one"]);
    let mut once = service();
    for message in ["open notes", "cmd c 1 one"] {
        once.handle(&mut env, 0, message.as_bytes());
    }
    assert_same(&once, &fresh);
    // A base that is gone restores nothing.
    env.artifacts.clear();
    let (replayed, restore) = self::restore(&mut journal, &mut env);
    assert_eq!(
        restore.notice,
        "the edit service restarted; could not restore notes 2 (it now answers \"err no version notes 2\"); :journal lists it"
    );
    assert_eq!(restore.lost, [":edit notes 2", "c 1 one"]);
    assert!(journal.is_empty() && replayed.session.is_none());
}

#[test]
fn a_replay_cannot_build_or_reach_a_component() {
    let mut env = Fake::new(&numbered(3, "\n"));
    let (mut live, mut journal) = (service(), Journal::new());
    for message in ["open notes", "cmd c 1 x", "cmd build"] {
        say(&mut live, &mut env, &mut journal, message);
    }
    let built = env.puts[0].1.clone();
    env.artifacts.push(("e1", built));
    let before = (env.requests.len(), env.controls.len(), env.puts.len());
    let entries = String::from_utf8(journal.messages(2).remove(0)).unwrap();
    let all = String::from_utf8(journal.messages(3).remove(0)).unwrap();
    for (message, stopped) in [
        // An edit, then a build: it never reaches the build service.
        (format!("{entries}\n{:016x} cmd build", 0), 2),
        // Built, then a try: it never reaches notes.
        (format!("{all}\n{:016x} cmd try", 0), 3),
    ] {
        let reply = service().handle(&mut env, 0, message.as_bytes());
        let lead = format!("err replay stopped at {stopped}: it now answers ");
        assert!(reply.starts_with(&lead), "{reply}");
    }
    let after = (env.requests.len(), env.controls.len(), env.puts.len());
    assert_eq!(before, after);
}

#[test]
fn built_comes_back_only_for_a_stored_upload_of_this_text() {
    let mut env = Fake::new(&numbered(3, "\n"));
    let (mut live, mut journal) = (service(), Journal::new());
    for message in ["open notes", "cmd c 1 x", "cmd build"] {
        say(&mut live, &mut env, &mut journal, message);
    }
    let built = env.puts[0].1.clone();
    let other = artifact("notes", "e1", &small("other\n"));
    for (stored, wanted) in [(Some(built), Some("e1")), (Some(other), None), (None, None)] {
        env.artifacts.retain(|(label, _)| *label != "e1");
        env.artifacts.extend(stored.map(|bytes| ("e1", bytes)));
        let (replayed, restore) = restore(&mut journal, &mut env);
        assert!(restore.lost.is_empty(), "{}", restore.notice);
        let session = replayed.session.unwrap();
        assert_eq!(session.built.as_deref(), wanted);
    }
}

#[test]
fn a_replay_into_a_fresh_service_ignores_the_note_about_dropped_edits() {
    let mut env = Fake::new(&numbered(3, "\n"));
    env.artifacts
        .push(("e1", artifact("notes", "e1", &small("e1\n"))));
    let (mut live, mut journal) = (service(), Journal::new());
    for message in ["open notes", "cmd c 1 x", "open notes e1", "cmd c 1 y"] {
        say(&mut live, &mut env, &mut journal, message);
    }
    assert_eq!(journal.commands(), [":edit notes e1", "c 1 y"]);
    let (replayed, restore) = restore(&mut journal, &mut env);
    assert!(restore.lost.is_empty(), "{}", restore.notice);
    assert_same(&live, &replayed);
}

#[test]
fn a_replay_with_no_answer_is_tried_once_more_then_given_up() {
    let mut env = Fake::new(&numbered(3, "\n"));
    let (mut live, mut journal) = (service(), Journal::new());
    for message in ["open notes", "cmd c 1 x"] {
        say(&mut live, &mut env, &mut journal, message);
    }
    let restore = journal.restore(&mut |_| None);
    assert!(restore.again && restore.lost.is_empty(), "{restore:?}");
    assert_eq!(journal.len(), 2);
    // The try after that one answers.
    let (replayed, restore) = self::restore(&mut journal, &mut env);
    assert!(!restore.again && restore.lost.is_empty(), "{restore:?}");
    assert_same(&live, &replayed);
    // Silent twice in a row: given up, with every command listed.
    journal.restore(&mut |_| None);
    let restore = journal.restore(&mut |_| None);
    assert!(!restore.again, "{restore:?}");
    assert_eq!(restore.lost, [":edit notes 2", "c 1 x"]);
    assert!(journal.is_empty());
}

#[test]
fn a_replay_from_the_open_replaces_whatever_session_the_service_holds() {
    let mut env = Fake::new(&numbered(3, "\n"));
    let (mut live, mut journal) = (service(), Journal::new());
    for message in ["open notes", "cmd c 1 x", "cmd c 2 y"] {
        say(&mut live, &mut env, &mut journal, message);
    }
    // A service that already took this journal, or that the founder edited
    // without the kernel hearing the answer, ends up as the journal says.
    let mut other = service();
    for message in ["open notes", "cmd c 3 z"] {
        other.handle(&mut env, 0, message.as_bytes());
    }
    for _ in 0..2 {
        let restore = journal.restore(&mut |m| Some(other.handle(&mut env, 0, m).into_bytes()));
        assert!(restore.lost.is_empty(), "{}", restore.notice);
        assert_same(&live, &other);
    }
}

#[test]
fn a_newer_service_that_takes_over_mid_replay_gets_it_all_from_the_open() {
    let mut env = Fake::new(&numbered(12, "\n"));
    let (mut live, mut journal) = (service(), Journal::new());
    say(&mut live, &mut env, &mut journal, "open notes");
    for _ in 0..300 {
        say(&mut live, &mut env, &mut journal, "cmd find line 1");
    }
    say(&mut live, &mut env, &mut journal, "cmd c 1 x");
    assert!(journal.messages(journal.len()).len() >= 2);
    // The first message reaches a service that then dies; the rest reach a new one.
    let (mut dying, mut newer, mut sent) = (service(), service(), 0);
    let restore = journal.restore(&mut |m| {
        sent += 1;
        let to = if sent == 1 { &mut dying } else { &mut newer };
        Some(to.handle(&mut env, 0, m).into_bytes())
    });
    assert!(restore.lost.is_empty(), "{}", restore.notice);
    assert_same(&live, &newer);
}
