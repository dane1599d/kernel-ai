//! The outbox: sends the person allowed, held for a few seconds with `undo`
//! live before they go (ADR 0002 D2's `Hold { ms }`, built for the P5 close
//! on 2026-11-16). Not "held": that word already means the apps' requests
//! held during a line (`waiting`).
//!
//! - A send asks first, always. After Allow it waits here `ms`, and `undo`
//!   (or `undo <id>`) cancels it while it waits.
//! - It goes once, when due: the shell writes `sending` to `/system/log`
//!   before it calls the network, and sends nothing if that write fails.
//! - Once it is sending it has no undo. Undoing a cancel never sends again:
//!   a send always asks again.
//! - The outbox lives in the shell's memory only. A send the log holds as
//!   `held` when the shell starts is logged `unsent` and never goes; one it
//!   holds as `sending` is logged as one that may have been sent.
//!
//! Its words are code's: the model never says that something was sent.

use crate::actlog::{ActLog, Change, State};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Sends waiting at most: a ninth is refused, never queued.
pub const MAX_PENDING: usize = 8;
/// The action a held send's change is logged under: the send row's name.
pub const ACTION: &str = "net_send";
/// Who carries a send out.
pub const OWNER: &str = "fetch";

/// One send, as the row was allowed with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Send {
    pub host: String,
    pub path: String,
    pub body: String,
}

/// Why the network did not take a send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendFailed {
    pub why: String,
    /// The request may have gone out before it failed (a cut after the body
    /// was written): it is logged as one that may have been sent.
    pub maybe_sent: bool,
}

/// A site's answer as the result line says it: the status and a fixed
/// phrase for it (`404 Not Found`), never the server's own reason, which is
/// the site's text, not code's (#962). A status with no phrase here is the
/// number alone.
pub fn status_line(status: u16) -> String {
    let phrase = match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        301 | 302 | 303 | 307 | 308 => "Redirect, not followed",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        410 => "Gone",
        413 => "Content Too Large",
        415 => "Unsupported Media Type",
        422 => "Unprocessable Content",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => return status.to_string(),
    };
    format!("{status} {phrase}")
}

/// A failed call to the send service, as a send's failure. Only a call the
/// kernel refused before the service could take it is "not sent" (a Stop
/// ends a call only while no server has taken it); anything else, a timeout
/// or the service dying with it included, may have gone out. Fault is not
/// "not sent": the kernel copies the reply into the shell's buffer after
/// the service answered, so a fault there comes after the send.
pub fn call_failed(error: kabi::Error) -> SendFailed {
    use kabi::Error as E;
    let maybe_sent = !matches!(
        error,
        E::Permission
            | E::Invalid
            | E::NoMemory
            | E::NotFound
            | E::TooBig
            | E::Unavailable
            | E::BadHandle
            | E::Revoked
            | E::Interrupted
    );
    let why = match maybe_sent {
        true => format!("the send service gave no answer ({error:?})"),
        false => format!("the send service could not be called ({error:?})"),
    };
    SendFailed { why, maybe_sent }
}

/// How long past due a held send waits for the desktop to answer before it
/// is ended unsent (`Shell::end_unchecked_sends`).
pub const UNCHECKED_MS: u64 = 60_000;
/// Why: code's words, after "Not sent to <host>: ".
pub const UNCHECKED_WHY: &str =
    "the desktop did not answer for a minute, so its undo could not be checked";

/// Where a send stands, as its change's `after=` word in the log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Waiting, with `undo` live.
    Held,
    /// Due, and going out: no undo.
    Sending,
    Sent,
    /// `undo` took it back while it waited.
    Cancelled,
    /// The network refused it before it went.
    Failed,
    /// It may have gone out: a cut or a Stop after it started.
    Unknown,
    /// It never went: the shell restarted during the hold, or the log could
    /// not be written first.
    Unsent,
}

impl Stage {
    pub fn word(self) -> &'static str {
        match self {
            Self::Held => "held",
            Self::Sending => "sending",
            Self::Sent => "sent",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
            Self::Unsent => "unsent",
        }
    }

    pub fn of(word: &str) -> Option<Self> {
        [
            Self::Held,
            Self::Sending,
            Self::Sent,
            Self::Cancelled,
            Self::Failed,
            Self::Unknown,
            Self::Unsent,
        ]
        .into_iter()
        .find(|s| s.word() == word)
    }

    /// Whether it is over: it went, or never will.
    pub fn ended(self) -> bool {
        !matches!(self, Self::Held | Self::Sending)
    }
}

/// A send in the outbox, by the log entry that allowed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    pub entry: u64,
    pub send: Send,
    pub due_ms: u64,
    pub stage: Stage,
}

