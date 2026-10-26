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
use llmproto::act::{self, ActError, ActRequest, Call, History, Turn};
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

/// Prompt tokens a request must have computed for its prefill rate to count.
const MEASURED_TOKENS: usize = 32;
/// The prefill rate assumed before any is measured: a slow guest's.
const SLOW_TOKENS_PER_S: f64 = 10.0;
/// Time kept for writing a short script after its prompt: 40 tokens at a slow
/// guest's 8 tokens/s.
const SCRIPT_DECODE_NS: u64 = 5_000_000_000;

/// One decode of a request (a request may take two: `act` then its script), for
/// the profile `act` logs.
#[derive(Clone, Copy, Default)]
struct Phase {
    started: u64,
    cached: usize,
    computed: usize,
    /// When the last prompt token was computed (prefill's end).
    prefilled: u64,
    first: Option<u64>,
    last: u64,
    tokens: usize,
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
    /// Each decode, from its `Step::Cached`.
    phases: alloc::vec::Vec<Phase>,
    /// The prefill rate an earlier request or warm-up measured, tokens/s.
    known_rate: Option<f64>,
}

/// Prompt tokens a warm-up computed, and how fast.
struct Meter {
    started: u64,
    computed: usize,
    last: u64,
}

impl Meter {
    fn new() -> Self {
        let now = sys::time_ns();
        Self {
            started: now,
            computed: 0,
            last: now,
        }
    }

    fn step(&mut self, step: &Step) {
        if let Step::Prompt = step {
            self.computed += 1;
            self.last = sys::time_ns();
        }
    }

    fn rate(&self) -> Option<f64> {
        rate(self.computed, self.last - self.started)
    }
}

/// Tokens/s over `computed` prompt tokens in `ns`, once there are enough to
/// count.
fn rate(computed: usize, ns: u64) -> Option<f64> {
    (computed >= MEASURED_TOKENS).then(|| computed as f64 * 1e9 / ns.max(1) as f64)
}

