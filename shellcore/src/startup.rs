//! What the shell reads back at start, and writes as it goes: the log, the
//! rules and the withheld rows.

#[allow(unused_imports)]
use super::*;

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
    /// The rules `/system/state/rules` holds; a file refused whole is set aside,
    /// and the shell starts with none.
    pub(crate) fn with_saved_rules(mut self) -> Self {
        let Some(text) = self.host.rules_read() else {
            return self;
        };
        match rules::Rules::restore(&text) {
            Ok(rules) => self.rules = rules,
            Err(why) => self.host.audit(&format!(
                "the rules on disk were refused ({why}); starting with none"
            )),
        }
        self
    }

    /// Writes the rules to `/system/state/rules`; a failure is audited, and the
    /// rules hold in memory.
    pub(crate) fn save_rules(&self) {
        let written = self
            .rules
            .text()
            .and_then(|text| self.host.rules_write(&text));
        if let Err(why) = written {
            self.host
                .audit(&format!("the rules were not written: {why}"));
        }
    }

    pub(crate) fn with_saved_log(mut self) -> Self {
        let Some(text) = self.host.log_read() else {
            // Nothing on disk is an empty log: it needs no write until it changes.
            self.saved_log = Some(self.log.text());
            return self;
        };
        match actlog::parse(&text) {
            Ok(records) => {
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
        self.host.log_write(&text)?;
        self.saved_log = Some(text);
        Ok(())
    }
}
