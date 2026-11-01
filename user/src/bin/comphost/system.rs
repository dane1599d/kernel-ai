//! The System monitor's `kai:system` reads (P4 task 9): when the shown version
//! has a read waiting and the last sample is old enough, comphost reads the
//! kernel's `METRICS`, asks the llm service for its `health`, and gives the app
//! the sample through `samples.on-sample`, a call of its own like a window's
//! input.
//!
//! A read is never held here. It waits in the instance's own queue until it can
//! be answered in the same step, so a reloaded, resumed or swapped instance
//! starts with none, and tickets (which count from 0 for each instance) never
//! reach an instance that did not ask. A read waiting is also use: the app does
//! not nap, and the app stops asking when it has nothing to show (its window
//! closed), which lets it nap.
//!
//! Only the process the system image names [`MONITOR_APP`] is granted
//! `kai:system` (its plan line, never its package), and only it has a route to
//! llm, which is used for `health` and nothing else.

use alloc::string::String;
use alloc::vec::Vec;
use kabi::Error;
use kabi::handle::Handle;
use livecore::host::{Env, Host};
use user::{println, sys};
use wasmhost::LiveInstance;
use wasmhost::system::{model_status_of, sample_of};

/// The one app the image grants `kai:system`: the plan line's name for it, which
/// is its component's.
pub const MONITOR_APP: &str = "monitor-window";

/// Least time between two samples, in milliseconds: the monitor wants a picture
/// each second, not as many as it can ask for.
const PERIOD_MS: u64 = 1_000;
/// How long `health` may take before the model is shown busy. The service answers
/// one request at a time, and loading or answering one is one long call; an idle
/// service replies at once, so a short wait only costs the window a little while
/// the model is busy (lane C's point on a call that blocks input).
const HEALTH_MS: u64 = 100;
/// Room for llm's `health` reply.
const REPLY: usize = 1024;

pub struct Source {
    /// The route to llm: the second route, so the plan line's `uses=` must list
    /// `desktop` then `llm` (`uses=desktop+llm`) and nothing between them. The
    /// order matters, and the image's plan line for [`MONITOR_APP`] pins it with
    /// a test; with no second route the model is not reported.
    llm: Option<Handle>,
    /// The earliest time the next sample may be taken, in milliseconds.
    next_ms: u64,
    reply: Vec<u8>,
    /// Whether the log has said that the first sample reached the app.
    said: bool,
}

impl Source {
    pub fn new() -> Self {
        Self {
            llm: user::route(1),
            next_ms: 0,
            reply: alloc::vec![0u8; REPLY],
            said: false,
        }
    }

    /// Whether the shown version has a read waiting. A standby's reads are
    /// dropped, so a version rehearsed beside the shown one asks for nothing.
    pub fn waiting<E: Env<Instance = LiveInstance>>(&self, host: &mut Host<E>) -> bool {
        let mut waiting = false;
        host.each_instance(|instance, _, shown| {
            if let LiveInstance::V2(core) = instance {
                if shown {
                    waiting = core.system_waiting();
                } else {
                    drop(core.take_system());
                }
            }
        });
        waiting
    }

    /// How long until a waiting read may be answered, for the host's wait; 0 if
    /// it may be now, `None` when none waits.
    pub fn wait_ms<E: Env<Instance = LiveInstance>>(
        &self,
        host: &mut Host<E>,
        now_ms: u64,
    ) -> Option<u64> {
        self.waiting(host)
            .then(|| self.next_ms.saturating_sub(now_ms))
    }

    /// Answers the waiting reads if one is due; whether it did.
    pub fn pump<E: Env<Instance = LiveInstance>>(
        &mut self,
        host: &mut Host<E>,
        now_ms: u64,
    ) -> bool {
        if self.wait_ms(host, now_ms) != Some(0) {
            return false;
        }
        let sample = self.sample();
        self.next_ms = now_ms.saturating_add(PERIOD_MS);
        let bytes = sample.as_ref().map(Vec::len);
        let answered = crate::ui::serve_shown(host, |core| {
            for ticket in core.take_system() {
                core.system_sample(ticket, sample.clone())?;
            }
            Ok(())
        });
        match (answered, bytes) {
            (None, _) => println!("comphost: the system sample did not reach the app"),
            // Said once, so a log shows that the app was given the machine.
            (Some(_), Ok(len)) if !self.said => {
                self.said = true;
                println!("comphost: the first system sample reached the monitor ({len} bytes)");
            }
            _ => {}
        }
        true
    }

    /// The sample's bytes, or why there are none.
    fn sample(&mut self) -> Result<Vec<u8>, String> {
        let model = self.model();
        let metrics = sys::metrics().map_err(|error| {
            println!("comphost: the kernel's metrics were not read: {error:?}");
            String::from("the system could not be read")
        })?;
        Ok(sample_of(&metrics, model).encode())
    }

    /// The model's state: what llm's `health` says, busy if it did not answer in
    /// time, not reported if it could not be asked.
    fn model(&mut self) -> systemabi::ModelStatus {
        let Some(route) = self.llm else {
            return systemabi::ModelStatus::UNKNOWN;
        };
        match sys::call_route(route, b"health", &mut self.reply, HEALTH_MS) {
            Ok(len) => {
                let text = &self.reply[..len.min(self.reply.len())];
                model_status_of(&String::from_utf8_lossy(text))
            }
            Err(Error::TimedOut) => systemabi::ModelStatus {
                state: systemabi::ModelState::Busy,
                tps10: 0,
            },
            Err(_) => systemabi::ModelStatus::UNKNOWN,
        }
    }
}
