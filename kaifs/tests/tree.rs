//! The crash gate for the B+tree (P2 task 4, third part): the tree over the real
//! allocator and the superblock rings, committed as kaifs commits. Each commit
//! writes the dirty nodes to free blocks, frees the nodes they replace (waiting
//! N+2), writes the allocator's part of its chain, FLUSH, the superblock naming
//! the tree and the chain in both rings, FLUSH.
//!
//! Workloads: every sequence of up to three of {put, overwrite, delete, commit},
//! then a commit, on trees of 0, 1,000 and 5,000 keys; and twelve random batches
//! of 120 actions, whose epochs pass 12 writes and get random subsets. Debug
//! builds run the smaller part. Every crash state
//! must mount the newest superblock whose roots verify, load the allocator, walk
//! a tree that verifies node by node with sound order, levels and fill, hold
//! exactly the items its generation committed, and reach no block the allocator
//! calls free or pending, never older than the last commit waited on.
//!
//! Deliberate bugs that must be caught: a replaced node reused in its own
//! transaction, and no flush before the superblock.
//!
//! `cargo test --release -p kaifs --test tree -- --nocapture`

use kaifs::allocator::{Allocator, run_of};
use kaifs::crash::{Options, Oracle, Recording, Rng, Tally, enumerate};
use kaifs::format::{self, BlockPtr, KIND_NODE, Superblock};
use kaifs::tree::{Child, Key, MAX_VALUE, Node, Store, Tree};
use kaifs::{Block, BlockDev, CsumKind, Error, Image, MemDisk, RecordingDisk};
use std::collections::BTreeMap;

const KIND: CsumKind = CsumKind::Seahash64;
const UUID: [u8; 16] = *b"kaifs tree gate.";

#[derive(Clone, Copy, Debug, Default)]
struct Faults {
    reuse_now: bool,
    skip_data_flush: bool,
}

/// The tree's view of the disk and the allocator for one transaction.
struct Disk<'a, D> {
    dev: &'a mut D,
    alloc: &'a mut Allocator,
    txn: u64,
    reuse_now: bool,
}

fn load<D: BlockDev>(dev: &mut D, ptr: &BlockPtr) -> Result<Node, Error> {
    if ptr.kind != KIND_NODE {
        return Err(Error::Corrupt("node pointer kind"));
    }
    let block = format::read_checked(dev, KIND, ptr, "node")?;
    let node = Node::decode(&block)?;
    match node.level() == ptr.level {
        true => Ok(node),
        false => Err(Error::Corrupt("node level")),
    }
}

impl<D: BlockDev> Store for Disk<'_, D> {
    fn load(&mut self, ptr: &BlockPtr) -> Result<Node, Error> {
        load(self.dev, ptr)
    }

    fn replaced(&mut self, ptr: BlockPtr) {
        match self.reuse_now {
            true => self.alloc.free_now(run_of(&ptr)),
            false => self.alloc.free_later(self.txn, run_of(&ptr)),
        }
    }

    fn store(&mut self, block: &Block, level: u8) -> Result<BlockPtr, Error> {
        let run = self.alloc.alloc(0)?;
        self.dev.write(run.addr, block)?;
        Ok(BlockPtr {
            addr: run.addr,
            birth: self.txn,
            blocks: 1,
            kind: KIND_NODE,
            level,
            flags: 0,
            csum: KIND.sum(block),
        })
    }
}

fn item_hash(key: &Key, value: &[u8]) -> u64 {
    let mut bytes = Vec::with_capacity(17 + value.len());
    bytes.extend_from_slice(&key.oid.to_le_bytes());
    bytes.push(key.kind);
    bytes.extend_from_slice(&key.off.to_le_bytes());
    bytes.extend_from_slice(value);
    seahash::hash(&bytes)
}

/// Order-independent digest and count of a map's items.
fn digest<'a>(items: impl IntoIterator<Item = (&'a Key, &'a Vec<u8>)>) -> (u64, u64) {
    items.into_iter().fold((0, 0), |(d, n), (k, v)| {
        (d.wrapping_add(item_hash(k, v)), n + 1)
    })
}

