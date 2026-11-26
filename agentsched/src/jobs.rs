//! Agents' requests as llm jobs (ADR 0013 section 2; row F3 of
//! `docs/plans/agents-os.md`, slice 1). Here, not in `llmproto`, since
//! `llmproto` is below `kaipkg`, which `kaiagent` and so this crate use. The person's requests stay llm's
//! synchronous path; an agent's come in on its own route, whose badge the kernel
//! sets (`kabi::handle::agent`), and wait their turn in `agentsched`:
//!
//! - `agent-start <act|script|ask> <request>`: `ok <ticket>` once the scheduler
//!   takes it, or `err <why>` in the ADR's words (`Over quota: goals used 100000
//!   tokens today`, too long, too many waiting, not an agent);
//! - `agent-take <ticket>`: `pending`, the answer as its own request's reply
//!   (after `used <n> ` when llm charged the job `n` tokens, so the agent can
//!   charge its own budget, a goal's tokens), or `ended <why>`;
//! - `agent-stop <ticket>`: `ok`, the request dropped.
//!
//! **Only the route's badge names an agent** ([`Caller`]): the kernel puts it
//! on the route (`RECV_H`'s `RecvInfo::badge`), and no process can forge it. The
//! caller's pid is no identity: a process whose pid is 2 is not `goals`.
//!
//! A ticket is good only on the badge that started it: any other badge hears
//! `ended unknown ticket`, as for a ticket that never was, so a ticket says
//! nothing about another agent's work. Badge 0 (no agent) and `kai` (the
//! person) start no job: theirs is the foreground, which never waits for one.
//!
//! This is the state and the rules; llm's service loop runs the engine on the
//! scheduler's decisions (slice 2). `act` and `script` answers are grammar-shaped
//! ([`Shape::Grammar`]: interrupted, they start again from their prompt), `ask`
//! is free text.

use crate::{
    Agent, AgentId, Ask, Class, Config, MAX_QUEUED, Now, Refusal, RequestId, Scheduler, Shape,
};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;

/// The badge of a route that is no agent's.
pub const NO_AGENT: u64 = 0;
/// `kai`'s badge: the shell, which is the person.
pub const KAI: u64 = 1;
/// The built-in agents other than `kai`: each badge, name and class (ADR 0013
/// section 1; `agentid::Builtin`'s order, which a test holds this to).
pub const BUILTIN: [(u64, &str, Class); 3] = [
    (2, "goals", Class::Background),
    (3, "index", Class::Index),
    (4, "maint", Class::Background),
];
/// The most answers kept for one badge before it takes them: a badge with this many
/// jobs, waiting or done, starts no more.
pub const MAX_JOBS: usize = MAX_QUEUED;

/// Which answer a job asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// An `act` call: grammar-shaped.
    Act,
    /// A `script` program: grammar-shaped.
    Script,
    /// An `ask` answer: free text.
    Ask,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Act => "act",
            Kind::Script => "script",
            Kind::Ask => "ask",
        }
    }

    fn from_word(word: &str) -> Option<Kind> {
        [Kind::Act, Kind::Script, Kind::Ask]
            .into_iter()
            .find(|kind| kind.word() == word)
    }

    /// What the scheduler decodes it under.
    pub fn shape(self) -> Shape {
        match self {
            Kind::Act | Kind::Script => Shape::Grammar,
            Kind::Ask => Shape::Free,
        }
    }
}

/// An agent command, read from a request's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command<'a> {
    Start { kind: Kind, request: &'a str },
    Take(u64),
    Stop(u64),
}

/// The agent command `text` is, `Ok(None)` when it is no agent command, or why it
/// is one out of shape.
pub fn parse(text: &str) -> Result<Option<Command<'_>>, String> {
    let (command, rest) = text.split_once(' ').unwrap_or((text, ""));
    let ticket = |rest: &str| {
        rest.parse::<u64>()
            .ok()
            .filter(|_| rest.bytes().all(|b| b.is_ascii_digit()))
            .ok_or_else(|| format!("{command} takes a ticket, not {rest:?}"))
    };
    match command {
        "agent-start" => {
            let (word, request) = rest.split_once(' ').unwrap_or((rest, ""));
            let kind = Kind::from_word(word)
                .ok_or_else(|| format!("agent-start takes act, script or ask, not {word:?}"))?;
            if request.trim().is_empty() {
                return Err(String::from("agent-start needs a request"));
            }
            Ok(Some(Command::Start { kind, request }))
        }
        "agent-take" => Ok(Some(Command::Take(ticket(rest)?))),
        "agent-stop" => Ok(Some(Command::Stop(ticket(rest)?))),
        _ => Ok(None),
    }
}

