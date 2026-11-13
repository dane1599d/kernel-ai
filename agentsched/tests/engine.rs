//! The scheduler drives the real engine (ADR 0013 section 2): `agentsched` decides
//! what runs and where each agent's state goes, a rig carries the decisions out on an
//! `infer::Engine` over the fixture model (a snapshot into the RAM pool, a restore
//! into a slot, a prefill block, a decode token), and every answer must come out
//! exactly as it does when the agent has the model to itself, token for token,
//! however the requests were interleaved and moved.
//!
//! This is the host half of the integration the `llm` wiring does after the v0.1
//! tag, and the rig is that wiring in miniature: one engine slot for each resident
//! request, a pool of snapshots keyed by request, a prefill quantum as `reply`
//! stopped after a block, and a decode quantum as one token (`reply` for the first,
//! `continue_reply` after it).

use agentsched::{
    Agent, AgentId, Ask, Class, Config, Decision, Kind, Now, Place, Quantum, RequestId, Scheduler,
};
use infer::chat::{Budget, Engine, Role, Step, Turn};
use infer::fixture::{self, Spec};
use infer::segments::{Key, Snapshot, spill_pack_id};
use infer::{Next, Sampler};
use quant::Tier;
use std::collections::BTreeMap;

const USER: u32 = 1;
const BOT_A: u32 = 5;
const BOT_B: u32 = 6;
/// Tokens an answer asks for.
const MAX_NEW: usize = 12;
const BUDGET: Budget = Budget {
    kv_bytes: 64 << 20,
    other_bytes: 64 << 20,
};

/// A request the rig runs: who asked what, and where its state is.
struct Job {
    agent: u32,
    user: String,
    /// The engine slot it holds while it is resident.
    slot: Option<usize>,
    /// Its state while it waits in the RAM pool.
    snapshot: Snapshot,
    /// The last token it picked, not yet run through the model.
    pending: Option<u32>,
    tokens: Vec<u32>,
    /// The quanta it has been given, in order.
    kinds: Vec<Kind>,
    sampler: Sampler,
}

struct Rig<'a> {
    engine: Engine<'a>,
    sched: Scheduler,
    now: Now,
    jobs: BTreeMap<RequestId, Job>,
    /// Engine slots holding no resident request.
    free: Vec<usize>,
    tier: Tier,
    /// Request ids in the order they finished.
    finished: Vec<RequestId>,
    /// Moves made: out of a slot into the pool, and back.
    saved: usize,
    restored: usize,
}

impl<'a> Rig<'a> {
    fn new(bytes: &'a [u8], tier: Tier, slots: usize) -> Self {
        let mut engine = Engine::load(bytes, tier, 256, BUDGET).unwrap();
        for _ in 1..slots {
            engine.add_slot().unwrap();
        }
        let mut sched = Scheduler::new(Config {
            kv_slots: slots,
            ..Config::default()
        });
        sched.set_agent(Agent::builtin(AgentId(USER), Class::Foreground, 1));
        sched.set_agent(Agent::builtin(AgentId(BOT_A), Class::Background, 1));
        sched.set_agent(Agent::builtin(AgentId(BOT_B), Class::Background, 1));
        Self {
            engine,
            sched,
            now: Now { ms: 0, day: 1 },
            jobs: BTreeMap::new(),
            free: (0..slots).rev().collect(),
            tier,
            finished: Vec::new(),
            saved: 0,
            restored: 0,
        }
    }

    /// A pool snapshot's key: bound to the agent's request and to no engine slot, since
    /// the request goes back to whichever slot is free.
    fn key(&self, request: RequestId, agent: u32) -> Key {
        Key {
            model_sha256: [3; 32],
            tier: self.tier,
            pack_id: spill_pack_id(agent, request.0),
            slot: 0,
        }
    }

    fn submit(&mut self, agent: u32, user: &str) -> RequestId {
        let prompt = self
            .engine
            .conversation_tokens(&[Turn::new(Role::User, user)]);
        let ask = Ask {
            agent: AgentId(agent),
            prompt_tokens: prompt as u32,
            max_new_tokens: MAX_NEW as u32,
        };
        let id = self.sched.submit(ask, self.now).unwrap();
        let job = Job {
            agent,
            user: user.to_string(),
            slot: None,
            snapshot: Snapshot::new(),
            pending: None,
            tokens: Vec::new(),
            kinds: Vec::new(),
            sampler: Sampler::greedy(),
        };
        self.jobs.insert(id, job);
        id
    }