/// What a send came to, for the terminal's next prompt and the panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ended {
    pub entry: u64,
    pub host: String,
    pub stage: Stage,
    /// Code's line for it: `Sent to hooks.example.`
    pub line: String,
}

/// A send the panel shows while it waits or goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shown {
    pub entry: u64,
    pub host: String,
    pub sending: bool,
    /// Until it is due; 0 once it is sending.
    pub left_ms: u64,
}

#[derive(Default)]
pub struct Outbox {
    pending: Vec<Pending>,
}

impl Outbox {
    /// Holds `send` for entry `entry` until `due_ms`; `Err` with code's words
    /// when [`MAX_PENDING`] wait already.
    pub fn hold(&mut self, entry: u64, send: Send, due_ms: u64) -> Result<(), String> {
        if self.pending.len() >= MAX_PENDING {
            return Err(format!(
                "{MAX_PENDING} sends are waiting already; try again once they have gone"
            ));
        }
        self.pending.push(Pending {
            entry,
            send,
            due_ms,
            stage: Stage::Held,
        });
        Ok(())
    }

    pub fn is_full(&self) -> bool {
        self.pending.len() >= MAX_PENDING
    }

    pub fn get(&self, entry: u64) -> Option<&Pending> {
        self.pending.iter().find(|p| p.entry == entry)
    }

    /// Takes entry `entry`'s send back, compare-and-set: only while it is
    /// held. Its host, or why not.
    pub fn cancel(&mut self, entry: u64) -> Result<String, Stage> {
        let at = self
            .pending
            .iter()
            .position(|p| p.entry == entry)
            .ok_or(Stage::Unsent)?;
        match self.pending[at].stage {
            Stage::Held => Ok(self.pending.remove(at).send.host),
            stage => Err(stage),
        }
    }

    /// The first held send due at `now_ms`, now sending: its entry.
    pub fn start_due(&mut self, now_ms: u64) -> Option<u64> {
        let due = self
            .pending
            .iter_mut()
            .find(|p| p.stage == Stage::Held && p.due_ms <= now_ms)?;
        due.stage = Stage::Sending;
        Some(due.entry)
    }

    /// Takes entry `entry`'s send out of the outbox, as it ends.
    /// The held sends more than `by_ms` past due at `now_ms`.
    pub fn overdue(&self, now_ms: u64, by_ms: u64) -> Vec<u64> {
        self.pending
            .iter()
            .filter(|p| p.stage == Stage::Held && p.due_ms.saturating_add(by_ms) <= now_ms)
            .map(|p| p.entry)
            .collect()
    }

    pub fn take(&mut self, entry: u64) -> Option<Pending> {
        let at = self.pending.iter().position(|p| p.entry == entry)?;
        Some(self.pending.remove(at))
    }

    /// The soonest a held send is due, in ms from `now_ms` (0 when one is
    /// due already); `None` with none held.
    pub fn next_due(&self, now_ms: u64) -> Option<u64> {
        self.pending
            .iter()
            .filter(|p| p.stage == Stage::Held)
            .map(|p| p.due_ms.saturating_sub(now_ms))
            .min()
    }

    /// The sends the panel shows, oldest first.
    pub fn shown(&self, now_ms: u64) -> Vec<Shown> {
        self.pending
            .iter()
            .map(|p| Shown {
                entry: p.entry,
                host: p.send.host.clone(),
                sending: p.stage == Stage::Sending,
                left_ms: match p.stage {
                    Stage::Held => p.due_ms.saturating_sub(now_ms),
                    _ => 0,
                },
            })
            .collect()
    }
}

/// `Sending to hooks.example in 5 s. undo 12`: the line a send's Allow ends
/// with, naming the entry `undo` takes back.
pub fn held_line(host: &str, ms: u32, entry: u64) -> String {
    format!("Sending to {host} in {} s. undo {entry}", ms.div_ceil(1000))
}

/// What a send came to, in code's words.
pub fn ended_line(host: &str, stage: Stage, why: &str) -> String {
    match stage {
        // `why` is the site's answer: a 2xx needs no words.
        Stage::Sent if why.is_empty() || why.starts_with('2') => format!("Sent to {host}."),
        Stage::Sent => format!("Sent to {host}; it answered {why}"),
        Stage::Cancelled => format!("Not sent to {host}: undone."),
        Stage::Failed => format!("Not sent to {host}: {why}"),
        Stage::Unknown => format!("It may have been sent to {host}: {why}"),
        Stage::Unsent => format!("Not sent to {host}: {why}"),
        Stage::Held | Stage::Sending => format!("Sending to {host}"),
    }
}

