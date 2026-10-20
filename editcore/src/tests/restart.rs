//! A build the edit service before this one left in the kernel.

use super::fake::{Fake, Job, kernel, say, service};
use super::numbered;
use crate::journal::Journal;
use alloc::format;
use alloc::vec;
use alloc::vec::Vec;
use kabi::Error;

const SYNC: &str = "sync 3\ncomponent notes\nlabel e1\noutcome commit\ngeneration 1\nsource -\ncommit 1a2b3c4d5e6f0123456789abcdef0123456789ab\n";

fn left(env: &mut Fake, reply: Result<Vec<u8>, Error>, busy: u32) {
    env.job = Some(Job { reply, busy });
}

#[test]
fn a_job_from_before_a_restart_keeps_builds_and_syncs_busy() {
    let mut env = Fake::new("x\n");
    left(&mut env, Ok(Vec::new()), 9);
    let mut fresh = service();
    assert_eq!(kernel(&mut fresh, &mut env, SYNC), "err busy building");
    kernel(&mut fresh, &mut env, "open notes");
    kernel(&mut fresh, &mut env, "cmd c 1 y");
    assert_eq!(
        kernel(&mut fresh, &mut env, "cmd build"),
        "err busy building e1"
    );
    assert_eq!(kernel(&mut fresh, &mut env, SYNC), "err busy building e1");
    assert!(env.requests.is_empty());
}

#[test]
fn an_inherited_failed_exchange_is_logged_and_cleared() {
    for decided in [false, true] {
        let mut env = Fake::new("x\n");
        left(&mut env, Err(Error::Network), 0);
        let mut fresh = service();
        let first = if decided { "cmd status" } else { SYNC };
        kernel(&mut fresh, &mut env, first);
        assert_eq!(
            env.logs,
            ["a build from before the restart failed (Network); build again"]
        );
        assert!(env.job.is_none() && !fresh.polling());
        assert!(kernel(&mut fresh, &mut env, SYNC).starts_with("ok synced 3 "));
    }
}

#[test]
fn a_finished_job_waits_for_the_first_command_after_a_restart() {
    let mut env = Fake::new(&numbered(12, "\n"));
    let (mut live, mut journal) = (service(), Journal::new());
    say(&mut live, &mut env, &mut journal, "open notes");
    for _ in 0..300 {
        say(&mut live, &mut env, &mut journal, "cmd find line 1");
    }
    let messages = journal.messages(journal.len());
    assert!(messages.len() >= 2);
    left(&mut env, Ok(Vec::new()), 0);
    env.polls.clear();
    let mut fresh = service();
    for (index, message) in messages.iter().enumerate() {
        let reply = fresh.handle(&mut env, 0, message);
        assert!(reply.starts_with("ok replayed "), "{reply}");
        // The kernel's git sync may come between two replay messages.
        if index == 0 {
            kernel(&mut fresh, &mut env, SYNC);
        }
    }
    assert!(env.polls.iter().all(|took| !took), "{:?}", env.polls);
    assert!(env.job.is_some());
    kernel(&mut fresh, &mut env, "cmd p 1");
    assert_eq!(env.polls.last(), Some(&true));
    assert!(env.job.is_none());
}

#[test]
fn a_replay_into_a_live_service_takes_nothing_until_the_next_command() {
    let mut env = Fake::new(&numbered(3, "\n"));
    let (mut live, mut journal) = (service(), Journal::new());
    for message in ["open notes", "cmd c 1 x"] {
        say(&mut live, &mut env, &mut journal, message);
    }
    say(&mut live, &mut env, &mut journal, "cmd build");
    let polls = env.polls.len();
    for message in journal.messages(journal.len()) {
        live.handle(&mut env, 0, &message);
    }
    assert_eq!(env.polls.len(), polls);
    assert!(env.job.is_some() && live.polling());
    say(&mut live, &mut env, &mut journal, "cmd status");
    assert_eq!(env.polls.last(), Some(&true));
    assert!(env.job.is_none() && env.puts.len() == 1);
}

const BUILT: &str = "ok built e1 from 2 (commit 1a2b3c4d5e6f); :apply runs it";

/// A live service that opened notes, made `edits`, built and kept answering:
/// its journal, with the build left in the kernel, done after `busy` polls.
fn killed_while_building(env: &mut Fake, edits: &[&str], after: &[&str], busy: u32) -> Journal {
    let (mut live, mut journal) = (service(), Journal::new());
    say(&mut live, env, &mut journal, "open notes");
    for edit in edits {
        say(&mut live, env, &mut journal, edit);
    }
    env.busy_polls = 99;
    say(&mut live, env, &mut journal, "cmd build");
    for message in after {
        say(&mut live, env, &mut journal, message);
    }
    env.job.as_mut().unwrap().busy = busy;
    env.polls.clear();
    journal
}

