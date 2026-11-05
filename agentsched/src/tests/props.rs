//! Properties over random workloads, on a seeded generator (splitmix64, the same
//! on every host): the rules hold after every decision, not only in the cases
//! thought of. A failing seed is the seed to rerun.

use super::fake::*;
use crate::*;
use std::collections::{BTreeMap, BTreeSet};
use std::format;
use std::vec;
use std::vec::Vec;

/// splitmix64, as `sched::sim::Rng` is.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// 0 to `n - 1`.
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn range(&mut self, low: u64, high: u64) -> u64 {
        low + self.below(high - low + 1)
    }

    fn percent(&mut self, p: u64) -> bool {
        self.below(100) < p
    }
}

/// The agents of one run: the user, a second foreground stream, an interactive agent,
/// some background agents with budgets, and the indexer.
fn agents(rng: &mut Rng) -> Vec<Agent> {
    let mut agents = vec![
        kai(),
        Agent::builtin(AgentId(10), Class::Foreground, 1),
        agent(
            20,
            Class::Interactive,
            rng.range(1, 4) as u32,
            budgets(rng.range(300, 5_000) as u32, 1_024, rng.range(1, 64) as u32),
        ),
        Agent::builtin(AgentId(40), Class::Index, 1),
    ];
    for i in 0..rng.range(1, 4) {
        let b = budgets(
            rng.range(200, 6_000) as u32,
            rng.range(64, 512) as u32,
            rng.range(1, 64) as u32,
        );
        agents.push(agent(
            30 + i as u32,
            Class::Background,
            rng.range(1, 5) as u32,
            b,
        ));
    }
    agents
}

fn config(rng: &mut Rng) -> Config {
    Config {
        kv_slots: rng.range(1, 4) as usize,
        pool_mib: [0, 6, 20, 512][rng.below(4) as usize],
        spill: rng.percent(70),
        batch: match rng.below(3) {
            0 => Batch::Serial,
            n => Batch::Up(n as u8 + 1),
        },
        batch_foreground: rng.percent(30),
        max_positions: 4_096,
    }
}

/// What the scheduler held before a decision.
#[derive(Clone, Copy)]
struct Was {
    class: Class,
    place: Place,
    agent: AgentId,
}

fn snapshot(s: &Scheduler) -> BTreeMap<RequestId, Was> {
    s.reqs
        .iter()
        .map(|(id, r)| {
            (
                *id,
                Was {
                    class: r.class,
                    place: r.place,
                    agent: r.agent,
                },
            )
        })
        .collect()
}

/// The legal moves of a state.
fn legal(from: Place, to: Place) -> bool {
    matches!(
        (from, to),
        (Place::Slot, Place::Ram | Place::Spill | Place::Cold)
            | (Place::Ram, Place::Spill | Place::Cold | Place::Slot)
            | (Place::Spill | Place::Cold, Place::Slot)
    )
}

