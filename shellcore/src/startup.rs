//! What the shell reads back at start and saves: the log, the rules and the
//! withheld rows.

use crate::{LogFailed, RULES, Shell, actlog, grants, netask, outbox, rules};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

impl Shell {
    /// The rows withheld from apps' effects, checked at boot.
    pub(crate) fn with_saved_withheld(mut self) -> Self {
        grants::check_saved(self.host.as_ref());
        let (high, dropped) = netask::at_boot(self.host.as_ref());
        self.net_high = high;
        self.grant_memo.session_stale.set(!dropped);
        self
    }

    /// The log `/system/log` holds, so `undo` works across a reboot; a text that
    /// is not `actlog 1` is set aside, and the log starts again.
    /// The rules `/system/state/rules` holds. A file refused whole is set
    /// aside: the shell starts with none and never writes over it this run, so
    /// the next rule change cannot make the loss permanent (#10). A rule whose
    /// action is not in the registry is set aside alone, and audited.
    pub(crate) fn with_saved_rules(mut self) -> Self {
        let read = self.host.rules_read();
        let Some(text) = self.read_at_start(RULES, read) else {
            return self;
        };
        match rules::Rules::restore(&text) {
            Ok((rules, set_aside)) => {
                self.rules = rules;
                for why in set_aside {
                    self.host
                        .audit(&format!("a rule on disk was set aside: {why}"));
                }
            }
            Err(why) => {
                self.host.audit(&format!(
                    "the rules on disk were refused ({why}); starting with none, \
                     and the file is not written over"
                ));
                self.unread.push(RULES);
            }
        }
        self
    }

    /// `what`'s file as the host read it at start: `None` with no file, or
    /// with one that could not be read. That one is audited and never written
    /// over this run ([`Shell::not_unread`]): a whole-file write would lose
    /// what it held.
    pub(crate) fn read_at_start(
        &mut self,
        what: &'static str,
        read: Result<Option<String>, String>,
    ) -> Option<String> {
        match read {
            Ok(text) => text,
            Err(why) => {
                self.host.audit(&format!(
                    "the {what} on disk could not be read ({why}); starting with none, \
                     and the file is not written over"
                ));
                self.unread.push(what);
                None
            }
        }
    }

    /// Why `what`'s file may not be written: it could not be read at start.
    pub(crate) fn not_unread(&self, what: &str) -> Result<(), String> {
        match self.unread.contains(&what) {
            true => Err(format!(
                "the {what} on disk could not be read at start, so it is not written over"
            )),
            false => Ok(()),
        }
    }

    /// Writes the rules to `/system/state/rules`; a failure is audited, and
    /// returned for a reply that would otherwise say the change holds across a
    /// reboot. The rules hold in memory either way.
    pub(crate) fn save_rules(&self) -> Result<(), String> {
        let written = self
            .not_unread(RULES)
            .and_then(|()| self.rules.text())
            .and_then(|text| self.host.rules_write(&text));
        if let Err(why) = &written {
            self.host
                .audit(&format!("the rules were not written: {why}"));
        }
        written
    }

    pub(crate) fn with_saved_log(mut self) -> Self {
        let Some(text) = self.host.log_read() else {
            // Nothing on disk is an empty log: it needs no write until it changes.
            self.saved_log = Some(self.log.text());
            return self;
        };
        match actlog::parse_setting_aside(&text) {
            Ok((records, aside)) => {
                if !aside.is_empty() {
                    let ids: Vec<String> = aside.iter().map(|id| format!("#{id}")).collect();
                    self.host.audit(&format!(
                        "log entries {} named no agent and were set aside",
                        ids.join(", ")
                    ));
                }
                self.log = actlog::ActLog::from_records(records);
                self.next_id = self.log.last_id() + 1;
                self.saved_log = Some(text);
                // The outbox starts empty: a send still waiting, or going,
                // when the shell stopped never goes now.
                if outbox::settle_at_start(&mut self.log) {
                    self.host
                        .audit("sends held when the shell stopped were logged as not sent");
                    self.save_log();
                }
            }
            Err(error) => {
                self.host.audit(&format!(
                    "the log on disk is not actlog 1 (line {}: {}); starting a new one",
                    error.line, error.why
                ));
                // Its sends are lost with it; one may have been going out.
                if text.contains(" after=sending ") {
                    self.host
                        .audit("the log set aside held a send that may have been sent");
                }
            }
        }
        self
    }

    /// Writes the log to `/system/log` if the line changed it.
    pub(crate) fn save_log(&mut self) {
        if let Err(why) = self.write_log() {
            self.host.audit(&format!("the log was not written: {why}"));
        }
    }

    /// [`Shell::save_log`], saying whether the log is on disk: a send goes
    /// only after its `sending` is.
    pub(crate) fn write_log(&mut self) -> Result<(), String> {
        let text = self.log.text();
        if self.saved_log.as_ref() == Some(&text) {
            return Ok(());
        }
        match self.host.log_write(&text) {
            Ok(()) => {}
            // The line's files are not on the disk, nor the log with them.
            Err(LogFailed::NotLanded { txn, why }) => return Err(self.not_landed(txn, &why)),
            Err(LogFailed::Failed(why)) => return Err(why),
        }
        self.saved_log = Some(text);
        Ok(())
    }
}
