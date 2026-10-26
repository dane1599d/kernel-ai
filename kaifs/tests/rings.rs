//! The crash gate for format 1's commit point (P2 task 4, first part): a store of
//! sixteen cells, copy-on-write, committed through the superblock rings as kaifs
//! commits. Each commit writes the cells it changes and a root naming all sixteen
//! to fresh blocks, FLUSH, the superblock to slot `generation mod 8` of both
//! rings, FLUSH. Every crash state of 400 such commits must mount the newest
//! superblock whose root verifies, hold every cell as that generation had it, and
//! hold no generation older than the last one waited on. Two deliberate bugs must
//! be caught: no flush between the cells and the superblock, and one slot for
//! every commit.
//!
//! Release builds run all 400 commits:
//! `cargo test --release -p kaifs --test rings -- --nocapture`

use kaifs::crash::{Options, Oracle, Recording, Rng, Tally, enumerate};
use kaifs::csum::Csum;
use kaifs::format::{self, BlockPtr, KIND_NODE, KIND_RECORD, PTR_BYTES, Superblock};
use kaifs::{BLOCK, Block, BlockDev, CsumKind, Image, MemDisk, RecordingDisk};
use std::collections::BTreeMap;

const BLOCKS: u64 = 8192;
const CELLS: usize = 16;
const UUID: [u8; 16] = *b"kaifs ring gate.";
const COMMITS: u64 = if cfg!(debug_assertions) { 40 } else { 400 };

#[derive(Clone, Copy, Debug, Default)]
struct Faults {
    /// No flush between a commit's cells and its superblock.
    skip_data_flush: bool,
    /// Every commit in ring A's first slot alone.
    one_slot: bool,
}

struct Toy {
    dev: RecordingDisk,
    next: u64,
    generation: u64,
    cells: [BlockPtr; CELLS],
    faults: Faults,
    expected: BTreeMap<u64, [Csum; CELLS]>,
    waited: Vec<(usize, u64)>,
    base: MemDisk,
}

fn cell_block(generation: u64, cell: usize, rng: &mut Rng) -> Block {
    let mut block = [0; BLOCK];
    block[..8].copy_from_slice(&generation.to_le_bytes());
    block[8] = cell as u8;
    for byte in block[16..].iter_mut().step_by(61) {
        *byte = rng.next_u64() as u8;
    }
    block
}

fn root_block(generation: u64, cells: &[BlockPtr; CELLS]) -> Block {
    let mut block = [0; BLOCK];
    block[..8].copy_from_slice(&generation.to_le_bytes());
    for (i, cell) in cells.iter().enumerate() {
        block[8 + i * PTR_BYTES..8 + (i + 1) * PTR_BYTES].copy_from_slice(&cell.encode());
    }
    block
}

fn csums(cells: &[BlockPtr; CELLS]) -> [Csum; CELLS] {
    cells.map(|c| c.csum)
}

impl Toy {
    fn new(faults: Faults) -> Toy {
        let mut toy = Toy {
            dev: RecordingDisk::new(BLOCKS),
            next: format::data_area(BLOCKS).0,
            generation: 0,
            cells: [BlockPtr::NULL; CELLS],
            faults,
            expected: BTreeMap::new(),
            waited: Vec::new(),
            base: MemDisk::default(),
        };
        format::clear_rings(&mut toy.dev, BLOCKS).unwrap();
        toy.commit(&mut Rng(1), 0);
        toy.dev.checkpoint();
        toy.base = toy.dev.disk.clone();
        toy.waited = vec![(0, toy.generation)];
        toy
    }

    fn put(&mut self, block: &Block, kind: u8) -> BlockPtr {
        let addr = self.next;
        self.next += 1;
        self.dev.write(addr, block).unwrap();
        BlockPtr {
            addr,
            birth: self.generation + 1,
            blocks: 1,
            kind,
            level: 0,
            flags: 0,
            csum: CsumKind::Seahash64.sum(block),
        }
    }

