//! The local backend: the model the kernel mapped into this process, run by Kernel
//! AI's own engine (`infer::chat`) through `llmproto::local`, exactly as xtask runs
//! it on the host. It sizes the engine to the memory the plan line gives this
//! service (`kabi::model`), runs it on one worker per CPU online
//! ([`super::workers`]), keeps each request inside a deadline under its callers'
//! timeouts, and measures the speed. It sleeps 1 ms once per second of compute
//! rather than after every token: a call it serves is not charged to the burst
//! limit (L6), but one that waited while llm was busy can pass its deadline
//! before the engine's own limit, and the sleep (which the workers, spinning only
//! briefly, sleep through too) keeps every thread's burst short then. The engine
//! keeps its KV cache between requests, so a prompt that starts like the last one
//! (the script system prompt, say) is not computed again.

use alloc::format;
use alloc::string::String;
use infer::Next;
use infer::chat::{Budget, Engine, Step};
use infer::tagged::Tagged;
use llmproto::act::{self, ActError, ActRequest, Call};
use llmproto::chat::{self, Completion};
use llmproto::local;
use llmproto::script::{self, Answer, AnswerError, ScriptRequest};
use llmproto::tune::{self, TuneRequest};
use quant::{Parallel, Tier};
use user::{println, sys};

/// Longest one `ask` or `explain` may run: under the shell's 140 s and the
/// kernel's 150 s. A script request gets the shell's own limit for a round.
const DEADLINE_MS: u64 = 120_000;
/// The prompt `:model` times, and how many tokens it may take.
const TEST_PROMPT: &str = "Say hello in one short sentence.";
const TEST_TOKENS: u32 = 32;
/// Compute between two 1 ms sleeps: far under llm's 10 s burst limit, and a
/// thousandth of the time.
const SLEEP_EVERY_NS: u64 = 1_000_000_000;

/// How fast an answer came, and how much of its prompt the cache held.
#[derive(Clone, Copy)]
struct Speed {
    /// From the request to the first token of the answer.
    first_ns: u64,
    /// Tokens after the first, and the time they took.
    tokens: usize,
    decode_ns: u64,
    /// Prompt tokens taken from the cache, and computed.
    cached: usize,
    computed: usize,
    /// Answer tokens jump-forward fed as blocks, not decoded one by one.
    forced: usize,
}

impl Speed {
    fn per_second(&self) -> Option<f64> {
        (self.tokens > 0 && self.decode_ns > 0)
            .then(|| self.tokens as f64 * 1e9 / self.decode_ns as f64)
    }

    /// `tune`'s line: the prompt, the first token and the answer's rate.
    fn timing(&self) -> String {
        format!(
            "{} prompt tokens ({} from the cache), first token after {} ms, {}",
            self.cached + self.computed,
            self.cached,
            self.first_ns / 1_000_000,
            self.rate()
        )
    }

    fn rate(&self) -> String {
        match self.per_second() {
            Some(rate) => format!("{} more tokens at {rate:.2} tokens/s", self.tokens),
            None => String::from("too short to time"),
        }
    }

    fn describe(&self) -> String {
        let rate = match self.per_second() {
            Some(rate) => format!("{rate:.1} tokens/s"),
            None => String::from("too short to time"),
        };
        let forced = match self.forced {
            0 => String::new(),
            n => format!(", {n} of {} answer tokens forced", self.tokens + 1),
        };
        format!(
            "{rate}, {} of {} prompt tokens from the cache{forced}",
            self.cached,
            self.cached + self.computed
        )
    }
}

/// One request's clock: the deadline, checked after every step, a 1 ms sleep
/// once [`SLEEP_EVERY_NS`] of compute has passed since the last, and what
/// [`Speed`] reports.
struct Clock {
    started: u64,
    deadline: u64,
    first: Option<u64>,
    last: u64,
    tokens: usize,
    cached: usize,
    computed: usize,
    stopped: bool,
    slept: u64,
}

impl Clock {
    fn new(limit_ms: u64) -> Self {
        let started = sys::time_ns();
        Self {
            started,
            deadline: started.saturating_add(limit_ms.saturating_mul(1_000_000)),
            first: None,
            last: started,
            tokens: 0,
            cached: 0,
            computed: 0,
            stopped: false,
            slept: started,
        }
    }

    fn step(&mut self, step: Step) -> Next {
        let now = sys::time_ns();
        match step {
            Step::Cached(tokens) => self.cached = tokens,
            Step::Prompt => self.computed += 1,
            Step::Token(_) => {
                self.first.get_or_insert(now);
                self.last = now;
                self.tokens += 1;
            }
        }
        // A sleep blocks, which ends the burst the kernel times for this thread,
        // and the workers sleep on their futex meanwhile.
        if now - self.slept >= SLEEP_EVERY_NS {
            sys::sleep_ms(1);
            self.slept = sys::time_ns();
        }
        self.stopped = now >= self.deadline;
        match self.stopped {
            true => Next::Stop,
            false => Next::Continue,
        }
    }

    fn speed(&self) -> Option<Speed> {
        let first = self.first?;
        Some(Speed {
            first_ns: first - self.started,
            tokens: self.tokens.saturating_sub(1),
            decode_ns: self.last - first,
            cached: self.cached,
            computed: self.computed,
            forced: 0,
        })
    }
}

