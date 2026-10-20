//! The local backend: the model the kernel mapped into this process, run by Kernel
//! AI's own engine (`infer::chat`). It sizes the engine to the memory the plan line
//! gives this service (`kabi::model`), sleeps 1 ms after every prompt token and
//! every answer token, so the burst limit never sees one long run, keeps each
//! request inside a deadline under its callers' timeouts, and measures the speed.

use alloc::format;
use alloc::string::String;
use infer::Next;
use infer::chat::{Budget, Ending, Engine, Step};
use llmproto::chat::{self, Completion};
use quant::Tier;
use user::sys;

/// Longest one request may run: under the shell's 140 s and the kernel's 150 s.
const DEADLINE_MS: u64 = 120_000;
/// The prompt `:model` times, and how many tokens it may take.
const TEST_PROMPT: &str = "Say hello in one short sentence.";
const TEST_TOKENS: u32 = 32;

/// How fast an answer came.
#[derive(Clone, Copy)]
struct Speed {
    /// From the request to the first token of the answer.
    first_ns: u64,
    /// Tokens after the first, and the time they took.
    tokens: usize,
    decode_ns: u64,
}

impl Speed {
    fn per_second(&self) -> Option<f64> {
        (self.tokens > 0 && self.decode_ns > 0)
            .then(|| self.tokens as f64 * 1e9 / self.decode_ns as f64)
    }

    fn describe(&self) -> String {
        match self.per_second() {
            Some(rate) => format!("{rate:.1} tokens/s"),
            None => String::from("too short to time"),
        }
    }
}

pub struct Local {
    engine: Engine<'static>,
    last: Option<Speed>,
}

impl Local {
    /// Loads the model in `bytes` on the best tier this CPU and the kernel allow.
    pub fn load(bytes: &'static [u8]) -> Result<Self, String> {
        let budget = Budget {
            kv_bytes: kabi::model::KV_BYTES as usize,
            other_bytes: kabi::model::ENGINE_OTHER_BYTES as usize,
        };
        let engine = Engine::load(bytes, Tier::best(), kabi::model::CONTEXT_CAP, budget)
            .map_err(|error| format!("{error}"))?;
        Ok(Self { engine, last: None })
    }

    /// The name the file gives the model.
    pub fn name(&self) -> &str {
        self.engine.about().name.as_deref().unwrap_or("(unnamed)")
    }

    /// What `health` says: the model, the tier, the context and the last speed.
    pub fn describe(&self) -> String {
        let about = self.engine.about();
        let speed = self.last.map_or_else(
            || String::from("no answer yet"),
            |speed| format!("last answer {}", speed.describe()),
        );
        format!(
            "local model {} ({} tier, context {}, {speed})",
            self.name(),
            about.tier.name(),
            about.context
        )
    }

    /// The model's answer to `request`, which must be one turn.
    pub fn complete(&mut self, request: &chat::Request) -> Result<Completion, String> {
        let (system, user) = request
            .single_turn()
            .ok_or("the local model answers one question at a time")?;
        let started = sys::time_ns();
        let deadline = started.saturating_add(DEADLINE_MS * 1_000_000);
        let (mut first, mut last) = (None, started);
        let max_new = request.max_tokens as usize;
        let reply = self
            .engine
            .answer(system, user, max_new, |step| {
                let now = sys::time_ns();
                if let Step::Token(_) = step {
                    first.get_or_insert(now);
                    last = now;
                }
                // A sleep blocks, which ends the burst the kernel is timing.
                sys::sleep_ms(1);
                if now >= deadline {
                    Next::Stop
                } else {
                    Next::Continue
                }
            })
            .map_err(|error| match error {
                infer::Error::ContextFull => {
                    String::from("the question is too long for the model's context")
                }
                other => format!("the local model failed: {other}"),
            })?;
        if reply.ending == Ending::Stopped && reply.tokens == 0 {
            return Err(format!(
                "the local model did not start answering within {} s",
                DEADLINE_MS / 1000
            ));
        }
        if let Some(first) = first {
            self.last = Some(Speed {
                first_ns: first - started,
                tokens: reply.tokens.saturating_sub(1),
                decode_ns: last - first,
            });
        }
        Ok(Completion {
            text: reply.text(),
            cut_off: reply.cut_off(),
        })
    }

    /// `:model`'s lines: the model, the engine, then one timed test prompt.
    pub fn report(&mut self, len: usize) -> String {
        let about = self.engine.about().clone();
        let c = about.config;
        let licence = about.license.as_deref().unwrap_or("not stated in the file");
        let mut lines = format!(
            "model: {}, {len} bytes, licence {licence}\n\
             engine: qwen3, {} layers, {} wide, {} tokens; context {} of {} ({} KiB of KV cache); {} tier\n",
            self.name(),
            c.n_layer,
            c.n_embd,
            c.vocab,
            about.context,
            c.context_length,
            about.kv_bytes / 1024,
            about.tier.name()
        );
        self.last = None;
        // No system prompt, so the test fits even a small model's context.
        let test = chat::Request::new(None, TEST_PROMPT, TEST_TOKENS);
        match self.complete(&test) {
            Ok(completion) => {
                let speed = self.last;
                let first = speed.map_or(0, |s| s.first_ns / 1_000_000);
                let rate = speed.map_or_else(|| String::from("no tokens"), |s| s.describe());
                let answer = chat::answer(&completion).unwrap_or_else(|why| why);
                lines.push_str(&format!(
                    "test: \"{TEST_PROMPT}\": first token after {first} ms, {rate}\n\
                     test answer: {}",
                    llmproto::text::printable(&answer, 120, 1)
                ));
            }
            Err(why) => lines.push_str(&format!("test: {why}")),
        }
        lines
    }
}