    fn commit(&mut self, rng: &mut Rng, changes: u64) {
        let generation = self.generation + 1;
        for _ in 0..changes {
            let cell = rng.below(CELLS as u64) as usize;
            let block = cell_block(generation, cell, rng);
            self.cells[cell] = self.put(&block, KIND_RECORD);
        }
        let root = self.put(&root_block(generation, &self.cells), KIND_NODE);
        if !self.faults.skip_data_flush {
            self.dev.flush().unwrap();
        }
        let mut sb = Superblock::new(UUID, BLOCKS, generation);
        sb.volumes = root;
        match self.faults.one_slot {
            true => self.dev.write(1, &sb.encode()).unwrap(),
            false => format::write_rings(&mut self.dev, &sb).unwrap(),
        }
        self.dev.flush().unwrap();
        self.generation = generation;
        self.expected.insert(generation, csums(&self.cells));
        self.waited.push((self.dev.log.len(), generation));
    }
}

/// Mounts as kaifs does: the newest superblock whose root verifies; then every
/// cell it names must verify and match what that generation committed.
struct ToyOracle<'a> {
    expected: &'a BTreeMap<u64, [Csum; CELLS]>,
}

impl Oracle for ToyOracle<'_> {
    fn judge(&mut self, image: &mut Image<'_>, min: u64) -> Result<(), String> {
        let found = format::read_rings(image).map_err(|e| format!("rings: {e:?}"))?;
        for sb in found {
            let Ok(root) = format::read_checked(image, sb.csum_kind, &sb.volumes, "root") else {
                continue;
            };
            let generation = sb.generation;
            if generation < min {
                return Err(format!("mounted generation {generation}, older than {min}"));
            }
            let expected = self
                .expected
                .get(&generation)
                .ok_or(format!("generation {generation} was never committed"))?;
            for (i, want) in expected.iter().enumerate() {
                let cell = BlockPtr::decode_at(&root[..], 8 + i * PTR_BYTES)
                    .map_err(|e| format!("cell {i}: {e:?}"))?;
                if cell.csum != *want {
                    return Err(format!(
                        "generation {generation}: cell {i} is not as committed"
                    ));
                }
                if !cell.is_null() {
                    format::read_checked(image, sb.csum_kind, &cell, "cell").map_err(|_| {
                        format!("generation {generation}: cell {i} does not verify")
                    })?;
                }
            }
            return Ok(());
        }
        Err("no superblock with a root that verifies".into())
    }
}

fn run(faults: Faults, commits: u64, seed: u64) -> (Tally, u64) {
    let mut toy = Toy::new(faults);
    let mut rng = Rng(seed);
    for _ in 0..commits {
        let changes = rng.below(12);
        toy.commit(&mut rng, changes);
    }
    let slots: Vec<u64> = format::all_slots(BLOCKS).collect();
    let tear = |addr: u64| slots.contains(&addr);
    let recording = Recording {
        base: &toy.base,
        log: &toy.dev.log,
        waited: &toy.waited,
        tear: &tear,
    };
    let mut oracle = ToyOracle {
        expected: &toy.expected,
    };
    let tally = enumerate(
        &recording,
        &Options::default(),
        &mut oracle,
        &mut Rng(seed ^ 7),
    );
    (tally, toy.dev.writes)
}

#[test]
fn every_crash_state_of_ring_commits_mounts_a_committed_generation() {
    let (tally, writes) = run(Faults::default(), COMMITS, 42);
    eprintln!(
        "kaifs rings: {COMMITS} commits, {writes} writes, {} crash states, {} violations",
        tally.states, tally.violations
    );
    assert_eq!(tally.violations, 0, "{:#?}", tally.examples);
    assert!(tally.states > COMMITS * 100);
}

#[test]
fn the_enumerator_catches_a_missing_flush() {
    let faults = Faults {
        skip_data_flush: true,
        ..Faults::default()
    };
    let (tally, _) = run(faults, 20, 43);
    eprintln!(
        "kaifs rings, no flush before the superblock: {} states, {} violations; first: {:?}",
        tally.states,
        tally.violations,
        tally.examples.first()
    );
    assert!(tally.violations > 0);
}

#[test]
fn the_enumerator_catches_one_slot_for_every_commit() {
    let faults = Faults {
        one_slot: true,
        ..Faults::default()
    };
    let (tally, _) = run(faults, 20, 44);
    eprintln!(
        "kaifs rings, one slot for every commit: {} states, {} violations; first: {:?}",
        tally.states,
        tally.violations,
        tally.examples.first()
    );
    assert!(tally.violations > 0);
}