/// One run: random asks and cancels between decisions, every rule checked after each.
fn run(seed: u64, steps: usize) -> Vec<Event> {
    let mut rng = Rng(seed);
    let cfg = config(&mut rng);
    let all = agents(&mut rng);
    let mut e = Engine::new(cfg, &all);
    let mut charged: BTreeMap<AgentId, u64> = BTreeMap::new();
    for step in 0..steps {
        // The world: asks, cancels, and the day turning.
        if rng.percent(35) {
            let who = all[rng.below(all.len() as u64) as usize].id;
            let prompt = rng.range(0, 150) as u32;
            let new = rng.range(0, 60) as u32;
            let _ = e.ask(who, prompt, new);
        }
        if rng.percent(3) && !e.sched.reqs.is_empty() {
            let at = rng.below(e.sched.reqs.len() as u64) as usize;
            let id = *e.sched.reqs.keys().nth(at).unwrap();
            e.sched.finish(id);
        }
        if rng.percent(1) {
            e.day += 1;
            charged.clear();
        }
        let before = snapshot(&e.sched);
        let best = before.values().map(|w| w.class).min();
        let slots_before = e.sched.resident();
        let event = e.step();
        let ctx = format!("seed {seed}, step {step}, {event:?}, {cfg:?}");
        match &event {
            Event::Idle => assert!(before.is_empty(), "idle with work: {ctx}"),
            Event::End(id, _) => assert!(before.contains_key(id), "{ctx}"),
            Event::Move(id, from, to) => {
                let was = before.get(id).unwrap_or_else(|| panic!("unknown: {ctx}"));
                assert_eq!(was.place, *from, "{ctx}");
                assert!(legal(*from, *to), "{ctx}");
                if *to == Place::Slot {
                    assert!(slots_before < cfg.kv_slots, "no free slot: {ctx}");
                }
            }
            Event::Run(q) => {
                assert!(!q.is_empty() && q.len() <= usize::from(MAX_BATCH), "{ctx}");
                let lead = before[&q[0].0];
                // The highest class that has anything waiting runs.
                assert_eq!(Some(lead.class), best, "class order: {ctx}");
                let mut agents_seen = BTreeSet::new();
                for (id, kind, tokens) in q {
                    let was = before.get(id).unwrap_or_else(|| panic!("unknown: {ctx}"));
                    assert_eq!(was.place, Place::Slot, "run outside a slot: {ctx}");
                    assert_eq!(was.class, lead.class, "mixed batch: {ctx}");
                    assert!(agents_seen.insert(was.agent), "one agent twice: {ctx}");
                    assert!((1..=BLOCK_TOKENS).contains(tokens), "{ctx}");
                    assert!(*kind == Kind::Decode || q.len() == 1, "{ctx}");
                    *charged.entry(was.agent).or_default() += u64::from(*tokens);
                }
                match cfg.batch {
                    Batch::Serial => assert_eq!(q.len(), 1, "serial: {ctx}"),
                    Batch::Up(n) => assert!(q.len() <= usize::from(n), "{ctx}"),
                }
                if q.len() > 1 && lead.class == Class::Foreground {
                    assert!(cfg.batch_foreground, "foreground batched: {ctx}");
                }
            }
        }
        // A deficit is only ever paid from what a visit gave.
        assert!(
            e.sched.agents.values().all(|a| a.deficit >= 0),
            "a deficit below zero: {ctx}"
        );
        // The memory rules hold after every decision.
        assert!(e.sched.resident() <= cfg.kv_slots, "slots: {ctx}");
        let pool = u64::from(cfg.pool_mib) << 20;
        assert!(e.sched.ram_bytes(None) <= pool, "pool: {ctx}");
        for a in &all {
            if let Some(b) = a.budgets {
                assert!(
                    e.sched.ram_bytes(Some(a.id)) <= u64::from(b.pool_mib) << 20,
                    "pool share: {ctx}"
                );
                // The day's tokens are counted, and never pass the budget.
                let used = e.sched.used_today(a.id).unwrap();
                assert!(used <= b.tokens_per_day, "over the day: {ctx}");
                assert_eq!(
                    u64::from(used),
                    charged.get(&a.id).copied().unwrap_or(0),
                    "counted: {ctx}"
                );
            }
        }
    }
    // With no new work, everything ends: nothing is lost or stuck.
    let mut guard = 0;
    while e.step() != Event::Idle {
        guard += 1;
        assert!(guard < 200_000, "seed {seed}: it never went idle");
    }
    assert_eq!(e.sched.pending(), 0);
    e.log
}

#[test]
fn the_rules_hold_after_every_decision_of_random_workloads() {
    for seed in 0..120 {
        run(seed, 500);
    }
}

#[test]
fn the_same_workload_gives_the_same_decisions() {
    for seed in [1, 7, 42, 1_000_003] {
        assert_eq!(run(seed, 400), run(seed, 400), "seed {seed}");
    }
}