/// The journal replayed into a fresh service, with a git sync between messages.
fn replayed(env: &mut Fake, journal: &Journal) -> crate::service::Service {
    let mut fresh = service();
    for message in journal.messages(journal.len()) {
        assert!(fresh.handle(env, 0, &message).starts_with("ok replayed "));
        kernel(&mut fresh, env, SYNC);
    }
    fresh
}

fn finds() -> Vec<&'static str> {
    let mut edits = vec!["cmd c 1 x"];
    edits.extend(core::iter::repeat_n("cmd find line", 300));
    edits
}

#[test]
fn a_restarted_service_adopts_a_job_built_from_the_replayed_text() {
    let mut env = Fake::new(&numbered(12, "\n"));
    let mut journal = killed_while_building(&mut env, &finds(), &[], 0);
    assert!(journal.messages(journal.len()).len() >= 2);
    let mut fresh = replayed(&mut env, &journal);
    assert!(env.polls.iter().all(|took| !took) && env.job.is_some());
    say(&mut fresh, &mut env, &mut journal, "cmd p 1");
    let status = say(&mut fresh, &mut env, &mut journal, "cmd status");
    assert_eq!(
        status,
        format!("ok notes 2: edited, built e1\nlast build: {BUILT}")
    );
    assert_eq!(env.logs.last().map(|log| log.as_str()), Some(&BUILT[3..]));
    let (_, bytes, base, _) = &env.puts[0];
    assert_eq!((base.as_str(), env.puts.len()), ("2", 1));
    // The journal heard of it from the store: a second restart keeps it built.
    env.artifacts.push(("e1", bytes.clone()));
    let mut again = service();
    let restore = journal.restore(&mut |m| Some(again.handle(&mut env, 0, m).into_bytes()));
    assert!(restore.lost.is_empty(), "{}", restore.notice);
    assert_eq!(again.session.unwrap().built.as_deref(), Some("e1"));
}

#[test]
fn a_restarted_service_drops_a_job_from_other_text() {
    let lib = numbered(3, "\n");
    // An edit answered after the build: the replayed text is not what was built.
    let mut env = Fake::new(&lib);
    let journal = killed_while_building(&mut env, &["cmd c 1 x"], &["cmd c 2 y"], 0);
    let mut fresh = replayed(&mut env, &journal);
    let status = kernel(&mut fresh, &mut env, "cmd status");
    assert!(
        status.ends_with("last build: err dropped e1: it was built before the edit service restarted, from other text"),
        "{status}"
    );
    // No session came back.
    let mut env = Fake::new(&lib);
    killed_while_building(&mut env, &["cmd c 1 x"], &[], 0);
    let status = kernel(&mut service(), &mut env, "cmd status");
    assert!(status.ends_with("and no session came back"), "{status}");
    assert!(env.puts.is_empty() && env.job.is_none());
}

#[test]
fn an_inherited_failed_build_only_logs() {
    let mut env = Fake::new(&numbered(3, "\n"));
    env.host = super::fake::Host::Fails;
    let journal = killed_while_building(&mut env, &["cmd c 3 x"], &[], 0);
    let mut fresh = replayed(&mut env, &journal);
    let line = fresh.session.as_ref().unwrap().line();
    let reply = kernel(&mut fresh, &mut env, "cmd status");
    assert!(!reply.starts_with("#at "), "{reply}");
    assert_eq!(fresh.session.as_ref().unwrap().line(), line);
    assert_eq!(
        env.logs.last().map(|log| log.as_str()),
        Some("a build from before the restart failed; build again")
    );
}

#[test]
fn an_adopted_build_after_later_edits_is_stored_but_not_built() {
    let mut env = Fake::new(&numbered(3, "\n"));
    let journal = killed_while_building(&mut env, &["cmd c 1 x"], &[], 99);
    let mut fresh = replayed(&mut env, &journal);
    env.job.as_mut().unwrap().busy = 2;
    kernel(&mut fresh, &mut env, "cmd status");
    kernel(&mut fresh, &mut env, "cmd c 2 later");
    let status = kernel(&mut fresh, &mut env, "cmd status");
    assert!(
        status.ends_with(&format!("{BUILT}; your edits since are not in it")),
        "{status}"
    );
    assert_eq!(env.puts.len(), 1);
    assert_eq!(fresh.session.unwrap().built, None);
}
