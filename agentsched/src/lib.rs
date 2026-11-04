//! The model scheduler's policy (ADR 0013 section 2), as a pure decision crate: no
//! engine, no clock and no `llm` wiring (that comes after the v0.1 tag). Given the
//! requests waiting, the agents' budgets and the time, [`Scheduler::next`] says
//! what the model does next, and the caller does it and asks again.
//!
//! **Safe points.** One decision is one quantum: a prefill block of up to
//! [`BLOCK_TOKENS`] tokens, or one decode token (for each sequence of a batch). The
//! caller asks again after each, so a foreground request that arrives while a
//! background one runs waits for at most the quantum in flight.
//!
//! **Classes, strictly ordered** ([`Class`]): the user's own request first, then an
//! agent the user is watching, then background work, then embedding work. A higher
//! class always runs before a lower one. Within the foreground, the oldest request
//! first. Within the other classes, deficit round robin over tokens, weighted by
//! each agent's share, and aging: a request that has waited over [`AGING_MS`] in
//! its class goes to the front of that class, never above it.
//!
//! **Where the state goes.** A request runs only with its KV in a resident slot.
//! When there is no free slot, the victim is chosen idle holders first (a slot held
//! [`IDLE_HOLD_MS`] without decoding), then the lowest class, then the least
//! recently run, and its state goes, in this order: a RAM snapshot in the bounded
//! pool (within the agent's own pool share), else a spill file, else dropped (to be
//! prefilled again, and charged again). A full pool demotes its lowest-priority
//! snapshot first. Every move is a decision the caller carries out
//! ([`Decision::Move`]); a resident request that gets the model again needs none.
//!
//! **Budgets** are a packaged agent's, from its manifest (`kaiagent::Budgets`): a
//! day's tokens (prefill plus decode, counted here and reset when the day turns), the
//! KV positions one request may hold (refused at [`Scheduler::submit`], before any
//! prefill), the pool share, and [`REQUEST_MS`] of running time a request. The user's
//! foreground requests are never an agent's budget.
//!
//! **Batching.** [`Batch::Serial`] is the default: S13-2's fail result is serial
//! decode only. [`Batch::Up`] lets agents of one class decode together, up to 4
//! sequences, and the foreground decodes alone unless [`Config::batch_foreground`].
//! Only sequences already resident and ready to decode join, so a batch forms among
//! agents the round robin has served in turn; the others ride along without paying
//! the visit's deficit. Whether it pays is S13-2's measurement (on the runners, 1.00x
//! the throughput with 2 sequences and 1.60x with 4), which is why serial is the
//! default.
//!
//! **Starvation.** A lower class runs whenever every higher class is empty. Within
//! the classes below the foreground, a request waits at most [`AGING_MS`] plus a
//! quantum for each other request of its class; and while agents of a class all have
//! work, each one's share of the tokens stays within 20% of its share of the weights.
//! The user's request waits for the quantum in flight and the moves its slot needs.
//! The tests check these over random workloads.
//!
//! **The caller** carries out each [`Decision`] and calls [`Scheduler::next`] again.
//! A [`Decision::Run`] has already counted its tokens and moved its requests on; the
//! caller says [`Scheduler::finish`] for one the model stopped before its last
//! token. A request the scheduler ends (`End`) is gone from it.
//!
//! This crate is `no_std` + `alloc`: `llm` and the host tests run the same code.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

mod pick;
mod place;
mod sched;
#[cfg(test)]
mod tests;

pub use sched::Scheduler;

use kaiagent::Budgets;

/// Tokens in a prefill block: the longest a safe point is away (ADR 0013 section 2).
pub const BLOCK_TOKENS: u32 = 32;
/// What one KV position costs, in bytes (section 2: 144 KiB for the 4B model).
pub const KV_BYTES_PER_POSITION: u64 = 144 * 1024;
/// A request waiting longer than this in its class goes to its front.
pub const AGING_MS: u64 = 60_000;
/// A slot held this long without decoding is given up first.
pub const IDLE_HOLD_MS: u64 = 300_000;
/// Running time a request may take before it ends with a timeout.
pub const REQUEST_MS: u64 = 120_000;
/// The most sequences decoding together.
pub const MAX_BATCH: u8 = 4;
/// The most requests one agent may have waiting.
pub const MAX_QUEUED: usize = 64;

/// How urgent a request is: a lower class always runs first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Class {
    /// The user's own request, from the panel, the terminal or `c>`.
    Foreground,
    /// An agent the user started and is watching.
    Interactive,
    /// Goals, packaged background agents and `maint`.
    Background,
    /// Embedding work.
    Index,
}

impl Class {
    /// Every class, highest first.
    pub const ALL: [Class; 4] = [
        Class::Foreground,
        Class::Interactive,
        Class::Background,
        Class::Index,
    ];

