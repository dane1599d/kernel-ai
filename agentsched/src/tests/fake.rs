//! A fake clock and a fake engine: the scheduler decides, the engine takes time.

use crate::{
    Agent, AgentId, Ask, Class, Config, Decision, Kind, Now, Place, Refusal, RequestId, Scheduler,
};
use kaiagent::Budgets;
use std::vec::Vec;

/// A prefill block of 32 tokens, in ms (ADR 0013 section 2: about 350 ms at
/// 91 tokens/s).
pub const PREFILL_MS: u64 = 350;
/// One decode step, in ms (about 100 ms at 10 tokens/s).
pub const DECODE_MS: u64 = 100;
/// Each sequence beyond the first in a batch adds this to a decode step.
pub const BATCH_MS: u64 = 15;
/// A move between a slot and the RAM pool, and one to or from a spill file.
pub const RAM_MOVE_MS: u64 = 40;
pub const SPILL_MOVE_MS: u64 = 900;

pub const KAI: AgentId = AgentId(1);

/// The user, as the foreground.
pub fn kai() -> Agent {
    Agent::builtin(KAI, Class::Foreground, 1)
}

pub fn budgets(tokens: u32, kv: u32, pool_mib: u32) -> Budgets {
    Budgets {
        tokens_per_day: tokens,
        kv_positions: kv,
        pool_mib,
    }
}

/// A packaged agent of `class`.
pub fn agent(id: u32, class: Class, weight: u32, b: Budgets) -> Agent {
    Agent {
        id: AgentId(id),
        class,
        weight,
        budgets: Some(b),
    }
}

/// What happened, in order, for the tests to read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Run(Vec<(RequestId, Kind, u32)>),
    Move(RequestId, Place, Place),
    End(RequestId, crate::End),
    Idle,
}

/// The scheduler with a clock.
pub struct Engine {
    pub sched: Scheduler,
    pub ms: u64,
    pub day: u32,
    pub log: Vec<Event>,
    /// Requests that were over, by the scheduler's word or the last quantum.
    pub done: Vec<RequestId>,
}

impl Engine {
    pub fn new(config: Config, agents: &[Agent]) -> Self {
        let mut sched = Scheduler::new(config);
        for agent in agents {
            sched.set_agent(*agent);
        }
        Self {
            sched,
            ms: 0,
            day: 0,
            log: Vec::new(),
            done: Vec::new(),
        }
    }

    pub fn now(&self) -> Now {
        Now {
            ms: self.ms,
            day: self.day,
        }
    }

    pub fn ask(&mut self, agent: AgentId, prompt: u32, max_new: u32) -> Result<RequestId, Refusal> {
        let now = self.now();
        self.sched.submit(
            Ask {
                agent,
                prompt_tokens: prompt,
                max_new_tokens: max_new,
            },
            now,
        )
    }

    /// One decision, carried out: the clock moves by what it costs.
    pub fn step(&mut self) -> Event {
        let now = self.now();
        let event = match self.sched.next(now) {
            Decision::Idle => Event::Idle,
            Decision::End { request, why, .. } => {
                self.done.push(request);
                Event::End(request, why)
            }
            Decision::Move { request, from, to } => {
                self.ms += move_ms(from, to);
                Event::Move(request, from, to)
            }
            Decision::Run(quanta) => {
                let decoding = quanta.iter().all(|q| q.kind == Kind::Decode);
                self.ms += match decoding {
                    true => DECODE_MS + BATCH_MS * (quanta.len() as u64 - 1),
                    false => quanta
                        .iter()
                        .map(|q| (PREFILL_MS * u64::from(q.tokens) / 32).max(1))
                        .sum(),
                };
                self.done
                    .extend(quanta.iter().filter(|q| q.last).map(|q| q.request));
                Event::Run(
                    quanta
                        .iter()
                        .map(|q| (q.request, q.kind, q.tokens))
                        .collect(),
                )
            }
        };
        self.log.push(event.clone());
        event
    }

    /// Steps until the scheduler is idle, at most `most` decisions.
    pub fn run_idle(&mut self, most: usize) {
        for _ in 0..most {
            if self.step() == Event::Idle {
                return;
            }
        }
        panic!("the scheduler did not go idle in {most} decisions");
    }
}

/// The time a move takes.
pub fn move_ms(from: Place, to: Place) -> u64 {
    match (from, to) {
        (Place::Spill, _) | (_, Place::Spill) => SPILL_MOVE_MS,
        (Place::Ram, _) | (_, Place::Ram) => RAM_MOVE_MS,
        _ => 0,
    }
}
