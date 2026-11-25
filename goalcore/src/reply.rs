//! What the shell answers the `goals` agent's steps (#1134's wire), read here so
//! the parse is tested on the host. The other way, what goals reports to the
//! shell, is [`crate::report`].
//!
//! ```text
//! agent-step goal=<id> <call>   ok <tier> #<entry>: <text>      (ran)
//!                               err <tier> #<entry>: <text>     (failed)
//!                               ok asked <entry>: on the person's card
//!                               err refused: <why>              (the ceiling)
//! agent-step wait <entry>       ok waiting #<entry>
//!                               ok ran #<entry>: <line>
//!                               err not run #<entry>: <line>    (a no)
//!                               err unknown step #<entry>       (a no)
//! ```

use crate::StepDone;
use alloc::format;
use alloc::string::{String, ToString};

/// What one shell reply says of a step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    /// On the person's card, as entry `n`: its answer comes by `agent-step wait`.
    Asked(u64),
    /// Still on the card.
    Waiting,
    /// The person's no, or no answer kept for the entry: the step did not run.
    NotRun(String),
    /// The goal's ceiling refused it: the shell restarted (it keeps no grants) or
    /// the goal was revoked.
    Refused(String),
    /// It ran, or it failed; the entry is the actlog's.
    Done(StepDone),
}

/// Reads one reply to `agent-step goal=…` or `agent-step wait …`.
pub fn read(reply: &str) -> Reply {
    if let Some(rest) = reply.strip_prefix("ok asked ") {
        return match rest.split_once(':').map(|(n, _)| n.trim().parse::<u64>()) {
            Some(Ok(n)) => Reply::Asked(n),
            _ => Reply::Done(failed(None, format!("an ask with no entry: {reply}"))),
        };
    }
    if reply.starts_with("ok waiting #") {
        return Reply::Waiting;
    }
    if let Some(why) = reply.strip_prefix("err not run ") {
        return Reply::NotRun(why.to_string());
    }
    if reply.starts_with("err unknown step #") {
        return Reply::NotRun(String::from("no answer is kept for it"));
    }
    if reply.starts_with("err refused") {
        return Reply::Refused(reply.to_string());
    }
    Reply::Done(done(reply))
}

/// `ok <tier> #<entry>: …` or `ok ran #<entry>: …` ran; anything else failed.
fn done(reply: &str) -> StepDone {
    let (ok, rest) = match reply.split_once(' ') {
        Some(("ok", rest)) => (true, rest),
        Some((_, rest)) => (false, rest),
        None => (false, reply),
    };
    let entry = rest
        .split_once('#')
        .and_then(|(_, after)| after.split(':').next()?.trim().parse::<u64>().ok());
    match ok {
        true => StepDone {
            entry,
            tokens: 0,
            failed: None,
        },
        false => failed(entry, rest.to_string()),
    }
}

/// A step that failed, with why.
pub fn failed(entry: Option<u64>, why: String) -> StepDone {
    StepDone {
        entry,
        tokens: 0,
        failed: Some(why),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ask_names_its_entry_and_one_without_a_number_failed() {
        assert_eq!(read("ok asked 17: on the person's card"), Reply::Asked(17));
        assert_eq!(read("ok asked 3:"), Reply::Asked(3));
        for bad in ["ok asked x: on the card", "ok asked 17", "ok asked : card"] {
            assert!(
                matches!(
                    read(bad),
                    Reply::Done(StepDone {
                        failed: Some(_),
                        ..
                    })
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_wait_reads_as_still_waiting_ran_or_not_run() {
        assert_eq!(read("ok waiting #17"), Reply::Waiting);
        assert_eq!(
            read("ok ran #17: trashed old.txt"),
            Reply::Done(StepDone {
                entry: Some(17),
                tokens: 0,
                failed: None
            })
        );
        assert_eq!(
            read("err not run #17: you said no"),
            Reply::NotRun("#17: you said no".to_string())
        );
        assert!(matches!(read("err unknown step #17"), Reply::NotRun(_)));
    }

    #[test]
    fn a_step_reply_ran_failed_or_was_refused() {
        assert_eq!(
            read("ok run #4: 4 CPUs"),
            Reply::Done(StepDone {
                entry: Some(4),
                tokens: 0,
                failed: None
            })
        );
        assert_eq!(
            read("err ask #5: no such file"),
            Reply::Done(failed(Some(5), "ask #5: no such file".to_string()))
        );
        assert!(matches!(
            read("err refused: the agent's grants do not include that action"),
            Reply::Refused(_)
        ));
        assert_eq!(read("err"), Reply::Done(failed(None, "err".to_string())));
    }
}
