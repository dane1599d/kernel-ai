//! The crash gate for the allocator (P2 task 4, second part). A filesystem of
//! allocations alone: each commit allocates runs of every size, frees some it
//! holds, writes its part of the allocator chain, FLUSH, the superblock naming the
//! chain in both rings, FLUSH. Over 3,000 commits the chain condenses to a new base
//! every 64 deltas. Every crash state must mount the newest superblock whose chain
//! head verifies, load that chain, and rebuild exactly the free and pending space
//! its generation committed, never older than the last commit waited on.
//!
//! Deliberate bugs: a delta that leaves out its own block, and a new base that
//! frees the old chain at once, must be caught; reuse at N+1 must not be (ADR 0004
//! §3: two flushes a commit make it safe).
//!
//! `cargo test --release -p kaifs --test alloc -- --nocapture`

use kaifs::allocator::{Allocator, Faults, State};
use kaifs::buddy::Run;
use kaifs::crash::{Options, Oracle, Recording, Rng, Tally, enumerate};
use kaifs::format::{self, Superblock};
use kaifs::{BlockDev, CsumKind, Image, MemDisk, RecordingDisk};
use std::collections::BTreeMap;

const BLOCKS: u64 = 1 << 16;
const UUID: [u8; 16] = *b"kaifs alloc gate";
const COMMITS: u64 = if cfg!(debug_assertions) { 300 } else { 3000 };
const KIND: CsumKind = CsumKind::Seahash64;

struct Toy {
    dev: RecordingDisk,
    alloc: Allocator,
    generation: u64,
    held: Vec<Run>,
    expected: BTreeMap<u64, State>,
    waited: Vec<(usize, u64)>,
    base: MemDisk,
}

impl Toy {
    fn new(faults: Faults) -> Toy {
        let mut dev = RecordingDisk::new(BLOCKS);
        format::clear_rings(&mut dev, BLOCKS).unwrap();
        let mut alloc = Allocator::new(format::data_area(BLOCKS));
        alloc.faults = faults;
        let mut toy = Toy {
            dev,
            alloc,
            generation: 0,
            held: Vec::new(),
            expected: BTreeMap::new(),
            waited: Vec::new(),
            base: MemDisk::default(),
        };
        toy.commit(&mut Rng(1), false);
        toy.dev.checkpoint();
        toy.base = toy.dev.disk.clone();
        toy.waited = vec![(0, toy.generation)];
        toy
    }

    fn commit(&mut self, rng: &mut Rng, change: bool) {
        let txn = self.generation + 1;
        if change {
            for _ in 0..rng.below(8) {
                let level = [0, 0, 0, 0, 1, 2, 3, 4, 6, 9][rng.below(10) as usize];
                if let Ok(run) = self.alloc.alloc(level) {
                    self.held.push(run);
                }
            }
            for _ in 0..rng.below(6) {
                if !self.held.is_empty() {
                    let run = self
                        .held
                        .swap_remove(rng.below(self.held.len() as u64) as usize);
                    self.alloc.free_later(txn, run);
                }
            }
        }
        let head = self.alloc.persist(&mut self.dev, KIND, txn).unwrap();
        self.dev.flush().unwrap();
        let mut sb = Superblock::new(UUID, BLOCKS, txn);
        sb.alloc = head;
        format::write_rings(&mut self.dev, &sb).unwrap();
        self.dev.flush().unwrap();
        self.alloc.committed(txn);
        self.generation = txn;
        self.expected.insert(txn, self.alloc.state());
        self.waited.push((self.dev.log.len(), txn));
    }
}

/// Free and pending space together: what N+1 reuse does not change.
fn unused(state: &State) -> Vec<(u64, u64)> {
    let mut all: Vec<(u64, u64)> = state.free.clone();
    all.extend(state.pending.iter().map(|&(_, s, l)| (s, l)));
    all.sort_unstable();
    let mut out: Vec<(u64, u64)> = Vec::new();
    for (s, l) in all {
        match out.last_mut() {
            Some((os, ol)) if *os + *ol == s => *ol += l,
            _ => out.push((s, l)),
        }
    }
    out
}

struct AllocOracle<'a> {
    expected: &'a BTreeMap<u64, State>,
    faults: Faults,
    /// Compare free and pending space exactly, or only their union.
    exact: bool,
}

impl Oracle for AllocOracle<'_> {
    fn judge(&mut self, image: &mut Image<'_>, min: u64) -> Result<(), String> {
        let found = format::read_rings(image).map_err(|e| format!("rings: {e:?}"))?;
        for sb in found {
            if format::read_checked(image, KIND, &sb.alloc, "head").is_err() {
                continue;
            }
            let generation = sb.generation;
            if generation < min {
                return Err(format!("mounted generation {generation}, older than {min}"));
            }
            let loaded = Allocator::load_with(image, KIND, &sb.alloc, generation, self.faults)
                .map_err(|e| format!("generation {generation}: the chain does not load: {e:?}"))?;
            let want = self
                .expected
                .get(&generation)
                .ok_or(format!("generation {generation} was never committed"))?;
            let got = loaded.state();
            let same = match self.exact {
                true => got == *want,
                false => unused(&got) == unused(want),
            };
            if !same {
                return Err(format!(
                    "generation {generation}: the allocator is not as committed"
                ));
            }
            return Ok(());
        }
        Err("no superblock whose allocator chain verifies".into())
    }
}

fn run(faults: Faults, commits: u64, seed: u64, exact: bool) -> Tally {
    let mut toy = Toy::new(faults);
    let mut rng = Rng(seed);
    for _ in 0..commits {
        let change = rng.below(8) != 0;
        toy.commit(&mut rng, change);
    }
    let slots: Vec<u64> = format::all_slots(BLOCKS).collect();
    let tear = |addr: u64| slots.contains(&addr);
    let recording = Recording {
        base: &toy.base,
        log: &toy.dev.log,
        waited: &toy.waited,
        tear: &tear,
    };
    let mut oracle = AllocOracle {
        expected: &toy.expected,
        faults,
        exact,
    };
    enumerate(
        &recording,
        &Options::default(),
        &mut oracle,
        &mut Rng(seed ^ 9),
    )
}

#[test]
fn every_crash_state_rebuilds_the_committed_allocator() {
    let tally = run(Faults::default(), COMMITS, 42, true);
    eprintln!(
        "kaifs alloc: {COMMITS} commits, {} crash states, {} violations",
        tally.states, tally.violations
    );
    assert_eq!(tally.violations, 0, "{:#?}", tally.examples);
}

fn faulty(faults: Faults, exact: bool) -> Tally {
    let tally = run(faults, 400, 43, exact);
    eprintln!(
        "kaifs alloc fault {faults:?}: {} states, {} violations; first: {:?}",
        tally.states,
        tally.violations,
        tally.examples.first()
    );
    tally
}

#[test]
fn a_delta_without_its_own_block_is_caught() {
    let faults = Faults {
        unlogged_self: true,
        ..Faults::default()
    };
    assert!(faulty(faults, true).violations > 0);
}

#[test]
fn freeing_the_old_chain_at_once_is_caught() {
    let faults = Faults {
        free_chain_now: true,
        ..Faults::default()
    };
    assert!(faulty(faults, true).violations > 0);
}

#[test]
fn reuse_at_n_plus_one_is_safe() {
    let faults = Faults {
        reuse_next: true,
        ..Faults::default()
    };
    assert_eq!(faulty(faults, false).violations, 0);
}