struct Run {
    dev: RecordingDisk,
    alloc: Allocator,
    tree: Tree,
    model: BTreeMap<Key, Vec<u8>>,
    generation: u64,
    faults: Faults,
    rng: Rng,
    expected: BTreeMap<u64, (u64, u64)>,
    waited: Vec<(usize, u64)>,
    base: MemDisk,
    blocks: u64,
}

#[derive(Clone, Copy, Debug)]
enum Action {
    Put,
    Overwrite,
    Delete,
    Commit,
}

const ACTIONS: [Action; 4] = [
    Action::Put,
    Action::Overwrite,
    Action::Delete,
    Action::Commit,
];

impl Run {
    fn setup(blocks: u64, keys: u64, seed: u64) -> Run {
        let mut dev = RecordingDisk::new(blocks);
        format::clear_rings(&mut dev, blocks).unwrap();
        let mut run = Run {
            dev,
            alloc: Allocator::new(format::data_area(blocks)),
            tree: Tree::default(),
            model: BTreeMap::new(),
            generation: 0,
            faults: Faults::default(),
            rng: Rng(seed),
            expected: BTreeMap::new(),
            waited: Vec::new(),
            base: MemDisk::default(),
            blocks,
        };
        for i in 0..keys {
            let key = Key::new(i * 2, (i % 3) as u8, i % 5);
            let value = run.value();
            run.insert(key, value);
            if i % 4096 == 4095 {
                run.commit();
            }
        }
        run.commit();
        run.checkpoint();
        run
    }

    fn checkpoint(&mut self) {
        self.dev.checkpoint();
        self.base = self.dev.disk.clone();
        self.waited = vec![(0, self.generation)];
    }

    /// A copy from the same disk, for one workload.
    fn fork(&self, seed: u64, faults: Faults) -> Run {
        let mut alloc = self.alloc.clone();
        alloc.faults = Default::default();
        Run {
            dev: self.dev.clone(),
            alloc,
            tree: self.tree.clone(),
            model: self.model.clone(),
            generation: self.generation,
            faults,
            rng: Rng(seed),
            expected: self.expected.clone(),
            waited: self.waited.clone(),
            base: self.base.clone(),
            blocks: self.blocks,
        }
    }

    fn value(&mut self) -> Vec<u8> {
        let len = [1, 16, 60, 300, MAX_VALUE][self.rng.below(5) as usize];
        (0..len).map(|_| self.rng.next_u64() as u8).collect()
    }

    fn store(&mut self) -> Disk<'_, RecordingDisk> {
        Disk {
            dev: &mut self.dev,
            alloc: &mut self.alloc,
            txn: self.generation + 1,
            reuse_now: self.faults.reuse_now,
        }
    }

    fn insert(&mut self, key: Key, value: Vec<u8>) {
        let mut tree = std::mem::take(&mut self.tree);
        tree.insert(key, value.clone(), &mut self.store()).unwrap();
        self.tree = tree;
        self.model.insert(key, value);
    }

    fn existing(&mut self) -> Option<Key> {
        let n = self.model.len() as u64;
        (n > 0).then(|| *self.model.keys().nth(self.rng.below(n) as usize).unwrap())
    }

    fn commit(&mut self) {
        let txn = self.generation + 1;
        let mut tree = std::mem::take(&mut self.tree);
        let root = tree.write_out(&mut self.store()).unwrap();
        self.tree = tree;
        let head = self.alloc.persist(&mut self.dev, KIND, txn).unwrap();
        if !self.faults.skip_data_flush {
            self.dev.flush().unwrap();
        }
        let mut sb = Superblock::new(UUID, self.blocks, txn);
        sb.volumes = root;
        sb.alloc = head;
        format::write_rings(&mut self.dev, &sb).unwrap();
        self.dev.flush().unwrap();
        self.alloc.committed(txn);
        self.generation = txn;
        self.expected.insert(txn, digest(&self.model));
        self.waited.push((self.dev.log.len(), txn));
    }

    fn act(&mut self, action: Action) {
        match action {
            Action::Put => {
                let key = Key::new(1 + 2 * self.rng.below(1 << 30), self.rng.below(3) as u8, 0);
                let value = self.value();
                self.insert(key, value);
            }
            Action::Overwrite => {
                if let Some(key) = self.existing() {
                    let value = self.value();
                    self.insert(key, value);
                }
            }
            Action::Delete => {
                if let Some(key) = self.existing() {
                    let mut tree = std::mem::take(&mut self.tree);
                    assert!(tree.remove(&key, &mut self.store()).unwrap().is_some());
                    self.tree = tree;
                    self.model.remove(&key);
                }
            }
            Action::Commit => self.commit(),
        }
    }

    fn enumerate(&self, rng: &mut Rng) -> Tally {
        let slots: Vec<u64> = format::all_slots(self.blocks).collect();
        let tear = |addr: u64| slots.contains(&addr);
        let recording = Recording {
            base: &self.base,
            log: &self.dev.log,
            waited: &self.waited,
            tear: &tear,
        };
        let mut oracle = TreeOracle {
            expected: &self.expected,
        };
        enumerate(&recording, &Options::default(), &mut oracle, rng)
    }
}

