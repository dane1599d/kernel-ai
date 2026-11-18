//! Per-agent consent at the one door (ADR 0013 §1, ADR 0002 D3).
//!
//! An agent never borrows the person's consent. `agentstep` decides its call
//! on the words [`agentid::Admission::consent_words`] gives it, which for an
//! agent are none; this module is the rest:
//!
//! - **No rule of the person's covers an agent.** A standing rule the person
//!   wrote ("always allow stopping build-*") was written for Kai's lines; an
//!   agent's step under it still asks (`agentcore`'s rule check reads
//!   [`Shell::agent_acting`]).
//! - **The card names the agent and offers no Always,** so the person sees
//!   who asks and no answer to an agent writes a rule.
//! - **The question is opened in [`agentid::Asks`]:** at most
//!   [`agentid::asks::MAX_PER_AGENT`] per agent, with room kept for Kai's.
//! - **Only the person's answer runs it,** at the console (the kernel
//!   terminal's keyboard), and only if the yes still holds when it runs: the
//!   agent that asked, the same entry and the same call, and the agent still
//!   on the roster with grants for the row ([`agentid::Answered::spend`]).

use crate::{Reply, Shell};
use agentid::{Admission, AnswerError, AskError, Mark, Source};
use alloc::format;
use alloc::string::String;
use termcore::menu::{Answer, Menu, Origin};

/// What a step whose question could not be opened is told.
fn ask_refusal(why: AskError) -> &'static str {
    match why {
        AskError::TooMany => "the agent has 4 questions waiting for the person",
        AskError::Full => "the person's card has no room for an agent's question",
        AskError::Duplicate => "that entry asks already",
        AskError::Retired => "the agent is not on the roster",
    }
}

/// The menu `text` (`Menu::encode`'s) as an agent's card: the question says
/// who asks, the Always choices are gone, and the highlight starts on Deny.
/// `None` if the question with the name is too long to show.
pub(crate) fn agent_menu(agent: &str, text: &str) -> Option<String> {
    let mut lines = text.splitn(5, '\n');
    let head = lines.next()?;
    let id = head
        .strip_prefix("menu ")?
        .split(' ')
        .next()?
        .parse::<u64>()
        .ok()?;
    let question = format!("{agent} asks: {}", lines.next()?);
    let (_kind, _target, call) = (lines.next()?, lines.next()?, lines.next()?);
    Menu::new(id, &question, None, None, Origin::Added)
        .and_then(|menu| menu.with_call(call))
        .ok()?;
    Some(Menu::encode(id, Origin::Added, &question, "", "", call))
}

impl Shell {
    /// The admission of the agent whose step is running, if one is: no rule
    /// of the person's covers it.
    pub(crate) fn agent_acting(&self) -> Option<&Admission> {
        self.acting.as_ref()
    }

    /// Opens the question step `id`'s pending call asks, for `admission`.
    /// On a refusal the pending call is let go and logged.
    pub(crate) fn open_agent_ask(&mut self, id: u64, admission: &Admission) -> Result<(), String> {
        let Some(shown) = self.pending.as_ref().map(|p| p.checked.script.clone()) else {
            return Err(String::from("nothing to ask"));
        };
        match self.agent_asks.ask(&self.roster, id, admission, &shown) {
            Ok(()) => {
                self.agent_admissions.push((id, admission.clone()));
                Ok(())
            }
            Err(why) => {
                if let Some(pending) = self.pending.take() {
                    let outcome = format!("not asked: {}", ask_refusal(why));
                    self.record(id, &pending.request, &pending.checked.script, &outcome);
                }
                Err(String::from(ask_refusal(why)))
            }
        }
    }

    /// Question `id` is closed, answered or not: its admission goes.
    pub(crate) fn close_agent_ask(&mut self, id: u64) {
        self.agent_asks.withdraw(id);
        self.agent_admissions.retain(|(entry, _)| *entry != id);
    }