    /// Carries out a move the scheduler asked for.
    fn moved(&mut self, request: RequestId, from: Place, to: Place) {
        let agent = self.jobs[&request].agent;
        let key = self.key(request, agent);
        match (from, to) {
            (Place::Cold, Place::Slot) => {
                let slot = self.free.pop().expect("a free slot for a request");
                self.jobs.get_mut(&request).unwrap().slot = Some(slot);
            }
            (Place::Slot, Place::Ram) => {
                let job = self.jobs.get_mut(&request).unwrap();
                let slot = job.slot.take().expect("a resident request");
                self.engine.use_slot(slot);
                self.engine.snapshot_slot(&key, &mut job.snapshot).unwrap();
                self.free.push(slot);
                self.saved += 1;
            }
            (Place::Ram, Place::Slot) => {
                let slot = self.free.pop().expect("a free slot for a restore");
                let job = self.jobs.get_mut(&request).unwrap();
                self.engine.use_slot(slot);
                let turns = [Turn::new(Role::User, &job.user)];
                self.engine
                    .restore_snapshot(&key, &turns, &job.snapshot)
                    .unwrap();
                job.slot = Some(slot);
                self.restored += 1;
            }
            other => panic!("a move this pool never makes: {other:?}"),
        }
    }

    /// Runs one quantum in the request's slot.
    fn run(&mut self, q: Quantum) {
        let job = self.jobs.get_mut(&q.request).unwrap();
        let slot = job.slot.expect("a quantum runs in a resident request");
        self.engine.use_slot(slot);
        let turns = [Turn::new(Role::User, &job.user)];
        job.kinds.push(q.kind);
        match q.kind {
            // A block of the prompt: stop after the first token of the block that ran.
            // The cache holds it, and asking for the same reply again goes on.
            Kind::Prefill => {
                let stopped = self
                    .engine
                    .reply(&turns, MAX_NEW, &mut job.sampler, None, |step| match step {
                        Step::Prompt => Next::Stop,
                        _ => Next::Continue,
                    })
                    .unwrap();
                assert!(stopped.bytes.is_empty(), "a prefill quantum made a token");
            }
            Kind::Decode => {
                let mut picked = None;
                let mut note = |step: Step| {
                    if let Step::Token(token) = step {
                        picked = Some(token);
                    }
                    Next::Continue
                };
                match job.pending {
                    // The first token: the prompt is in the cache, and `reply` runs only
                    // its last token again, which makes the logits the pick needs.
                    None => {
                        self.engine
                            .reply(&turns, 1, &mut job.sampler, None, &mut note)
                            .unwrap();
                    }
                    Some(pending) => {
                        self.engine
                            .continue_reply(pending, 1, &mut job.sampler, &mut note)
                            .unwrap();
                    }
                }
                match picked {
                    Some(token) => {
                        job.tokens.push(token);
                        job.pending = Some(token);
                    }
                    // The model ended the answer itself: the scheduler is told, as the
                    // wiring tells it.
                    None => {
                        self.sched.finish(q.request);
                        self.finished.push(q.request);
                        self.free.extend(job.slot.take());
                        return;
                    }
                }
            }
        }
        self.now.ms += 10 * u64::from(q.tokens);
        if q.last {
            // The scheduler has dropped it: its slot is free for the next request.
            self.finished.push(q.request);
            self.free.extend(job.slot.take());
        }
    }

    /// One decision of the scheduler, carried out; false when nothing waits.
    fn step(&mut self) -> bool {
        match self.sched.next(self.now) {
            Decision::Idle => false,
            Decision::End { request, why, .. } => panic!("{request:?} ended: {why:?}"),
            Decision::Move { request, from, to } => {
                self.moved(request, from, to);
                true
            }
            Decision::Run(quanta) => {
                for q in quanta {
                    self.run(q);
                }
                true
            }
        }
    }

    fn run_to_idle(&mut self) {
        for _ in 0..100_000 {
            if !self.step() {
                return;
            }
        }
        panic!("the scheduler never went idle");
    }

