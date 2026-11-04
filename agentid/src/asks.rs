//! Questions an agent's call asks, and who may answer them (ADR 0013 section 1; ADR
//! 0001 D6 and ADR 0002 D3).
//!
//! A call that asks waits for the person's yes. The yes is a key the person pressed at
//! the trusted path, so it carries the person's [`Mark`]; an agent that answers its
//! own question, or another agent's, is refused. The refusal leaves the question open
//! and tells the agent nothing about which questions are: the mark is checked first,
//! and the entry numbers are the log's, which no agent chooses.
//!
//! A yes is for one call: the agent that asked, the row it asked about, once, and only
//! while that agent is still on the roster with grants for the row. (The arguments the
//! person saw are bound by the `ONCE` handle's digest, ADR 0001 D6, as before.)
//!
//! Questions are bounded so that no agent can keep the person from being asked: at
//! most [`MAX_PER_AGENT`] each, and [`KAI_ROOM`] places that only `kai` may use.

use crate::door::Admission;
use crate::mark::Mark;
use crate::roster::{AgentId, Roster};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// The most questions open at once.
pub const MAX_OPEN: usize = 16;
/// The most questions one agent other than `kai` has open.
pub const MAX_PER_AGENT: usize = 4;
/// Places among [`MAX_OPEN`] that only `kai`'s questions may take.
pub const KAI_ROOM: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Open {
    entry: u64,
    by: AgentId,
    kai: bool,
    row: String,
}

/// The open questions.
#[derive(Clone, Debug, Default)]
pub struct Asks {
    open: Vec<Open>,
}

/// Why a question was not opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AskError {
    /// [`MAX_OPEN`] questions wait, or all but `kai`'s [`KAI_ROOM`].
    Full,
    /// The agent has [`MAX_PER_AGENT`] open already.
    TooMany,
    /// The log entry asks already.
    Duplicate,
    /// The agent is not on the roster any more.
    Retired,
}

/// Why an answer was not taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnswerError {
    /// A person did not make it.
    NotThePerson,
    /// No question is open for the entry.
    NothingAsked,
}

/// An answer the person gave. It is spent once.
#[derive(Debug, PartialEq, Eq)]
pub struct Answered {
    entry: u64,
    by: AgentId,
    row: String,
    yes: bool,
}

impl Answered {
    /// The log entry that asked.
    pub fn entry(&self) -> u64 {
        self.entry
    }

    pub fn yes(&self) -> bool {
        self.yes
    }

    /// Spends the answer on the call `admission` admitted: whether it may run now. It
    /// may if the answer was a yes, the call is the agent's that asked and for the row
    /// that was asked about, and the agent is still on the roster with grants for it.
    pub fn spend(self, roster: &Roster, admission: &Admission) -> bool {
        self.yes
            && admission.caller().id() == self.by
            && admission.row() == self.row
            && roster.revalidate(admission).is_ok()
    }
}

impl Asks {
    pub const fn new() -> Self {
        Self { open: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.open.len()
    }

    pub fn is_empty(&self) -> bool {
        self.open.is_empty()
    }

    pub fn is_open(&self, entry: u64) -> bool {
        self.open.iter().any(|open| open.entry == entry)
    }

    /// Opens the question that log entry `entry` asks, for the call `admission`
    /// admitted.
    pub fn ask(
        &mut self,
        roster: &Roster,
        entry: u64,
        admission: &Admission,
    ) -> Result<(), AskError> {
        let by = admission.caller();
        if !roster.is_live(by) {
            return Err(AskError::Retired);
        }
        if self.is_open(entry) {
            return Err(AskError::Duplicate);
        }
        if by.is_kai() {
            if self.open.len() >= MAX_OPEN {
                return Err(AskError::Full);
            }
        } else {
            let mine = self.open.iter().filter(|open| open.by == by.id()).count();
            let others = self.open.iter().filter(|open| !open.kai).count();
            if mine >= MAX_PER_AGENT {
                return Err(AskError::TooMany);
            }
            if others >= MAX_OPEN - KAI_ROOM {
                return Err(AskError::Full);
            }
        }
        self.open.push(Open {
            entry,
            by: by.id(),
            kai: by.is_kai(),
            row: admission.row().to_string(),
        });
        Ok(())
    }

    /// Takes `yes` or no for `entry`'s question, if `mark` says a person gave it.
    pub fn answer(&mut self, entry: u64, yes: bool, mark: Mark) -> Result<Answered, AnswerError> {
        if !mark.is_person() {
            return Err(AnswerError::NotThePerson);
        }
        let at = self
            .open
            .iter()
            .position(|open| open.entry == entry)
            .ok_or(AnswerError::NothingAsked)?;
        let open = self.open.remove(at);
        Ok(Answered {
            entry,
            by: open.by,
            row: open.row,
            yes,
        })
    }

    /// Drops `entry`'s question unanswered (it expired).
    pub fn withdraw(&mut self, entry: u64) -> bool {
        let before = self.open.len();
        self.open.retain(|open| open.entry != entry);
        self.open.len() != before
    }

    /// Drops every question `agent` asked, as when it is retired. A yes to one of them
    /// would not run in any case: [`Answered::spend`] checks the roster.
    pub fn withdraw_agent(&mut self, agent: AgentId) {
        self.open.retain(|open| open.by != agent);
    }
}