impl Clock {
    fn new(limit_ms: u64, known_rate: Option<f64>) -> Self {
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
            phases: alloc::vec::Vec::new(),
            known_rate,
        }
    }

    fn step(&mut self, step: Step) -> Next {
        let now = sys::time_ns();
        match step {
            Step::Cached(tokens) => {
                self.cached = tokens;
                self.phases.push(Phase {
                    started: now,
                    cached: tokens,
                    prefilled: now,
                    last: now,
                    ..Phase::default()
                });
            }
            Step::Prompt => {
                self.computed += 1;
                if let Some(phase) = self.phases.last_mut() {
                    phase.computed += 1;
                    phase.prefilled = now;
                }
            }
            Step::Token(_) => {
                self.first.get_or_insert(now);
                self.last = now;
                self.tokens += 1;
                if let Some(phase) = self.phases.last_mut() {
                    phase.first.get_or_insert(now);
                    phase.last = now;
                    phase.tokens += 1;
                }
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

    /// Where the time went, decode by decode: `act: 2210 ms; call: 12+1003
    /// prompt tokens, prefill 1500 ms, 14 tokens in 600 ms; ...`.
    fn profile(&self, names: &[&str]) -> String {
        let ms = |ns: u64| ns / 1_000_000;
        let mut line = format!("{} ms", ms(sys::time_ns() - self.started));
        for (i, phase) in self.phases.iter().enumerate() {
            let name = names.get(i).copied().unwrap_or("decode");
            let decode = phase.first.map_or(0, |first| phase.last - first);
            line.push_str(&format!(
                "; {name}: {}+{} prompt tokens, prefill {} ms, {} tokens in {} ms",
                phase.cached,
                phase.computed,
                ms(phase.prefilled - phase.started),
                phase.tokens,
                ms(decode),
            ));
        }
        line
    }

    /// The prefill rate this request measured, its latest decode first.
    fn rate(&self) -> Option<f64> {
        self.phases
            .iter()
            .rev()
            .find_map(|phase| rate(phase.computed, phase.prefilled - phase.started))
    }

    /// Whether the time left can prefill `tokens` more and write a short script:
    /// at the prefill rate this request measured, else the last one measured
    /// (a request with its prefix cached computes too few tokens to measure),
    /// else a slow guest's 10 tokens/s.
    fn room_for(&self, tokens: usize) -> bool {
        let rate = self.rate().or(self.known_rate).unwrap_or(SLOW_TOKENS_PER_S);
        let need = (tokens as f64 / rate * 1e9) as u64 + SCRIPT_DECODE_NS;
        sys::time_ns().saturating_add(need) <= self.deadline
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

/// Which KV slot each prompt uses: the act prefix stays in one, the script
/// prompt (and `ask`, `explain`, `tune`, `:model`) in the other, so neither
/// evicts the other (S02-2).
pub const SLOTS: local::act::Slots = local::act::Slots::TWO;

pub struct Local {
    engine: Engine<'static>,
    /// The tagged call's grammar over the pack's rows, built once.
    grammar: Tagged<'static>,
    /// Conversation memory for `act`, in RAM only (P1 plan section 4).
    history: History,
    last: Option<Speed>,
    /// The last prefill rate measured, tokens/s, for [`Clock::room_for`].
    prefill_rate: Option<f64>,
    /// The model file, for its identity in the KV files' key.
    bytes: &'static [u8],
    /// That identity, once computed.
    model_id: Option<[u8; 32]>,
}

impl Local {
    /// Loads the model in `bytes` on the best tier this CPU and the kernel allow,
    /// to run on `workers`.
    pub fn load(bytes: &'static [u8], workers: &'static dyn Parallel) -> Result<Self, String> {
        // Each KV slot gets its share: the act prefix's and the script prompt's.
        let budget = Budget {
            kv_bytes: (kabi::model::KV_BYTES as usize) / kabi::model::KV_SLOTS,
            other_bytes: kabi::model::ENGINE_OTHER_BYTES as usize,
        };
        let mut engine = Engine::load(bytes, Tier::best(), kabi::model::CONTEXT_CAP, budget)
            .map_err(|error| format!("{error}"))?;
        engine.add_slot().map_err(|error| format!("{error}"))?;
        engine.set_clock(user::pool::cycles);
        engine.set_parallel(workers);
        let grammar = local::act::grammar().map_err(|error| format!("{error}"))?;
        Ok(Self {
            engine,
            grammar,
            history: History::new(),
            last: None,
            prefill_rate: None,
            bytes,
            model_id: None,
        })
    }

    /// The key slot `slot`'s `kai-kv 1` file is saved under. Until P2 task 10's
    /// model store names the model by its SHA-256, its identity is the file's
    /// FNV-1a64 and length (computed once, as `health` reports it), in the key's
    /// 32 bytes.
    fn kv_key(&mut self, slot: usize) -> infer::segments::Key {
        let bytes = self.bytes;
        let model_sha256 = *self.model_id.get_or_insert_with(|| {
            let mut fnv = kabi::model::Fnv1a::new();
            fnv.update(bytes);
            let mut id = [0u8; 32];
            id[..8].copy_from_slice(&fnv.finish().to_le_bytes());
            id[8..16].copy_from_slice(&(bytes.len() as u64).to_le_bytes());
            id
        });
        infer::segments::Key {
            model_sha256,
            tier: self.engine.about().tier,
            pack_id: match slot == SLOTS.act {
                true => local::act::pack_id(),
                false => local::script_pack_id(),
            },
            slot: slot as u8,
        }
    }

    /// **Interim** (S10-3): fills both KV slots from their `kai-kv 1` files in
    /// `kai-cache`, logging each. Returns whether the act prefix and the script
    /// prompt came back; a slot that did not is left to the idle warm-up, and
    /// saved after it.
    pub fn restore(&mut self) -> (bool, bool) {
        let mut restored = [false; 2];
        for (i, slot) in [SLOTS.act, SLOTS.script].into_iter().enumerate() {
            let started = sys::time_ns();
            let key = self.kv_key(slot);
            self.engine.use_slot(slot);
            let mut input = crate::kvfile::reader(slot as u64);
            let got = match slot == SLOTS.act {
                true => local::act::restore(&mut self.engine, &self.history, &key, &mut input),
                false => local::restore(&mut self.engine, &key, &mut input),
            };
            let ms = (sys::time_ns() - started) / 1_000_000;
            match got {
                Ok(n) => println!("local: KV slot {slot} restored, {n} positions, in {ms} ms"),
                Err(why) => println!("local: KV slot {slot} not restored ({why:?}) in {ms} ms"),
            }
            restored[i] = got.is_ok();
        }
        (restored[0], restored[1])
    }

    /// **Interim** (S10-3): writes KV slot `slot` as its `kai-kv 1` file, logging
    /// it; a failure (no `kai-cache`, a read-only disk) only costs the next
    /// boot its prefill.
    pub fn save(&mut self, slot: usize) {
        let started = sys::time_ns();
        let key = self.kv_key(slot);
        self.engine.use_slot(slot);
        let mut writer = crate::kvfile::Writer::new(slot as u64);
        let wrote = self
            .engine
            .save_slot(&key, &mut |bytes| writer.write(bytes));
        let ok = wrote.is_ok() && writer.finish();
        let ms = (sys::time_ns() - started) / 1_000_000;
        match ok {
            true => println!("local: KV slot {slot} saved in {ms} ms"),
            false => println!("local: KV slot {slot} not saved ({ms} ms)"),
        }
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
        self.engine.use_slot(SLOTS.script);
        let mut clock = Clock::new(DEADLINE_MS, self.prefill_rate);
        let done = local::complete(&mut self.engine, request, |step| clock.step(step));
        self.last = clock.speed().or(self.last);
        self.prefill_rate = clock.rate().or(self.prefill_rate);
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
        self.engine.use_slot(SLOTS.script);
        let mut clock = Clock::new(
            request.timeout_ms.min(script::SERVICE_TIMEOUT_MS),
            self.prefill_rate,
        );
        let answer = local::script(&mut self.engine, request, |step| clock.step(step));
        self.prefill_rate = clock.rate().or(self.prefill_rate);
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
        // The pool's figures and the picks' time from here on are this request's.
        crate::workers::report();
        self.engine.take_picking();
        let clock = core::cell::RefCell::new(Clock::new(
            request.timeout_ms.min(act::SERVICE_TIMEOUT_MS),
            self.prefill_rate,
        ));
        let call = local::act::routed(
            &mut self.engine,
            &self.grammar,
            request,
            &self.history,
            SLOTS,
            |tokens| clock.borrow().room_for(tokens),
            |step| clock.borrow_mut().step(step),
        );
        let clock = clock.into_inner();
        self.prefill_rate = clock.rate().or(self.prefill_rate);
        let forced = self.engine.forced();
        self.last = clock
            .speed()
            .map(|speed| Speed { forced, ..speed })
            .or(self.last);
        // S02-2's profile: the call's decode, then a script's, if any.
        let name = call.as_ref().map_or("none", |call| call.name.as_str());
        println!(
            "act: {name}, {}; pool: {}; picking tokens {} ms",
            clock.profile(&["call", "script"]),
            crate::workers::report(),
            crate::workers::ms(self.engine.take_picking())
        );
        call
    }

    /// Keeps a finished turn the shell wrote (`remember`), within history's
    /// budget. Returns a line for the log.
    pub fn remember(&mut self, turn: Turn) -> String {
        let dropped = local::act::remember(&self.engine, &mut self.history, turn);
        let budget = local::act::history_budget(&self.engine);
        let kept = self.history.len();
        let tokens = self.history.tokens();
        match dropped {
            true => format!("{kept} turns, {tokens} of {budget} tokens (the older half went)"),
            false => format!("{kept} turns, {tokens} of {budget} tokens"),
        }
    }

    /// `new`: forgets the conversation.
    pub fn forget(&mut self) {
        self.history.clear();
    }

    /// Computes the `act` prefix (pack, MODE turn, history) into the cache while
    /// no request waits; `waiting` as for [`Local::warm`].
    pub fn warm_act(&mut self, mut waiting: impl FnMut() -> bool) -> Result<(), String> {
        self.engine.use_slot(SLOTS.act);
        let mut slept = 0;
        let mut meter = Meter::new();
        let warmed = local::act::warm(&mut self.engine, &self.history, |step| {
            meter.step(&step);
            if sys::time_ns() - slept < SLEEP_EVERY_NS {
                return Next::Continue;
            }
            let came = waiting();
            slept = sys::time_ns();
            match came {
                true => Next::Stop,
                false => Next::Continue,
            }
        });
        self.prefill_rate = meter.rate().or(self.prefill_rate);
        warmed.map_err(|error| local::describe(&error))
    }

    /// Computes the script system prompt into the cache while no request waits
    /// (`llmproto::local::warm`). It asks `waiting` whether a request came at
    /// the start and then once a second of compute, in place of the 1 ms sleep;
    /// if one did, it stops, and the request carries on from what was computed.
    pub fn warm(&mut self, mut waiting: impl FnMut() -> bool) -> Result<(), String> {
        self.engine.use_slot(SLOTS.script);
        let mut slept = 0;
        let mut meter = Meter::new();
        let warmed = local::warm(&mut self.engine, |step| {
            meter.step(&step);
            if sys::time_ns() - slept < SLEEP_EVERY_NS {
                return Next::Continue;
            }
            let came = waiting();
            slept = sys::time_ns();
            match came {
                true => Next::Stop,
                false => Next::Continue,
            }
        });
        self.prefill_rate = meter.rate().or(self.prefill_rate);
        warmed.map_err(|error| local::describe(&error))
    }

    /// The model's answer to a `tune` request, under the answer's schema, within
    /// `tune::SERVICE_TIMEOUT_MS`. It prints how long the answer took, so a slow
    /// machine's demo shows its own speed.
    pub fn tune(&mut self, request: &TuneRequest) -> Result<Completion, String> {
        self.engine.use_slot(SLOTS.script);
        let mut clock = Clock::new(tune::SERVICE_TIMEOUT_MS, self.prefill_rate);
        let done = local::tune(&mut self.engine, request, |step| clock.step(step));
        self.last = clock.speed().or(self.last);
        self.prefill_rate = clock.rate().or(self.prefill_rate);
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
        self.engine.use_slot(SLOTS.script);
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
