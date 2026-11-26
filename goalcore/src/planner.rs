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
use crate::Goals;
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
    /// The job's own answer, and the tokens llm says it used (`used <n> `
    /// before the answer; none when it counted none).
    Answer { text: &'a str, used: u64 },
}

/// Reads `agent-take`'s reply, or why the call got none.
pub fn taken<'a>(reply: Result<&'a str, &str>) -> Taken<'a> {
    match reply {
        Err(_) | Ok("pending") => Taken::Wait,
        Ok(busy) if busy.starts_with("busy ") => Taken::Wait,
        Ok("ended unknown ticket") => Taken::Replan,
        Ok(answer) => {
            let counted = answer
                .strip_prefix("used ")
                .and_then(|rest| rest.split_once(' '))
                .and_then(|(n, text)| n.parse::<u64>().ok().map(|n| (text, n)));
            let (text, used) = counted.unwrap_or((answer, 0));
            Taken::Answer { text, used }
        }
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
    /// Tickets of jobs no goal waits for that llm has not said it stopped
    /// (it was busy, or did not answer): asked again each turn.
    stopping: Vec<String>,
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
            stopping: Vec::new(),
        }
    }

    /// Stops `goal`'s job in flight, whose answer will never be taken (a
    /// cancel). A job left in llm still runs and holds one of the agent's
    /// `MAX_JOBS` places until llm restarts (bug-hunt-4 H4-14). Until llm says
    /// `ok` (it may be busy writing the person's app, or not answer in time)
    /// the ticket is kept and the stop is asked again each turn.
    pub fn forget(&mut self, goal: u64) {
        if let Some(at) = self.jobs.iter().position(|(g, _)| *g == goal) {
            let (_, ticket) = self.jobs.remove(at);
            self.stopping.push(ticket);
            self.stop_dropped();
        }
    }

    /// Once a turn: stops the jobs of goals that ended or are no longer kept,
    /// and asks again for the stops llm has not answered `ok`.
    pub fn drop_ended(&mut self, goals: &Goals) {
        let ended: Vec<u64> = self
            .jobs
            .iter()
            .map(|(goal, _)| *goal)
            .filter(|goal| goals.get(*goal).is_none_or(|g| g.state.is_final()))
            .collect();
        for goal in ended {
            self.forget(goal);
        }
        self.stop_dropped();
    }

    /// Asks llm to stop each dropped job; keeps those it did not stop.
    fn stop_dropped(&mut self) {
        let mut kept = Vec::new();
        for ticket in core::mem::take(&mut self.stopping) {
            match self.llm.call(&format!("agent-stop {ticket}")) {
                Ok(ok) if ok == "ok" => {}
                Ok(why) | Err(why) => {
                    self.llm.note(&format!("agent-stop {ticket}: {why}"));
                    kept.push(ticket);
                }
            }
        }
        self.stopping = kept;
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
            Taken::Answer { text, used } => {
                self.jobs.remove(at);
                ((self.read)(text), used)
            }
        }
    }
}

#[cfg(test)]
mod tests;