    /// Whether question `id` is an agent's.
    pub(crate) fn card_is_agents(&self, id: u64) -> bool {
        self.agent_admissions.iter().any(|(entry, _)| *entry == id)
    }

    /// The console's answer `word` to the card's question `id`, if it is an
    /// agent's: `None` to let the answer go on as any card's does, or the
    /// reply when it may not. `mark` is who answered.
    pub(crate) fn agent_answer(&mut self, id: u64, word: &str, mark: Mark) -> Option<Reply> {
        if !self.card_is_agents(id) || !self.on_card_for_desk(id) {
            return None;
        }
        let yes = match Answer::from_word(word) {
            Some(Answer::Allow) => true,
            Some(Answer::Deny) => false,
            // The agent's card offers no Always: no answer to an agent writes
            // a rule.
            _ => return Some(self.agent_not_run(id, "an agent's question has no Always")),
        };
        let answered = match self.agent_asks.answer(id, yes, mark) {
            Ok(answered) => answered,
            Err(AnswerError::NotThePerson) => {
                return Some(Reply::err("only the person answers an agent's question"));
            }
            Err(AnswerError::NothingAsked) => {
                return Some(self.agent_not_run(id, "nothing of the agent's asks there"));
            }
        };
        if !yes {
            self.close_agent_ask(id);
            return None;
        }
        let admission = self
            .agent_admissions
            .iter()
            .find(|(entry, _)| *entry == id)
            .map(|(_, admission)| admission.clone());
        let shown = self
            .pending
            .as_ref()
            .filter(|p| p.id == id)
            .map(|p| p.checked.script.clone());
        let holds = match (admission, shown) {
            (Some(admission), Some(shown)) => answered.spend(&self.roster, &admission, id, &shown),
            _ => false,
        };
        match holds {
            // `answer_pending` runs it, once, for this spent yes.
            true => {
                self.agent_spent = Some(id);
                None
            }
            false => Some(self.agent_not_run(id, "the agent's grants or call changed")),
        }
    }

    /// Whether the answer to question `id`, an agent's, may run it: only a yes
    /// [`Shell::agent_answer`] spent, once. Any other answer (a bare `y` at
    /// the console, an answer that skipped the card) lets it go.
    /// On a spent yes, the agent's name, which the run's entries are stamped
    /// with.
    pub(crate) fn agent_yes_spent(&mut self, id: u64) -> Result<agentid::Name, Reply> {
        let admission = self
            .agent_admissions
            .iter()
            .find(|(entry, _)| *entry == id)
            .map(|(_, admission)| admission.clone());
        match (self.agent_spent.take() == Some(id), admission) {
            (true, Some(admission)) => {
                self.close_agent_ask(id);
                Ok(admission.stamp().name().clone())
            }
            _ => Err(self.agent_not_run(id, "an agent's question is answered on its card")),
        }
    }

    /// Lets go of the card's question `id` without running it, tells the
    /// agent, and logs why.
    pub(crate) fn agent_not_run(&mut self, id: u64, why: &str) -> Reply {
        if let Some(pending) = self.pending.take_if(|p| p.id == id) {
            self.record(
                id,
                &pending.request,
                &pending.checked.script,
                &format!("refused: {why}"),
            );
            self.save_log();
        }
        self.close_agent_ask(id);
        if let Some((_, asker)) = self.card_for_desk.take() {
            self.desk_told(id, asker, false, true, format!("Not run: {why}."));
        }
        Reply::err(format!("Not run: {why}."))
    }

    /// The person's mark, for an answer the kernel terminal delivered: its
    /// lines are the keyboard's (the shell answers `eval_console` only for
    /// badge 0, the kernel's own route).
    pub(crate) fn console_mark() -> Mark {
        // SAFETY: not memory safety. `eval_console` is called only with the
        // kernel terminal's lines, which the person typed at the keyboard;
        // an agent's route never reaches it (`agentstep`'s gate).
        Mark::of(unsafe { Source::hardware() })
    }
}