/// What llm says to an agent's request that is not an agent command.
pub const AGENTS_USE_JOBS: &str = "err agents use agent-start";

/// Whether llm serves `command` (a request's text) to the route with `badge`. The
/// person's commands (`act` with their conversation, `remember`, `new`, the app
/// rounds, `ask`, `script`, `tune` and the rest) are `kai`'s, and badge 0's: the
/// services that call llm with no agent's badge (`updated`, the Monitor window).
/// An agent's badge may send only `agent-*`, so no agent reads, plants or wipes the
/// person's conversation or stops their app round.
pub fn may_serve(badge: u64, command: &str) -> bool {
    badge == NO_AGENT || badge == KAI || command.starts_with("agent-")
}

/// The name of the agent with `badge`, if it is a built-in other than `kai`.
pub fn builtin_name(badge: u64) -> Option<&'static str> {
    BUILTIN
        .iter()
        .find(|(b, ..)| *b == badge)
        .map(|(_, name, _)| *name)
}

/// A refusal of the scheduler's, in the ADR's words, for agent `name`.
pub fn refusal_words(refusal: Refusal, name: &str) -> String {
    match refusal {
        Refusal::UnknownAgent => format!("{name} is not an agent llm knows"),
        Refusal::Empty => String::from("an empty request"),
        Refusal::TooLong { positions, max } => {
            format!("the request from {name} needs {positions} positions; it may hold {max}")
        }
        Refusal::OverQuota { used, .. } => format!("Over quota: {name} used {used} tokens today"),
        Refusal::QueueFull => format!("{name} has {MAX_QUEUED} requests waiting"),
    }
}

/// Who sent a request, as the kernel reports it (`RECV_H`): the process, and the
/// badge of the route it came through. Only the badge is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caller {
    pub pid: u64,
    pub badge: u64,
}

/// What a job holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub badge: u64,
    pub kind: Kind,
    pub request: String,
    /// The scheduler's request while it waits or runs; none once it is done.
    pub queued: Option<RequestId>,
    /// Its reply, once done.
    pub done: Option<String>,
    /// The tokens it was charged ([`Jobs::charge`]), told with its answer.
    pub used: u32,
}

/// Agents' jobs and the scheduler they wait in.
pub struct Jobs {
    sched: Scheduler,
    jobs: BTreeMap<u64, Job>,
    next_ticket: u64,
}

impl Jobs {
    /// Jobs for the built-in agents, in a scheduler of `config`; `run` makes the
    /// tickets of this run of llm, so one from another run is unknown.
    pub fn new(config: Config, run: u64) -> Self {
        let mut sched = Scheduler::new(config);
        for (badge, _, class) in BUILTIN {
            sched.set_agent(Agent::builtin(AgentId(badge as u32), class, 1));
        }
        Self {
            sched,
            jobs: BTreeMap::new(),
            next_ticket: (run << 32) | 1,
        }
    }

    /// The scheduler, for llm's loop to drive.
    pub fn scheduler(&mut self) -> &mut Scheduler {
        &mut self.sched
    }

    /// Job `ticket`, if any.
    pub fn job(&self, ticket: u64) -> Option<&Job> {
        self.jobs.get(&ticket)
    }

    /// The job to run next when the caller runs requests whole (slice 2a): one of the
    /// highest class waiting, the oldest first. The person's requests never wait for
    /// it: the caller runs it only while none waits, and stops it when one comes.
    pub fn next_job(&self) -> Option<(u64, &Job)> {
        self.jobs
            .iter()
            .filter(|(_, job)| job.queued.is_some())
            .min_by_key(|(ticket, job)| {
                let class = self.sched.class_of(AgentId(job.badge as u32));
                (class.unwrap_or(Class::Index), **ticket)
            })
            .map(|(ticket, job)| (*ticket, job))
    }

