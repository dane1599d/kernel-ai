//! What the shell answers the `goals` agent's steps (#1134's wire), read here so
//! the parse is tested on the host. The other way, what goals reports to the
//! shell, is [`crate::report`].
//!
//! ```text
//! agent-step goal=<id> <call>   ok <tier> #<entry>: <text>      (ran)
//!                               err <tier> #<entry>: <text>     (failed)
//!                               ok asked <entry>: on the person's card
//!                               err refused: <why>              (the ceiling)
//!                               err busy: <why>                 (not taken now)
//! agent-step wait <entry>       ok waiting #<entry>
//!                               ok ran #<entry>: <line>
//!                               err failed #<entry>: <line>     (allowed, then failed)
//!                               err not run #<entry>: <line>    (a no)
//!                               err lapsed #<entry>: <line>     (not answered in time)
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
    /// The card was not answered in time: no no, and no run. The step is held,
    /// and may be asked again.
    Lapsed(String),
    /// The goal's ceiling refused it: the shell restarted (it keeps no grants) or
    /// the goal was revoked.
    Refused(String),
    /// The shell could not take it now (`err busy`: a question waits for the
    /// person). The step did not reach the door and made no entry: it is sent
    /// again later, and is no step run or failed (H2-9). (`err not asked` is
    /// not this: that step reached the door and was logged, so it is a step
    /// that failed.)
    Busy(String),
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
    if let Some(why) = reply.strip_prefix("err lapsed ") {
        return Reply::Lapsed(why.to_string());
    }
    if reply.starts_with("err unknown step #") {
        return Reply::NotRun(String::from("no answer is kept for it"));
    }
    if reply.starts_with("err refused") {
        return Reply::Refused(reply.to_string());
    }
    if let Some(why) = reply.strip_prefix("err busy") {
        return Reply::Busy(why.trim_start_matches(':').trim().to_string());
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
        assert_eq!(
            read("err lapsed #17: Not run: it was not answered within two minutes."),
            Reply::Lapsed("#17: Not run: it was not answered within two minutes.".to_string())
        );
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

    /// H2-9: the shell's busy reply is no step run or failed: it was never sent
    /// to the door. A step the shell did take and log stays a failed one.
    #[test]
    fn a_busy_shell_is_not_a_failed_step() {
        assert_eq!(
            read("err busy: a question waits for the person; step again later"),
            Reply::Busy(String::from(
                "a question waits for the person; step again later"
            ))
        );
        assert!(matches!(
            read("err not asked: the agent has 4 questions waiting"),
            Reply::Done(StepDone {
                failed: Some(_),
                ..
            })
        ));
    }
}