pub struct Local {
    engine: Engine<'static>,
    /// The tagged call's grammar over the pack's rows, built once.
    grammar: Tagged<'static>,
    last: Option<Speed>,
}

impl Local {
    /// Loads the model in `bytes` on the best tier this CPU and the kernel allow,
    /// to run on `workers`.
    pub fn load(bytes: &'static [u8], workers: &'static dyn Parallel) -> Result<Self, String> {
        let budget = Budget {
            kv_bytes: kabi::model::KV_BYTES as usize,
            other_bytes: kabi::model::ENGINE_OTHER_BYTES as usize,
        };
        let mut engine = Engine::load(bytes, Tier::best(), kabi::model::CONTEXT_CAP, budget)
            .map_err(|error| format!("{error}"))?;
        engine.set_parallel(workers);
        let grammar = local::act::grammar().map_err(|error| format!("{error}"))?;
        Ok(Self {
            engine,
            grammar,
            last: None,
        })
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
            "local model {} ({} tier, {} workers, context {}, {speed})",
            self.name(),
            about.tier.name(),
            about.workers,
            about.context
        )
    }

    /// The model's greedy answer to `request`, for `ask`, `:why` and `:model`.
    pub fn complete(&mut self, request: &chat::Request) -> Result<Completion, String> {
        let mut clock = Clock::new(DEADLINE_MS);
        let done = local::complete(&mut self.engine, request, |step| clock.step(step));
        self.last = clock.speed().or(self.last);
        let completion = done.map_err(|error| match error {
            infer::Error::ContextFull => {
                String::from("the question is too long for the model's context")
            }
            other => format!("the local model failed: {other}"),
        })?;
        if clock.stopped && clock.first.is_none() {
            return Err(format!(
                "the local model did not start answering within {} s",
                DEADLINE_MS / 1000
            ));
        }
        Ok(completion)
    }

    /// A script for the shell's `request`, within the time the shell gives one
    /// round and at most `script::SERVICE_TIMEOUT_MS`.
    pub fn script(&mut self, request: &ScriptRequest) -> Result<Answer, AnswerError> {
        let mut clock = Clock::new(request.timeout_ms.min(script::SERVICE_TIMEOUT_MS));
        let answer = local::script(&mut self.engine, request, |step| clock.step(step));
        let forced = self.engine.forced();
        self.last = clock
            .speed()
            .map(|speed| Speed { forced, ..speed })
            .or(self.last);
        answer
    }

    /// One tagged call for the shell's `request`, within the time it gives and
    /// at most `act::SERVICE_TIMEOUT_MS`; a script's code comes from the script
    /// prompt (`llmproto::local::act::routed`).
    pub fn act(&mut self, request: &ActRequest) -> Result<Call, ActError> {
        let mut clock = Clock::new(request.timeout_ms.min(act::SERVICE_TIMEOUT_MS));
        let call = local::act::routed(&mut self.engine, &self.grammar, request, |step| {
            clock.step(step)
        });
        let forced = self.engine.forced();
        self.last = clock
            .speed()
            .map(|speed| Speed { forced, ..speed })
            .or(self.last);
        call
    }

    /// Computes the script system prompt into the cache while no request waits
    /// (`llmproto::local::warm`). It asks `waiting` whether a request came at
    /// the start and then once a second of compute, in place of the 1 ms sleep;
    /// if one did, it stops, and the request carries on from what was computed.
    pub fn warm(&mut self, mut waiting: impl FnMut() -> bool) -> Result<(), String> {
        let mut slept = 0;
        local::warm(&mut self.engine, |_| {
            if sys::time_ns() - slept < SLEEP_EVERY_NS {
                return Next::Continue;
            }
            let came = waiting();
            slept = sys::time_ns();
            match came {
                true => Next::Stop,
                false => Next::Continue,
            }
        })
        .map_err(|error| local::describe(&error))
    }

    /// The model's answer to a `tune` request, under the answer's schema, within
    /// `tune::SERVICE_TIMEOUT_MS`. It prints how long the answer took, so a slow
    /// machine's demo shows its own speed.
    pub fn tune(&mut self, request: &TuneRequest) -> Result<Completion, String> {
        let mut clock = Clock::new(tune::SERVICE_TIMEOUT_MS);
        let done = local::tune(&mut self.engine, request, |step| clock.step(step));
        self.last = clock.speed().or(self.last);
        let timing = clock
            .speed()
            .map_or_else(|| String::from("no answer token"), |speed| speed.timing());
        let limit = match clock.stopped {
            true => format!(
                "; stopped at the {} s limit",
                tune::SERVICE_TIMEOUT_MS / 1000
            ),
            false => String::new(),
        };
        println!("tune: {timing}{limit}");
        done.map_err(|error| format!("the local model failed: {}", local::describe(&error)))
    }

    /// `:model`'s lines: the model, the engine, then one timed test prompt.
    pub fn report(&mut self, len: usize) -> String {
        let about = self.engine.about().clone();
        let c = about.config;
        let licence = about.license.as_deref().unwrap_or("not stated in the file");
        let mut lines = format!(
            "model: {}, {len} bytes, licence {licence}\n\
             engine: qwen3, {} layers, {} wide, {} tokens; context {} of {} ({} KiB of KV cache); {} tier, {} workers\n",
            self.name(),
            c.n_layer,
            c.n_embd,
            c.vocab,
            about.context,
            c.context_length,
            about.kv_bytes / 1024,
            about.tier.name(),
            about.workers
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