    pub(crate) fn index(self) -> usize {
        self as usize
    }
}

/// An agent's badge: the identity `llm` keys queues, quotas and slots on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AgentId(pub u32);

/// A request the scheduler holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RequestId(pub u64);

/// An agent the scheduler knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Agent {
    pub id: AgentId,
    /// The class of every request it makes.
    pub class: Class,
    /// Its share against the others of its class, at least 1.
    pub weight: u32,
    /// Its manifest's budgets; none for the user and the built-in agents.
    pub budgets: Option<Budgets>,
}

impl Agent {
    /// The user, or a built-in agent: no daily budget and no pool share.
    pub fn builtin(id: AgentId, class: Class, weight: u32) -> Self {
        Self {
            id,
            class,
            weight: weight.max(1),
            budgets: None,
        }
    }

    /// A packaged agent, held to its manifest: `background` runs as background work and
    /// `interactive` as an agent the user is watching.
    pub fn packaged(id: AgentId, manifest: &kaiagent::Manifest, weight: u32) -> Self {
        let class = match manifest.class {
            kaiagent::Class::Background => Class::Background,
            kaiagent::Class::Interactive => Class::Interactive,
        };
        Self {
            id,
            class,
            weight: weight.max(1),
            budgets: Some(manifest.budgets),
        }
    }
}

/// Whether sequences decode together.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Batch {
    /// One sequence at a time: S13-2's fail result, so the default until it passes.
    Serial,
    /// Up to this many (2 to [`MAX_BATCH`]) of one class.
    Up(u8),
}

/// What the scheduler is built with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// KV slots resident at once: what the model's memory allows.
    pub kv_slots: usize,
    /// The whole pool of RAM snapshots, in MiB (ADR 0013 question 4).
    pub pool_mib: u32,
    /// Whether an evicted state may go to a spill file; without, it is dropped.
    pub spill: bool,
    pub batch: Batch,
    /// Whether the foreground may join a batch (only if S13-2 shows at most 10% slower).
    pub batch_foreground: bool,
    /// The most KV positions any request may hold: the engine's window.
    pub max_positions: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            kv_slots: 2,
            pool_mib: kaiagent::MAX_POOL_MIB,
            spill: true,
            batch: Batch::Serial,
            batch_foreground: false,
            max_positions: kaiagent::MAX_KV_POSITIONS,
        }
    }
}

/// The time a call is made at: the monotonic clock, and the day (the host's, which
/// turns at midnight and resets each agent's daily count).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Now {
    pub ms: u64,
    pub day: u32,
}

/// What an agent asks the model for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ask {
    pub agent: AgentId,
    /// Tokens to prefill. At least 1 unless `max_new_tokens` is.
    pub prompt_tokens: u32,
    /// Tokens at most to decode; 0 for an embedding.
    pub max_new_tokens: u32,
}

/// Why a request is refused when it is made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    UnknownAgent,
    /// Nothing to do: no prompt and nothing to decode.
    Empty,
    /// It would hold more KV positions than the agent's quota (or the window).
    TooLong {
        positions: u32,
        max: u32,
    },
    /// The agent's day is spent: `Over quota: <agent> used <used> tokens today`.
    OverQuota {
        used: u32,
        budget: u32,
    },
    /// The agent already has [`MAX_QUEUED`] requests waiting.
    QueueFull,
}

/// Where a request's KV state is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Place {
    /// In a resident slot: it can run, and a switch to it copies nothing.
    Slot,
    /// A snapshot in the RAM pool.
    Ram,
    /// A `kai-kv 1` file in the cache.
    Spill,
    /// Nowhere: not started, or dropped, and prefilled again before it continues.
    Cold,
}

/// What a quantum does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Prefill,
    Decode,
}

/// One sequence's share of a [`Decision::Run`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quantum {
    pub request: RequestId,
    pub agent: AgentId,
    pub kind: Kind,
    /// Tokens this quantum processes (a prefill block, or 1 to decode).
    pub tokens: u32,
    /// The request is done with this quantum (it made all its tokens).
    pub last: bool,
}

/// Why the scheduler ended a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    /// It ran over [`REQUEST_MS`].
    Timeout,
    /// The agent's day is spent.
    OverQuota { used: u32, budget: u32 },
}

/// What to do next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Nothing is waiting.
    Idle,
    /// A request is over; tell its agent. It is gone from the scheduler.
    End {
        request: RequestId,
        agent: AgentId,
        why: End,
    },
    /// Move a request's state, then ask again. `from` is where it is, `to` where it
    /// goes; from [`Place::Cold`] to [`Place::Slot`] it is prefilled again.
    Move {
        request: RequestId,
        from: Place,
        to: Place,
    },
    /// Run these quanta together (one, unless batching), then ask again.
    Run(alloc::vec::Vec<Quantum>),
}
