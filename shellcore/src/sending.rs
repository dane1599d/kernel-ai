//! The shell's side of the outbox (`outbox`): holding a send after its Allow,
//! starting it when due with its `sending` on disk first, ending it, and
//! cancelling it by `undo`. The OS's loop drives it between lines:
//! [`Shell::start_due_send`], then a report to the panel, then
//! [`Shell::run_send`], so the panel says "Sending…" only once it is.

use crate::actlog::{Entry, Form, State};
use crate::outbox::{self, Ended, Send, Shown, Stage};
use crate::tiers::Tier;
use crate::{Reply, Shell};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

impl Shell {
    /// Holds `send`, which the person allowed for entry `id`'s request
    /// (`call`, as the card showed it): logged as held, with `undo` live, and
    /// due `ms` from now. Code's line for it, naming the entry `undo` takes.
    // The send row's Allow calls it (P5 close, with the row and its
    // transport); until then the outbox's tests drive it.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn hold_send(
        &mut self,
        id: u64,
        request: &str,
        call: &str,
        send: Send,
        ms: u32,
    ) -> Reply {
        let host = send.host.clone();
        if !outbox::is_host(&host) {
            let why = "that is not a host name";
            self.record(id, request, call, &format!("refused: {why}"));
            return Reply::err(format!("Not sent: {why}"));
        }
        let due_ms = self.host.now_ns() / 1_000_000 + u64::from(ms);
        if let Err(why) = self.outbox.hold(id, send, due_ms) {
            self.record(id, request, call, &format!("refused: {why}"));
            return Reply::err(outbox::ended_line(&host, Stage::Unsent, &why));
        }
        let line = outbox::held_line(&host, ms, id);
        self.log.push(Entry {
            id,
            t_ns: self.host.now_ns(),
            form: Form::Call,
            of: None,
            tier: Some(Tier::Ask),
            pack: Some(self.pack_id),
            consent: String::from("asked"),
            state: State::Done,
            request: String::from(request),
            script: String::from(call),
            outcome: line.clone(),
            changes: alloc::vec![outbox::held_change(id, &host)],
        });
        self.host
            .audit(&format!("request {id}: a send to {host} is held {ms} ms"));
        self.save_log();
        Reply::ok(line)
    }

    /// Starts the first held send due at `now_ms`: its entry, once the log
    /// on disk says `sending`. A send whose `sending` could not be written is
    /// logged unsent and never goes. `None` when nothing is due.
    pub fn start_due_send(&mut self, now_ms: u64) -> Option<u64> {
        loop {
            let entry = self.outbox.start_due(now_ms)?;
            let host = self.outbox.get(entry)?.send.host.clone();
            // An entry gone from the log's ring has no `sending` to write
            // first: it never goes.
            if !self.mark_send(entry, Stage::Sending, &format!("sending to {host}")) {
                self.host
                    .audit(&format!("send #{entry} not sent: its log entry is gone"));
                self.end_send(entry, Stage::Unsent, "its log entry is gone");
                continue;
            }
            match self.write_log() {
                Ok(()) => return Some(entry),
                Err(why) => {
                    self.host.audit(&format!(
                        "send #{entry} not sent: the log was not written: {why}"
                    ));
                    self.end_send(entry, Stage::Unsent, "the log could not be written first");
                }
            }
        }
    }

    /// [`Shell::start_due_send`] on a pass of the OS loop, unless that pass
    /// took a panel request (`took_request`): then none starts, and the next
    /// pass takes the request behind it first, so an undo queued after a line
    /// or a Stop wins over the send it names (#959 re-review).
    pub fn start_send_after(&mut self, took_request: bool, now_ms: u64) -> Option<u64> {
        match took_request {
            true => None,
            false => self.start_due_send(now_ms),
        }
    }

    /// Sends what [`Shell::start_due_send`] started for `entry`, once. A
    /// failure after it may have gone out, or a Stop that cut the wait for
    /// the answer, is logged as one that may have been sent.
    pub fn run_send(&mut self, entry: u64) {
        let Some(pending) = self
            .outbox
            .get(entry)
            .filter(|p| p.stage == Stage::Sending)
            .cloned()
        else {
            return;
        };
        let (stage, why) = match self.host.net_send(&pending.send) {
            // Any answer means the site has it, whatever it said.
            Ok(answer) => (
                Stage::Sent,
                String::from(answer.lines().next().unwrap_or("")),
            ),
            Err(failed) if failed.maybe_sent || self.host.interrupted() => {
                (Stage::Unknown, failed.why)
            }
            Err(failed) => (Stage::Failed, failed.why),
        };
        self.end_send(entry, stage, &why);
    }

    /// Every send due at `now_ms`, each started then sent: for a host with no
    /// panel to tell between the two.
    pub fn drain_sends(&mut self, now_ms: u64) {
        while let Some(entry) = self.start_due_send(now_ms) {
            self.run_send(entry);
        }
    }

    /// How long until the next held send is due, from `now_ms`.
    pub fn next_send_ms(&self, now_ms: u64) -> Option<u64> {
        self.outbox.next_due(now_ms)
    }

    /// The sends waiting or going, for the panel.
    pub fn held_sends(&self, now_ms: u64) -> Vec<Shown> {
        self.outbox.shown(now_ms)
    }

    /// What sends came to since the last look, for the panel.
    pub fn take_sends_ended(&mut self) -> Vec<Ended> {
        core::mem::take(&mut self.sends_ended)
    }

    /// `undo` of a held send: cancelled while it still waits, or code's
    /// words for why not. `host` is the change's, for those words.
    pub(crate) fn unsend(&mut self, entry: u64, host: &str) -> Result<String, String> {
        let host = self
            .outbox
            .cancel(entry)
            .map_err(|stage| outbox::cannot_undo(host, stage))?;
        let line = outbox::ended_line(&host, Stage::Cancelled, "");
        if let Some(e) = self.log.get_mut(entry) {
            e.outcome = line.clone();
        }
        self.host
            .audit(&format!("send #{entry} to {host} cancelled by undo"));
        self.sends_ended.push(Ended {
            entry,
            host: host.clone(),
            stage: Stage::Cancelled,
            line,
        });
        Ok(format!("the send to {host}, not sent"))
    }

    /// Why a bare `undo` stops: a send among the entries done since the
    /// newest live one is not held any more (it went, or its cancel was the
    /// last undo). Sends still waiting are named, with the `undo` for each.
    pub(crate) fn bare_send_refusal(&self) -> Option<String> {
        let refusal = self
            .log
            .entries()
            .rev()
            .take_while(|e| !e.live())
            .filter(|e| e.state == State::Done && !e.changes.is_empty())
            .find_map(|e| self.send_refusal(e))?;
        let waiting: Vec<String> = self
            .outbox
            .shown(0)
            .iter()
            .filter(|s| !s.sending)
            .map(|s| format!("{} is still waiting: undo {}", s.host, s.entry))
            .collect();
        Some(match waiting.is_empty() {
            true => refusal,
            false => format!("{refusal}. {}", waiting.join("; ")),
        })
    }

    /// Why `undo` cannot take entry `entry` back, when it is a send that is
    /// not held any more (or the undo of one); `None` for any other entry.
    pub(crate) fn send_refusal(&self, entry: &Entry) -> Option<String> {
        let change = entry.changes.iter().find(|c| c.action == outbox::ACTION)?;
        if change.inverse.is_some() {
            return None;
        }
        if entry.of.is_some() {
            return Some(format!(
                "Can't redo the send to {}: ask again to send it",
                change.res
            ));
        }
        let stage = Stage::of(&change.after).unwrap_or(Stage::Unknown);
        Some(outbox::cannot_undo(&change.res, stage))
    }

    /// Entry `entry`'s send at `stage`: its change's `after=`, no undo once
    /// it left the hold, and the entry's outcome. False when the entry is no
    /// longer in the log.
    fn mark_send(&mut self, entry: u64, stage: Stage, outcome: &str) -> bool {
        let Some(e) = self.log.get_mut(entry) else {
            return false;
        };
        for change in e.changes.iter_mut().filter(|c| c.action == outbox::ACTION) {
            change.after = String::from(stage.word());
            if stage != Stage::Held {
                change.inverse = None;
            }
        }
        e.outcome = String::from(outcome);
        if matches!(stage, Stage::Failed | Stage::Unsent) {
            e.state = State::Nothing;
        }
        true
    }

    /// Ends entry `entry`'s send at `stage`: out of the outbox, logged, and
    /// told at the terminal's next prompt and on the panel.
    fn end_send(&mut self, entry: u64, stage: Stage, why: &str) {
        let Some(pending) = self.outbox.take(entry) else {
            return;
        };
        let host = pending.send.host;
        let line = outbox::ended_line(&host, stage, why);
        self.mark_send(entry, stage, &line);
        self.save_log();
        self.host
            .audit(&format!("send #{entry} to {host}: {}", stage.word()));
        self.notices.push(line.clone());
        self.sends_ended.push(Ended {
            entry,
            host,
            stage,
            line,
        });
    }
}
