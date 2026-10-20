//! A build the edit service before this one left in the kernel.

use super::fake::{Fake, Job, kernel, say, service};
use super::numbered;
use crate::journal::Journal;
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
