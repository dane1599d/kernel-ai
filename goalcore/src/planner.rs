//! The `goals` agent's planner on llm's agent jobs (row G1), with no OS in it:
//! one `act` job a goal at a time, started with `agent-start act` and asked
//! about each turn with `agent-take`. The OS half (`user/src/bin/goals`) gives
//! it the call to llm, the request that plans a goal and how an answer reads
//! as a plan. llm's replies are read here, so the rules are tested on the host.
//!
//! Only a job's own answer ends a plan: `ok ...`, `err ...`, or `ended <why>`
//! from the scheduler. A reply that says llm cannot answer now fails no goal
//! (bug-hunt-4 H4-13): no reply (llm served the person's long request, or was
//! restarting) and `busy ...` (llm writes the person's app) keep the ticket
//! for the next turn; `ended unknown ticket` (llm restarted) drops it, and the
//! goal is planned afresh. Its wall time and deadline still bound the wait.

use crate::Goal;
use crate::runner::{Plan, Planner};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// llm, on the agent's route.
pub trait Llm {
    /// One request: llm's reply, or why none came.
    fn call(&mut self, text: &str) -> Result<String, String>;
    /// A line for the agent's log.
    fn note(&mut self, _line: &str) {}
}

/// What one `agent-take` came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Taken<'a> {
    /// Not answered yet, or llm cannot answer now: ask again next turn.
    Wait,
    /// llm does not know the ticket: plan the goal afresh.
    Replan,
    /// The job's own answer.
    Answer(&'a str),
}

/// Reads `agent-take`'s reply, or why the call got none.
pub fn taken<'a>(reply: Result<&'a str, &str>) -> Taken<'a> {
    match reply {
        Err(_) | Ok("pending") => Taken::Wait,
        Ok(busy) if busy.starts_with("busy ") => Taken::Wait,
        Ok("ended unknown ticket") => Taken::Replan,
        Ok(answer) => Taken::Answer(answer),
    }
}

/// Each goal's next step from llm's agent `act` jobs, one job a goal at a time.
pub struct JobPlanner<L, C> {
    llm: L,
    /// The `agent-start act` request that plans a goal, without `act`.
    request: fn(&Goal) -> Result<String, String>,
    /// What a job's answer says the goal does next.
    read: fn(&str) -> Plan<C>,
    /// The ticket of each goal's job in flight.
    jobs: Vec<(u64, String)>,
}

impl<L: Llm, C> JobPlanner<L, C> {
    pub fn new(
        llm: L,
        request: fn(&Goal) -> Result<String, String>,
        read: fn(&str) -> Plan<C>,
    ) -> Self {
        Self {
            llm,
            request,
            read,
            jobs: Vec::new(),
        }
    }

    /// Forgets `goal`'s job in flight (a cancel).
    pub fn forget(&mut self, goal: u64) {
        self.jobs.retain(|(g, _)| *g != goal);
    }

    /// Starts the job that plans `goal`: its ticket.
    fn start(&mut self, goal: &Goal) -> Result<String, String> {
        let body = (self.request)(goal)?;
        let started = self.llm.call(&format!("agent-start act {body}"))?;
        started
            .strip_prefix("ok ")
            .map(|ticket| ticket.trim().to_string())
            .ok_or(started)
    }
}

impl<L: Llm, C> Planner<C> for JobPlanner<L, C> {
    fn next(&mut self, goal: &Goal, _now: u64) -> (Plan<C>, u64) {
        let Some(at) = self.jobs.iter().position(|(g, _)| *g == goal.id) else {
            match self.start(goal) {
                Ok(ticket) => self.jobs.push((goal.id, ticket)),
                Err(why) => self
                    .llm
                    .note(&format!("goal {} not planned: {why}", goal.id)),
            }
            return (Plan::Later, 0);
        };
        let reply = self.llm.call(&format!("agent-take {}", self.jobs[at].1));
        match taken(reply.as_deref().map_err(String::as_str)) {
            Taken::Wait => {
                if let Err(why) = &reply {
                    self.llm
                        .note(&format!("goal {}: agent-take: {why}", goal.id));
                }
                (Plan::Later, 0)
            }
            Taken::Replan => {
                self.jobs.remove(at);
                (Plan::Later, 0)
            }
            Taken::Answer(answer) => {
                self.jobs.remove(at);
                ((self.read)(answer), 0)
            }
        }
    }
}

#[cfg(test)]
mod tests;