/// Why `undo` cannot take a send back once it is not held any more.
pub fn cannot_undo(host: &str, stage: Stage) -> String {
    match stage {
        Stage::Sending | Stage::Sent => format!("Can't undo: it was sent to {host}"),
        Stage::Unknown => format!("Can't undo: it may have been sent to {host}"),
        Stage::Cancelled | Stage::Failed | Stage::Unsent | Stage::Held => {
            format!("Can't undo: nothing was sent to {host}")
        }
    }
}

/// The longest host a send names (a DNS name's limit).
pub const MAX_HOST: usize = 253;

/// A host as a send may name one: a name or an address, printable, with no
/// space, `;` or `/`. The pane's wire takes the same (`deskcore::sends`).
pub fn is_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= MAX_HOST
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-:[]".contains(&b))
}

/// The log as a shell that starts finds it: every send it holds as `held` is
/// `unsent`, never to go, and every one it holds as `sending` may have gone.
/// None of them has an inverse (the log never keeps one for a send). Whether
/// any changed, so the log is written again.
pub fn settle_at_start(log: &mut ActLog) -> bool {
    let mut changed = false;
    for entry in log.entries_mut() {
        for change in entry.changes.iter_mut().filter(|c| c.action == ACTION) {
            let (stage, why) = match Stage::of(&change.after) {
                Some(Stage::Held) => (Stage::Unsent, "Kai restarted during the hold"),
                Some(Stage::Sending) => (Stage::Unknown, "Kai restarted as it was sending"),
                _ => continue,
            };
            change.after = String::from(stage.word());
            change.inverse = None;
            entry.outcome = ended_line(&change.res, stage, why);
            if stage == Stage::Unsent {
                entry.state = State::Nothing;
            }
            changed = true;
        }
    }
    changed
}

/// The change a held send is logged with: undone by cancelling it while it
/// waits. On disk its inverse is `none` (`undo::inverse_word`).
pub fn held_change(entry: u64, host: &str) -> Change {
    Change {
        action: String::from(ACTION),
        owner: String::from(OWNER),
        res: String::from(host),
        before: String::new(),
        after: String::from(Stage::Held.word()),
        inverse: Some(crate::undo::Inverse::Unsend { entry }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn send(host: &str) -> Send {
        Send {
            host: String::from(host),
            path: String::from("/hook"),
            body: String::from("hello"),
        }
    }

    #[test]
    fn a_send_is_due_once_and_only_when_its_time_comes() {
        let mut outbox = Outbox::default();
        outbox.hold(3, send("a.example"), 5_000).unwrap();
        assert_eq!(outbox.start_due(4_999), None);
        assert_eq!(outbox.next_due(4_000), Some(1_000));
        assert_eq!(outbox.start_due(5_000), Some(3));
        assert_eq!(outbox.start_due(9_000), None, "it started once");
        assert_eq!(outbox.next_due(9_000), None);
    }

    #[test]
    fn cancel_takes_back_only_a_held_send() {
        let mut outbox = Outbox::default();
        outbox.hold(3, send("a.example"), 5_000).unwrap();
        outbox.hold(4, send("b.example"), 5_000).unwrap();
        assert_eq!(outbox.cancel(3), Ok(String::from("a.example")));
        assert_eq!(outbox.cancel(3), Err(Stage::Unsent));
        assert_eq!(outbox.start_due(5_000), Some(4));
        assert_eq!(outbox.cancel(4), Err(Stage::Sending));
    }

    #[test]
    fn the_outbox_refuses_a_ninth_send() {
        let mut outbox = Outbox::default();
        for entry in 0..MAX_PENDING as u64 {
            outbox.hold(entry, send("a.example"), 1).unwrap();
        }
        assert!(outbox.is_full());
        let refused = outbox.hold(99, send("a.example"), 1).unwrap_err();
        assert!(refused.contains("8 sends are waiting"), "{refused}");
        assert!(outbox.get(99).is_none());
    }

    #[test]
    fn the_stage_words_read_back() {
        for stage in [
            Stage::Held,
            Stage::Sending,
            Stage::Sent,
            Stage::Cancelled,
            Stage::Failed,
            Stage::Unknown,
            Stage::Unsent,
        ] {
            assert_eq!(Stage::of(stage.word()), Some(stage));
        }
        assert_eq!(Stage::of("gone"), None);
    }

    #[test]
    fn the_held_line_names_the_entry_and_whole_seconds() {
        assert_eq!(
            held_line("hooks.example", 5_000, 12),
            "Sending to hooks.example in 5 s. undo 12"
        );
        assert_eq!(
            held_line("x.org", 4_500, 1),
            "Sending to x.org in 5 s. undo 1"
        );
    }
}
