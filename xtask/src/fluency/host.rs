//! The stand-in OS for gate runs. Its values are fixed and unlike any default, so a
//! script that prints a made-up number is caught. They are also chosen so that no
//! answer a task asks to compute equals a raw value (61 migrations on 7 CPUs is 8 per
//! CPU, not 7), so skipping the computation fails. It records every policy change.

use kabi::{Metrics, policy};
use llmproto::script::{Answer, AnswerError, ScriptRequest};
use std::cell::{Cell, RefCell};
use std::time::Instant;

/// Where script requests go: the LLM server, or canned answers in tests.
pub type Transport = Box<dyn Fn(&ScriptRequest) -> Result<Answer, AnswerError>>;

/// The services `processes()` lists, as (pid, name).
pub const SERVICES: &[(u64, &str)] = &[(12, "shell"), (2, "llm"), (19, "crasher")];

pub struct GateHost {
    started: Instant,
    policy: Cell<u64>,
    /// Every `set_policy` call, as policy ids.
    pub changes: RefCell<Vec<u64>>,
    /// What each round returned.
    pub answers: RefCell<Vec<Result<Answer, AnswerError>>>,
    transport: Transport,
}

impl GateHost {
    pub fn new(transport: Transport) -> Self {
        Self {
            started: Instant::now(),
            policy: Cell::new(policy::INTERACTIVE),
            changes: RefCell::new(Vec::new()),
            answers: RefCell::new(Vec::new()),
            transport,
        }
    }
}

impl shellcore::Host for GateHost {
    fn now_ns(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }

    fn interrupted(&self) -> bool {
        false
    }

    fn heap_bytes(&self) -> u64 {
        0
    }

    fn metrics(&self) -> Result<Metrics, kabi::Error> {
        let mut m = Metrics::empty();
        m.cpus = 7;
        m.policy = self.policy.get();
        m.running_policy = self.policy.get();
        m.context_switches = 9876;
        m.ejections = 13;
        m.migrations = 61;
        m.free_frames = 12_345;
        m.total_frames = 32_768;
        m.heap_bytes = 2_345_678;
        m.uptime_ns = 4_321_000_000;
        for (slot, (pid, name)) in m.processes.iter_mut().zip(SERVICES) {
            slot.pid = *pid;
            slot.name[..name.len()].copy_from_slice(name.as_bytes());
            slot.name_len = name.len() as u64;
        }
        m.process_count = SERVICES.len() as u64;
        Ok(m)
    }

    fn set_policy(&self, id: u64) -> Result<(), kabi::Error> {
        self.changes.borrow_mut().push(id);
        self.policy.set(id);
        Ok(())
    }

    fn ask(&self, _: &str) -> Result<String, String> {
        Err("not used by the gate".into())
    }

    fn script(&self, request: &ScriptRequest) -> Result<Answer, AnswerError> {
        let answer = (self.transport)(request);
        self.answers.borrow_mut().push(answer.clone());
        answer
    }

    fn audit(&self, _: &str) {}
}