    fn position(&self, request: RequestId) -> usize {
        self.finished
            .iter()
            .position(|done| *done == request)
            .expect("a finished request")
    }
}

/// What an agent's answer is when it has the model to itself.
fn alone(bytes: &[u8], tier: Tier, user: &str) -> Vec<u32> {
    let mut engine = Engine::load(bytes, tier, 256, BUDGET).unwrap();
    let mut tokens = Vec::new();
    engine
        .reply(
            &[Turn::new(Role::User, user)],
            MAX_NEW,
            &mut Sampler::greedy(),
            None,
            |step| {
                if let Step::Token(token) = step {
                    tokens.push(token);
                }
                Next::Continue
            },
        )
        .unwrap();
    tokens
}

/// A prompt of a few blocks, different for each agent.
fn text(words: &str, times: usize) -> String {
    vec![words; times].join(" ")
}

fn check_exact(rig: &Rig<'_>, bytes: &[u8]) {
    for (request, job) in &rig.jobs {
        let want = alone(bytes, rig.tier, &job.user);
        assert!(
            want.len() >= 3,
            "{request:?}: an answer of {} tokens",
            want.len()
        );
        assert_eq!(
            job.tokens,
            want,
            "{} (agent {}) answered differently than alone",
            rig.tier.name(),
            job.agent
        );
    }
}

#[test]
fn the_user_arriving_while_two_agents_are_mid_sentence_goes_first_and_everyone_is_exact() {
    let bytes = fixture::build(&Spec::tiny());
    for tier in Tier::all() {
        let mut rig = Rig::new(&bytes, tier, 2);
        let a = rig.submit(
            BOT_A,
            &text("the quick brown fox jumps over the lazy dog", 2),
        );
        let b = rig.submit(BOT_B, &text("pack my box with five dozen liquor jugs", 2));
        // Let the agents get going: A is three tokens into its answer.
        while rig.jobs[&a].tokens.len() < 3 {
            assert!(rig.step());
        }
        // The user types: every slot is held, so an agent's state goes to the pool.
        let user = rig.submit(USER, &text("how many cpus are there", 2));
        rig.run_to_idle();
        check_exact(&rig, &bytes);
        // The person's request finished before either agent's.
        assert!(rig.position(user) < rig.position(a), "{}", tier.name());
        assert!(rig.position(user) < rig.position(b), "{}", tier.name());
        // Someone's state really did leave its slot and come back.
        assert!(rig.saved >= 1 && rig.restored >= 1, "{}", tier.name());
        assert_eq!(rig.saved, rig.restored, "every state that left came back");
    }
}

#[test]
fn the_user_arriving_in_the_middle_of_an_agents_prompt_is_the_same() {
    let bytes = fixture::build(&Spec::tiny());
    for tier in Tier::all() {
        // One slot: the user's request takes it from an agent that has run one block of
        // its prompt, and the agent goes on from the cache the pool gave back.
        let mut rig = Rig::new(&bytes, tier, 1);
        let a = rig.submit(
            BOT_A,
            &text("the quick brown fox jumps over the lazy dog", 2),
        );
        while rig.jobs[&a].kinds.is_empty() {
            assert!(rig.step());
        }
        assert!(rig.jobs[&a].tokens.is_empty(), "still in its prompt");
        let user = rig.submit(USER, &text("how many cpus are there", 2));
        rig.run_to_idle();
        check_exact(&rig, &bytes);
        assert!(rig.position(user) < rig.position(a), "{}", tier.name());
        assert!(rig.saved >= 1 && rig.restored >= 1, "{}", tier.name());
    }
}

#[test]
fn two_agents_sharing_one_slot_swap_state_through_the_pool_and_stay_exact() {
    let bytes = fixture::build(&Spec::tiny());
    let tier = Tier::best();
    let mut rig = Rig::new(&bytes, tier, 1);
    rig.submit(
        BOT_A,
        &text("the quick brown fox jumps over the lazy dog", 2),
    );
    rig.submit(BOT_B, &text("pack my box with five dozen liquor jugs", 2));
    rig.run_to_idle();
    check_exact(&rig, &bytes);
    assert!(rig.saved >= 1, "the agents took turns in one slot");
    assert_eq!(rig.saved, rig.restored);
}
