//! One look at the machine, as the host reads it: `METRICS` (CPUs, memory, each
//! process's pages and CPU time, the uptime) and the model's `health`. Plain
//! numbers, no text to parse, so a hostile or odd value changes only what the
//! window shows. The monitor turns two samples into rates itself.

use alloc::string::String;
use alloc::vec::Vec;

/// Bytes in a page.
pub const PAGE: u64 = 4096;

/// What the model is doing, as llm's `health` reports it
/// (`llmproto::health::State`); `Unknown` when the host could not ask.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelState {
    Waiting,
    Warming,
    Ready,
    Remote,
    Failed,
    NoModel,
    /// `health` did not answer in time: loading, or answering a request.
    Busy,
    Unknown,
}

impl ModelState {
    /// How the window says it.
    pub fn word(self) -> &'static str {
        match self {
            ModelState::Waiting => "waiting for its model",
            ModelState::Warming => "warming up",
            ModelState::Ready => "ready",
            ModelState::Remote => "answering from a server",
            ModelState::Failed => "failed",
            ModelState::NoModel => "no model",
            ModelState::Busy => "busy",
            ModelState::Unknown => "not reported",
        }
    }
}

/// The model's state and the last answer's decode speed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelStatus {
    pub state: ModelState,
    /// Tokens a second times ten; 0 before the first answer, and for a server.
    pub tps10: u32,
}

impl ModelStatus {
    pub const UNKNOWN: Self = Self {
        state: ModelState::Unknown,
        tps10: 0,
    };
}

/// One process in a sample (`kabi::metrics::ProcessInfo`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcSample {
    pub pid: u32,
    pub name: String,
    /// Pages counted against its quota.
    pub pages: u64,
    /// CPU time its threads have used since it started, in nanoseconds.
    pub cpu_ns: u64,
}

/// One sample (`kabi::metrics::Metrics`, and the model's status).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sample {
    /// Nanoseconds since boot when the kernel took it.
    pub uptime_ns: u64,
    pub cpus: u32,
    pub free_pages: u64,
    pub total_pages: u64,
    pub heap_bytes: u64,
    pub procs: Vec<ProcSample>,
    /// Processes the kernel's list had no room for.
    pub omitted: u32,
    pub model: ModelStatus,
}