    /// Counts what job `ticket` spent against its agent's day, and keeps it.
    pub fn charge(&mut self, ticket: u64, tokens: u32, now: Now) {
        if let Some(job) = self.jobs.get_mut(&ticket) {
            job.used = job.used.saturating_add(tokens);
            let agent = AgentId(job.badge as u32);
            self.sched.charge_whole(agent, tokens, now);
        }
    }

    /// The ticket whose scheduler request is `request`.
    pub fn ticket_of(&self, request: RequestId) -> Option<u64> {
        self.jobs
            .iter()
            .find(|(_, job)| job.queued == Some(request))
            .map(|(ticket, _)| *ticket)
    }

    /// Answers agent command `command` from `caller`, known by its route's badge
    /// alone. `prompt_tokens` is what the request's prompt takes, and `max_new` what
    /// its answer may, both the engine's.
    pub fn handle(
        &mut self,
        caller: Caller,
        command: Command<'_>,
        prompt_tokens: u32,
        max_new: u32,
        now: Now,
    ) -> String {
        let badge = caller.badge;
        match command {
            Command::Start { kind, request } => {
                self.start(badge, kind, request, prompt_tokens, max_new, now)
            }
            Command::Take(ticket) => self.take(badge, ticket),
            Command::Stop(ticket) => self.stop(badge, ticket),
        }
    }

    fn start(
        &mut self,
        badge: u64,
        kind: Kind,
        request: &str,
        prompt_tokens: u32,
        max_new: u32,
        now: Now,
    ) -> String {
        if badge == NO_AGENT || badge == KAI {
            return String::from("err the person's requests and services' are not agent jobs");
        }
        let name = builtin_name(badge).unwrap_or("this agent");
        if self.jobs.values().filter(|job| job.badge == badge).count() >= MAX_JOBS {
            return format!("err {}", refusal_words(Refusal::QueueFull, name));
        }
        let ask = Ask {
            agent: AgentId(badge as u32),
            prompt_tokens,
            max_new_tokens: max_new,
            shape: kind.shape(),
        };
        let queued = match u32::try_from(badge) {
            Ok(_) => self.sched.submit(ask, now),
            Err(_) => Err(Refusal::UnknownAgent),
        };
        match queued {
            Ok(id) => {
                let ticket = self.next_ticket;
                self.next_ticket += 1;
                self.jobs.insert(
                    ticket,
                    Job {
                        badge,
                        kind,
                        request: String::from(request),
                        queued: Some(id),
                        done: None,
                        used: 0,
                    },
                );
                format!("ok {ticket}")
            }
            Err(refusal) => format!("err {}", refusal_words(refusal, name)),
        }
    }

    /// The job `ticket` if `badge` started it.
    fn mine(&self, badge: u64, ticket: u64) -> Option<&Job> {
        self.jobs.get(&ticket).filter(|job| job.badge == badge)
    }

    fn take(&mut self, badge: u64, ticket: u64) -> String {
        match self.mine(badge, ticket).map(|job| job.done.is_some()) {
            None => String::from("ended unknown ticket"),
            Some(false) => String::from("pending"),
            Some(true) => self
                .jobs
                .remove(&ticket)
                .and_then(|job| match job.used {
                    0 => job.done,
                    used => job.done.map(|done| format!("used {used} {done}")),
                })
                .unwrap_or_default(),
        }
    }

    fn stop(&mut self, badge: u64, ticket: u64) -> String {
        if self.mine(badge, ticket).is_none() {
            return String::from("ok");
        }
        if let Some(job) = self.jobs.remove(&ticket)
            && let Some(id) = job.queued
        {
            self.sched.finish(id);
        }
        String::from("ok")
    }

    /// The loop's answer for scheduler request `request`: its reply, kept for
    /// `agent-take`. The scheduler is told the request is over.
    pub fn finish(&mut self, request: RequestId, reply: String) {
        self.sched.finish(request);
        if let Some(ticket) = self.ticket_of(request)
            && let Some(job) = self.jobs.get_mut(&ticket)
        {
            job.queued = None;
            job.done = Some(reply);
        }
    }

    /// The scheduler ended `request` (a timeout, the day's budget): its job is done
    /// with `ended <why>`.
    pub fn ended(&mut self, request: RequestId, why: &str) {
        self.finish(request, format!("ended {why}"));
    }
}

#[cfg(test)]
mod tests;
