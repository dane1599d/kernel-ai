//! The goals the `goals` agent keeps, with the storm rules: at most
//! [`MAX_ACTIVE`] active at once and [`MAX_GOALS`] kept. What changes how many are
//! active (approving, resuming, a raise, a later deadline) goes through here, so
//! the limit holds whoever calls.

use crate::checkin::CheckIn;
use crate::goal::{Budget, Goal, Pause, Spec, State};
use crate::run::Refusal;
use crate::{MAX_ACTIVE, MAX_GOALS};
use alloc::vec::Vec;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Goals {
    pub(crate) list: Vec<Goal>,
    /// The number the next goal gets; ids are never used twice.
    pub(crate) next_id: u64,
}

impl Default for Goals {
    fn default() -> Self {
        Self::new()
    }
}

impl Goals {
    pub fn new() -> Self {
        Self {
            list: Vec::new(),
            next_id: 1,
        }
    }

    pub fn list(&self) -> &[Goal] {
        &self.list
    }

    pub fn get(&self, id: u64) -> Option<&Goal> {
        self.list.iter().find(|goal| goal.id == id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Goal> {
        self.list.iter_mut().find(|goal| goal.id == id)
    }

    /// Goals approved and neither paused nor finished.
    pub fn live(&self) -> usize {
        self.list.iter().filter(|goal| goal.state.is_live()).count()
    }

    fn goal(&mut self, id: u64) -> Result<&mut Goal, Refusal> {
        self.get_mut(id).ok_or(Refusal::NoSuchGoal(id))
    }

    /// Room for goal `id` to be active: it counts once if it already is.
    fn room(&self, id: u64) -> Result<(), Refusal> {
        let others = self
            .list
            .iter()
            .filter(|goal| goal.id != id && goal.state.is_live())
            .count();
        match others < MAX_ACTIVE {
            true => Ok(()),
            false => Err(Refusal::TooManyActive),
        }
    }

    /// `goal_set`: a goal made from `spec`, waiting for the person's yes to what
    /// [`Spec::approval`] shows. It takes no step until [`Goals::approve`].
    pub fn set(&mut self, spec: Spec, now: u64) -> Result<u64, Refusal> {
        spec.check(now).map_err(Refusal::BadSpec)?;
        if self.list.len() >= MAX_GOALS {
            return Err(Refusal::TooManyGoals);
        }
        let id = self.next_id;
        self.next_id += 1;
        self.list.push(Goal::from_spec(id, spec, now));
        Ok(id)
    }

    /// The person said yes to the scope, budget and check-ins: the goal starts.
    pub fn approve(&mut self, id: u64, now: u64) -> Result<(), Refusal> {
        self.room(id)?;
        self.goal(id)?.start(now)
    }

    /// `goal_resume`: continues a goal the person paused.
    pub fn resume(&mut self, id: u64) -> Result<(), Refusal> {
        self.room(id)?;
        self.goal(id)?.resume()
    }

    /// More budget, approved: resumes the goal if it has room again.
    pub fn raise(&mut self, id: u64, now: u64, more: Budget) -> Result<State, Refusal> {
        self.room(id)?;
        self.goal(id)?.raise(now, more)
    }

    /// A later deadline, approved: resumes the goal if it has room otherwise.
    pub fn extend(&mut self, id: u64, now: u64, by_ms: u64) -> Result<State, Refusal> {
        self.room(id)?;
        self.goal(id)?.extend(now, by_ms)
    }

    /// Drops a finished goal's record. Its steps stay in the `actlog`.
    pub fn forget(&mut self, id: u64) -> Result<(), Refusal> {
        let at = self
            .list
            .iter()
            .position(|goal| goal.id == id)
            .ok_or(Refusal::NoSuchGoal(id))?;
        match self.list[at].state.is_final() {
            true => {
                self.list.remove(at);
                Ok(())
            }
            false => Err(Refusal::NotFinished(self.list[at].state)),
        }
    }

    /// What the panel does on each tick: a goal whose deadline passed with no
    /// step running pauses and asks for a later one, and each goal with a
    /// check-in due gives one line (call again for another). Goals in id order.
    pub fn tick(&mut self, now: u64) -> Vec<(u64, CheckIn)> {
        let mut lines = Vec::new();
        for goal in &mut self.list {
            let late = goal.by_ms.is_some_and(|by| now >= by);
            if goal.state == State::Active && goal.flight.is_none() && late {
                goal.state = State::Paused(Pause::Deadline);
            }
            if let Some(line) = goal.due_checkin(now) {
                lines.push((goal.id, line));
            }
        }
        lines
    }
}