/// Backlogged agents of one class, with the higher classes idle: over ten minutes each
/// one's share of the tokens is within 20% of its share of the weights (ADR 0013
/// section 2, the M10 floor reused).
#[test]
fn backlogged_agents_get_their_weighted_share_over_ten_minutes() {
    for seed in 0..10 {
        let mut rng = Rng(seed + 5_000);
        let n = rng.range(2, 4) as u32;
        let weights: Vec<u32> = (0..n).map(|_| rng.range(1, 3) as u32).collect();
        let big = budgets(u32::MAX, 4_096, 512);
        let all: Vec<Agent> = (0..n)
            .map(|i| agent(30 + i, Class::Background, weights[i as usize], big))
            .collect();
        let cfg = Config {
            kv_slots: n as usize,
            ..Config::default()
        };
        let mut e = Engine::new(cfg, &all);
        let mut used_at_start = BTreeMap::new();
        while e.ms < 10 * 60_000 {
            // Always work waiting, as a backlogged agent has.
            for a in &all {
                let waiting = e.sched.reqs.values().filter(|r| r.agent == a.id).count();
                if waiting == 0 {
                    e.ask(a.id, 32, 2_000).unwrap();
                }
            }
            e.step();
            if e.ms < 60_000 {
                used_at_start = all
                    .iter()
                    .map(|a| (a.id, e.sched.used_today(a.id).unwrap()))
                    .collect();
            }
        }
        let served: Vec<f64> = all
            .iter()
            .map(|a| f64::from(e.sched.used_today(a.id).unwrap() - used_at_start[&a.id]))
            .collect();
        let total: f64 = served.iter().sum();
        let weight_total: f64 = weights.iter().map(|w| f64::from(*w)).sum();
        for (i, got) in served.iter().enumerate() {
            let fair = f64::from(weights[i]) / weight_total;
            let share = got / total;
            assert!(
                share >= 0.8 * fair && share <= 1.2 * fair,
                "seed {seed}: agent {i} of weight {} got {share:.3}, fair {fair:.3}",
                weights[i]
            );
        }
    }
}

/// With the higher classes idle, no request waits longer than the aging bound plus the
/// quanta of the others that aged before it: a request in the lowest class is not
/// starved by a heavier agent, however heavy.
#[test]
fn no_background_request_waits_longer_than_the_aging_bound() {
    for seed in 0..16 {
        let mut rng = Rng(seed + 9_000);
        let n = rng.range(2, 5) as u32;
        let big = budgets(u32::MAX, 4_096, 512);
        let all: Vec<Agent> = (0..n)
            .map(|i| agent(30 + i, Class::Background, rng.range(1, 400) as u32, big))
            .collect();
        let cfg = Config {
            kv_slots: n as usize,
            ..Config::default()
        };
        let mut e = Engine::new(cfg, &all);
        let bound = AGING_MS + u64::from(n) * PREFILL_MS;
        let mut longest = 0;
        while e.ms < 20 * 60_000 {
            for a in &all {
                if !e.sched.reqs.values().any(|r| r.agent == a.id) && rng.percent(50) {
                    e.ask(a.id, rng.range(1, 64) as u32, rng.range(1, 1_900) as u32)
                        .unwrap();
                }
            }
            let waited = e
                .sched
                .reqs
                .values()
                .map(|r| e.ms.saturating_sub(r.waiting_since_ms))
                .max()
                .unwrap_or(0);
            longest = longest.max(waited);
            assert!(
                waited <= bound,
                "seed {seed}: waited {waited} ms, bound {bound}"
            );
            e.step();
        }
        assert!(longest > 0);
    }
}

/// A foreground request that arrives at any time runs within one quantum of arriving
/// (plus the moves its slot needs): nothing waits ahead of the user.
#[test]
fn the_foreground_waits_for_at_most_the_quantum_in_flight_and_its_own_slot() {
    for seed in 0..80 {
        let mut rng = Rng(seed + 20_000);
        let cfg = config(&mut rng);
        let all = agents(&mut rng);
        let mut e = Engine::new(cfg, &all);
        for _ in 0..300 {
            for a in all.iter().filter(|a| a.class != Class::Foreground) {
                if rng.percent(20) {
                    let _ = e.ask(a.id, rng.range(1, 100) as u32, rng.range(0, 40) as u32);
                }
            }
            if rng.percent(10) && e.sched.reqs.values().all(|r| r.class != Class::Foreground) {
                let user = e.ask(KAI, rng.range(1, 64) as u32, rng.range(1, 10) as u32);
                if let Ok(user) = user {
                    // Every decision from here to its first quantum is about the user's
                    // request: moves to make room for it, and then it runs.
                    let mut moves = 0;
                    loop {
                        match e.step() {
                            Event::Run(q) => {
                                assert_eq!(q[0].0, user, "seed {seed}: ran another first");
                                break;
                            }
                            Event::Move(..) => moves += 1,
                            // A request over its day or its time ends at no cost to the model.
                            Event::End(..) => {}
                            other => panic!("seed {seed}: {other:?} before the user's request"),
                        }
                        // A state to demote, a victim out, the user in.
                        assert!(moves <= 3 + e.sched.pending(), "seed {seed}: {moves} moves");
                    }
                }
            }
            e.step();
        }
    }
}
