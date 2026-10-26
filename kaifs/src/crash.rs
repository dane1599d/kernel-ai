//! The crash enumerator (ADR 0004's S04-1 method), which gates every kaifs layer.
//!
//! A run is recorded on a [`RecordingDisk`]: the disk it started from, its log of
//! writes and flushes, and for each commit the log position after its second
//! flush. The log splits into flush epochs. A crash keeps every write before the
//! last completed flush, plus any subset of the open epoch's writes, so the
//! enumerator rebuilds:
//! - every subset of an epoch of up to [`Options::exhaustive`] writes; above that,
//!   every prefix and [`Options::random_subsets`] random subsets;
//! - every write the caller chooses (the superblock slots) torn at each 512-byte
//!   step, with every subset (or [`Options::torn_subsets`] random ones) of the
//!   epoch's other writes.
//!
//! Each state goes to an [`Oracle`], with the oldest generation it may hold: the
//! newest commit whose second flush completed before the crash.
//!
//! [`RecordingDisk`]: crate::RecordingDisk

use crate::Block;
use crate::dev::{Image, MemDisk, Op};
use std::rc::Rc;
use std::string::String;
use std::vec::Vec;

/// Judges one crash state.
pub trait Oracle {
    /// `image` is the disk the crash left; it may hold no commit older than
    /// `min_generation`. Err says what is wrong.
    fn judge(&mut self, image: &mut Image<'_>, min_generation: u64) -> Result<(), String>;

    /// The writes of one epoch are now durable: every later state holds them.
    /// An oracle that remembers what it verified forgets what these touch.
    fn durable(&mut self, _written: &[u64]) {}
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub exhaustive: usize,
    pub random_subsets: usize,
    pub torn_subsets: usize,
}

impl Default for Options {
    /// S04-1's: every subset up to 12 writes, 4,096 random ones above, and every
    /// (or 64 random) subsets of the others beside a torn write.
    fn default() -> Self {
        Options {
            exhaustive: 12,
            random_subsets: 4096,
            torn_subsets: 64,
        }
    }
}

/// Marsaglia's xorshift64, for repeatable random choices.
#[derive(Clone, Debug)]
pub struct Rng(pub u64);

impl Rng {
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0 | 1;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// A number below `n`, or 0 when `n` is 0.
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n.max(1)
    }
}

/// Crash states checked, and what failed.
#[derive(Clone, Debug, Default)]
pub struct Tally {
    pub states: u64,
    pub violations: u64,
    /// The first few violations, described.
    pub examples: Vec<String>,
}

impl Tally {
    pub fn add(&mut self, other: &Tally) {
        self.states += other.states;
        self.violations += other.violations;
        for example in &other.examples {
            self.note(example.clone());
        }
    }

    fn note(&mut self, example: String) {
        if self.examples.len() < 10 {
            self.examples.push(example);
        }
    }
}

/// A recorded run.
pub struct Recording<'a> {
    /// The disk the log starts from.
    pub base: &'a MemDisk,
    pub log: &'a [Op],
    /// For each commit: the log position after its second flush, and its
    /// generation. The base's own commit sits at position 0.
    pub waited: &'a [(usize, u64)],
    /// Which writes to tear, by address: the superblock slots.
    pub tear: &'a dyn Fn(u64) -> bool,
}

type Write = (u64, Rc<Block>);

/// The log's flush epochs: the index of each epoch's first op, and its writes.
fn epochs(log: &[Op]) -> Vec<(usize, Vec<Write>)> {
    let mut epochs = Vec::new();
    let mut current = (0, Vec::new());
    for (i, op) in log.iter().enumerate() {
        match op {
            Op::Write(addr, block) => current.1.push((*addr, block.clone())),
            Op::Flush => epochs.push(core::mem::replace(&mut current, (i + 1, Vec::new()))),
        }
    }
    epochs.push(current);
    epochs
}

fn subsets(n: usize, exhaustive: usize, random: usize, rng: &mut Rng) -> Vec<Vec<bool>> {
    if n <= exhaustive {
        return (0u64..1 << n)
            .map(|mask| (0..n).map(|i| mask & (1 << i) != 0).collect())
            .collect();
    }
    let mut masks: Vec<Vec<bool>> = (0..=n).map(|k| (0..n).map(|i| i < k).collect()).collect();
    masks.extend((0..random).map(|_| (0..n).map(|_| rng.next_u64() & 1 == 1).collect()));
    masks
}

fn judge(
    tally: &mut Tally,
    oracle: &mut dyn Oracle,
    image: &mut Image<'_>,
    min: u64,
    what: impl FnOnce() -> String,
) {
    tally.states += 1;
    if let Err(why) = oracle.judge(image, min) {
        tally.violations += 1;
        tally.note(std::format!("{}: {why}", what()));
    }
}

/// Checks every crash state of `rec` with `oracle`.
pub fn enumerate(
    rec: &Recording,
    options: &Options,
    oracle: &mut dyn Oracle,
    rng: &mut Rng,
) -> Tally {
    let mut tally = Tally::default();
    let mut durable = rec.base.clone();
    let mut written = Vec::new();
    for (start, writes) in epochs(rec.log) {
        let min = rec
            .waited
            .iter()
            .filter(|(at, _)| *at <= start)
            .map(|(_, g)| *g)
            .max()
            .unwrap_or(0);
        let n = writes.len();
        for mask in subsets(n, options.exhaustive, options.random_subsets, rng) {
            let mut image = Image::new(&durable);
            for ((addr, block), _) in writes.iter().zip(&mask).filter(|(_, on)| **on) {
                image.apply(*addr, block);
            }
            judge(&mut tally, oracle, &mut image, min, || {
                std::format!("epoch at op {start}, writes {mask:?}")
            });
        }
        for (t, (addr, block)) in writes.iter().enumerate() {
            if !(rec.tear)(*addr) {
                continue;
            }
            let others: Vec<usize> = (0..n).filter(|&i| i != t).collect();
            let masks = match others.len() <= options.exhaustive {
                true => subsets(others.len(), options.exhaustive, 0, rng),
                false => subsets(others.len(), 0, options.torn_subsets, rng),
            };
            for mask in &masks {
                for step in 1..crate::BLOCK / 512 {
                    let mut image = Image::new(&durable);
                    for (i, _) in others.iter().zip(mask).filter(|(_, on)| **on) {
                        image.apply(writes[*i].0, &writes[*i].1);
                    }
                    image.apply_torn(*addr, block, step * 512);
                    judge(&mut tally, oracle, &mut image, min, || {
                        std::format!(
                            "epoch at op {start}, block {addr} torn after {} bytes",
                            step * 512
                        )
                    });
                }
            }
        }
        written.clear();
        for (addr, block) in writes {
            written.push(addr);
            durable.put(addr, block);
        }
        oracle.durable(&written);
    }
    tally
}
