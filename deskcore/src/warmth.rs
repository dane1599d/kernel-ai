//! Whether the panel says "Kai is getting ready" (founder test 2): the
//! shell's rules, apart from its calls so they are tested on the host.
//! `user/src/bin/shell/warmth.rs` asks llm `warm`, feeds the answers in, and
//! tells `desktop` [`Readiness::want`] when it differs from what it told.
//!
//! llm is unconfirmed at first, and again after anything that makes it fill
//! its prompt slots: a `new`, a switch between `c>` and Rhai, or a restart
//! (a new pid, or a look that found no llm). Until it says "yes", a "no" (or a
//! panel line that waits on it) means "getting ready". Once it has said "yes",
//! a "no" is the brief re-compaction after a remembered turn, or an app build
//! holding its slot: the panel keeps saying ready.

/// How often llm is asked between lines while it is unconfirmed, or while the
/// panel says it is getting ready.
pub const COLD_ASK_NS: u64 = 3_000_000_000;
/// And otherwise: a restart shows within this.
pub const WARM_ASK_NS: u64 = 30_000_000_000;

/// llm's answer to `warm`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    /// `ok yes`: both prompt slots hold their prompts.
    Yes,
    /// `ok no`.
    No,
    /// No answer in time: llm is computing.
    Late,
}

/// What the shell believes about llm's warmth, and what the panel should say.
#[derive(Debug, Default)]
pub struct Readiness {
    /// llm said "yes" since the last thing that cooled it.
    confirmed: bool,
    /// The llm the last look saw (`None` inside: none ran), once one looked.
    seen: Option<Option<u64>>,
    /// Whether the panel should say ready; `None` until anything is known.
    want: Option<bool>,
    asked_ns: u64,
}

impl Readiness {
    /// Whether the panel should say ready, once anything is known.
    pub fn want(&self) -> Option<bool> {
        self.want
    }

    /// Whether a between-lines ask is due at `now` (ns); if so, it counts as
    /// made.
    pub fn due(&mut self, now: u64) -> bool {
        let gap = match self.confirmed && self.want != Some(false) {
            true => WARM_ASK_NS,
            false => COLD_ASK_NS,
        };
        if self.asked_ns != 0 && now.saturating_sub(self.asked_ns) < gap {
            return false;
        }
        self.asked_ns = now.max(1);
        true
    }

    /// The next between-lines pass asks: the panel's line has its answer, so
    /// llm is free.
    pub fn ask_soon(&mut self) {
        self.asked_ns = 0;
    }

    /// A look at the running llm, `pid`, and whether the shell cooled it since
    /// the last look. Any change of pid after the first look, to or from none
    /// included, is a restart. With no llm, the panel does not say "getting
    /// ready", and nothing is confirmed.
    pub fn look(&mut self, pid: Option<u64>, cooled: bool) {
        let restarted = self.seen.is_some_and(|seen| seen != pid);
        if cooled || restarted || pid.is_none() {
            self.confirmed = false;
        }
        self.seen = Some(pid);
        if pid.is_none() && self.want == Some(false) {
            self.want = Some(true);
        }
    }

    /// llm's answer; `waits` when the panel's line is about to wait on llm.
    pub fn answer(&mut self, answer: Answer, waits: bool) {
        match answer {
            Answer::Yes => {
                self.confirmed = true;
                self.want = Some(true);
            }
            // Before its first "yes" llm is filling its slots, and a line
            // that finds it computing waits on that.
            Answer::No if !self.confirmed => self.want = Some(false),
            Answer::Late if waits && !self.confirmed => self.want = Some(false),
            // A brief re-compaction or an app build, or a busy llm between
            // lines: nothing changes.
            Answer::No | Answer::Late => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LLM: Option<u64> = Some(7);

    fn warm() -> Readiness {
        let mut r = Readiness::default();
        r.look(LLM, false);
        r.answer(Answer::Yes, false);
        r
    }

    #[test]
    fn getting_ready_until_the_first_yes() {
        let mut r = Readiness::default();
        r.look(LLM, false);
        assert_eq!(r.want(), None);
        r.answer(Answer::Late, false);
        assert_eq!(r.want(), None, "a busy llm between lines changes nothing");
        r.answer(Answer::Late, true);
        assert_eq!(r.want(), Some(false), "a line waits on the warm-up");
        r.answer(Answer::No, false);
        assert_eq!(r.want(), Some(false));
        r.answer(Answer::Yes, false);
        assert_eq!(r.want(), Some(true));
    }

    /// The re-compaction after `remember`, or an app build's `ok no`.
    #[test]
    fn a_brief_no_once_confirmed_keeps_ready() {
        let mut r = warm();
        r.answer(Answer::No, false);
        r.answer(Answer::No, true);
        r.answer(Answer::Late, true);
        assert_eq!(r.want(), Some(true));
    }

    #[test]
    fn new_or_a_language_switch_cools_it() {
        let mut r = warm();
        r.look(LLM, true);
        r.answer(Answer::No, false);
        assert_eq!(r.want(), Some(false));
        r.answer(Answer::Yes, false);
        assert_eq!(r.want(), Some(true));
    }

    #[test]
    fn a_restart_is_a_new_pid() {
        let mut r = warm();
        r.look(Some(8), false);
        r.answer(Answer::No, false);
        assert_eq!(r.want(), Some(false));
    }

    /// A look between the old llm's exit and the new one's start sees none:
    /// the same restart, whatever pid the new llm has.
    #[test]
    fn a_restart_seen_as_a_gap_is_a_restart() {
        let mut r = warm();
        r.look(None, false);
        r.look(LLM, false);
        r.answer(Answer::No, false);
        assert_eq!(r.want(), Some(false));
    }

    #[test]
    fn no_llm_is_not_getting_ready_and_not_confirmed() {
        let mut r = Readiness::default();
        r.look(LLM, false);
        r.answer(Answer::No, false);
        assert_eq!(r.want(), Some(false));
        r.look(None, false);
        assert_eq!(r.want(), Some(true), "no llm: not getting ready");
        // Never confirmed by that: the llm that comes is asked as cold.
        r.look(LLM, false);
        r.answer(Answer::No, false);
        assert_eq!(r.want(), Some(false));
        let mut gone = Readiness::default();
        gone.look(None, false);
        assert_eq!(
            gone.want(),
            None,
            "nothing to tell before anything is known"
        );
    }

    #[test]
    fn asks_often_while_unconfirmed_or_getting_ready() {
        let mut r = Readiness::default();
        r.look(LLM, false);
        assert!(r.due(10));
        assert!(!r.due(10 + COLD_ASK_NS - 1));
        assert!(r.due(10 + COLD_ASK_NS));
        r.answer(Answer::Yes, false);
        let at = 10 + COLD_ASK_NS;
        assert!(!r.due(at + COLD_ASK_NS), "warm: the long gap");
        assert!(r.due(at + WARM_ASK_NS));
        r.ask_soon();
        assert!(r.due(at + WARM_ASK_NS + 1), "after a panel line: at once");
        // Cooled and told "no": the short gap again.
        r.look(LLM, true);
        r.answer(Answer::No, false);
        assert!(r.due(at + WARM_ASK_NS + 1 + COLD_ASK_NS));
    }
}