struct TreeOracle<'a> {
    expected: &'a BTreeMap<u64, (u64, u64)>,
}

/// Walks the subtree `ptr` names: every node verifies, keys stay within their
/// bounds, levels step down, every node below the root is at least a quarter
/// full, and no block is free or pending.
fn walk(
    image: &mut Image<'_>,
    alloc: &Allocator,
    ptr: &BlockPtr,
    lower: Option<Key>,
    upper: Option<Key>,
    root: bool,
    sum: &mut (u64, u64),
) -> Result<(), String> {
    let node = load(image, ptr).map_err(|e| format!("node {}: {e:?}", ptr.addr))?;
    if alloc.any_unused(ptr.addr, ptr.addr + 1) {
        return Err(format!("node {} is free or pending", ptr.addr));
    }
    let in_bounds = |k: &Key| lower.is_none_or(|l| *k >= l) && upper.is_none_or(|u| *k < u);
    match node {
        Node::Leaf(items) => {
            if !root && items.is_empty() {
                return Err(format!("empty leaf {}", ptr.addr));
            }
            for (k, v) in &items {
                if !in_bounds(k) {
                    return Err(format!("leaf {} holds a key outside its bounds", ptr.addr));
                }
                sum.0 = sum.0.wrapping_add(item_hash(k, v));
                sum.1 += 1;
            }
        }
        Node::Branch(_, children) => {
            if !root && children.len() < kaifs::tree::MAX_BRANCH / 4 {
                return Err(format!("branch {} is under a quarter full", ptr.addr));
            }
            for (i, (key, child)) in children.iter().enumerate() {
                let Child::Clean(p) = child else {
                    unreachable!()
                };
                let next = children.get(i + 1).map(|(k, _)| *k).or(upper);
                let low = if i == 0 { lower } else { Some(*key) };
                walk(image, alloc, p, low, next, false, sum)?;
            }
        }
    }
    Ok(())
}

impl Oracle for TreeOracle<'_> {
    fn judge(&mut self, image: &mut Image<'_>, min: u64) -> Result<(), String> {
        let found = format::read_rings(image).map_err(|e| format!("rings: {e:?}"))?;
        for sb in found {
            let roots_verify = format::read_checked(image, KIND, &sb.alloc, "chain").is_ok()
                && (sb.volumes.is_null()
                    || format::read_checked(image, KIND, &sb.volumes, "root").is_ok());
            if !roots_verify {
                continue;
            }
            let generation = sb.generation;
            if generation < min {
                return Err(format!("mounted generation {generation}, older than {min}"));
            }
            let alloc = Allocator::load(image, KIND, &sb.alloc, generation)
                .map_err(|e| format!("generation {generation}: allocator: {e:?}"))?;
            let mut sum = (0, 0);
            if !sb.volumes.is_null() {
                walk(image, &alloc, &sb.volumes, None, None, true, &mut sum)
                    .map_err(|e| format!("generation {generation}: {e}"))?;
            }
            let want = self
                .expected
                .get(&generation)
                .ok_or(format!("generation {generation} was never committed"))?;
            if sum != *want {
                return Err(format!(
                    "generation {generation}: {} items, not as committed ({})",
                    sum.1, want.1
                ));
            }
            return Ok(());
        }
        Err("no superblock whose roots verify".into())
    }
}

/// Every sequence of up to `len` actions.
fn sequences(len: usize) -> Vec<Vec<Action>> {
    let mut out = vec![Vec::new()];
    let mut frontier = vec![Vec::new()];
    for _ in 0..len {
        let mut next = Vec::new();
        for seq in &frontier {
            for action in ACTIONS {
                let mut longer: Vec<Action> = seq.clone();
                longer.push(action);
                next.push(longer);
            }
        }
        out.extend(next.iter().cloned());
        frontier = next;
    }
    out
}

fn sweep(keys: u64, blocks: u64, faults: Faults) -> Tally {
    let setup = Run::setup(blocks, keys, 0xC0FFEE ^ keys);
    let mut tally = Tally::default();
    let mut rng = Rng(42);
    for (i, seq) in sequences(3).iter().enumerate() {
        let mut run = setup.fork(1000 + i as u64, faults);
        for action in seq {
            run.act(*action);
        }
        run.act(Action::Commit);
        tally.add(&run.enumerate(&mut rng));
    }
    eprintln!(
        "kaifs tree: {keys:5} keys, every sequence of three, {faults:?}: {} crash states, {} violations",
        tally.states, tally.violations
    );
    tally
}

fn batches(keys: u64, blocks: u64, workloads: u64, actions: u64, faults: Faults) -> Tally {
    let setup = Run::setup(blocks, keys, 0xBA7C4 ^ keys);
    let mut tally = Tally::default();
    let mut rng = Rng(7);
    for w in 0..workloads {
        let mut run = setup.fork(5000 + w, faults);
        let mut pick = Rng(77 + w);
        for _ in 0..actions {
            let action = match pick.below(100) {
                0..=44 => Action::Put,
                45..=69 => Action::Overwrite,
                70..=84 => Action::Delete,
                _ => Action::Commit,
            };
            run.act(action);
        }
        run.act(Action::Commit);
        tally.add(&run.enumerate(&mut rng));
    }
    eprintln!(
        "kaifs tree: {keys:5} keys, {workloads} batches of {actions}, {faults:?}: {} crash states, {} violations",
        tally.states, tally.violations
    );
    tally
}

#[test]
fn every_crash_state_holds_a_committed_tree() {
    let debug = cfg!(debug_assertions);
    let mut total = Tally::default();
    total.add(&sweep(0, 4096, Faults::default()));
    total.add(&sweep(1000, 8192, Faults::default()));
    if !debug {
        total.add(&sweep(5000, 16384, Faults::default()));
    }
    total.add(&batches(
        1000,
        16384,
        if debug { 1 } else { 12 },
        120,
        Faults::default(),
    ));
    eprintln!(
        "kaifs tree total: {} crash states, {} violations",
        total.states, total.violations
    );
    assert_eq!(total.violations, 0, "{:#?}", total.examples);
}

#[test]
fn a_node_reused_in_its_own_transaction_is_caught() {
    let faults = Faults {
        reuse_now: true,
        ..Faults::default()
    };
    let tally = batches(1000, 16384, 1, 40, faults);
    eprintln!("  first: {:?}", tally.examples.first());
    assert!(tally.violations > 0);
}

#[test]
fn no_flush_before_the_superblock_is_caught() {
    let faults = Faults {
        skip_data_flush: true,
        ..Faults::default()
    };
    let tally = batches(1000, 16384, 1, 40, faults);
    eprintln!("  first: {:?}", tally.examples.first());
    assert!(tally.violations > 0);
}
