//! P2 task 4's figures (ADR 0004 S04-1's bars, on kaifs proper):
//! - bytes written per byte changed, for 4 KiB random overwrites, 64 a commit, on
//!   100,000 keys of 4 KiB records: at most 3.0x;
//! - the same inside 128 KiB records, reported only (ADR 0004 Consequences);
//! - a snapshot's cost: no extra flush and at most one extra block a commit.
//!
//! Release builds: `cargo test --release -p kaifs --test measure -- --nocapture`

use kaifs::check::{Cache, check};
use kaifs::crash::Rng;
use kaifs::{Fs, Key, RecordingDisk};

const KEYS: u64 = 100_000;

fn record(rng: &mut Rng, len: usize) -> Vec<u8> {
    (0..len).map(|_| rng.next_u64() as u8).collect()
}

fn key(n: u64) -> Key {
    Key::new(n, 1, 0)
}

/// A filesystem of `keys` records of `len` bytes, committed.
fn with_records(blocks: u64, keys: u64, len: usize) -> Fs<RecordingDisk> {
    let mut fs = Fs::format(RecordingDisk::new(blocks), [3; 16], "measure").unwrap();
    let mut rng = Rng(11);
    for n in 0..keys {
        fs.put_record(key(n), &record(&mut rng, len)).unwrap();
        if n % 4096 == 4095 {
            fs.commit().unwrap();
            fs.io.dev.checkpoint();
        }
    }
    fs.commit().unwrap();
    fs.io.dev.checkpoint();
    fs
}

/// Blocks written and flushes for `rounds` commits of `per_commit` overwrites at
/// random keys, each a record of `len` bytes.
fn overwrite_rounds(
    fs: &mut Fs<RecordingDisk>,
    keys: u64,
    len: usize,
    per_commit: u64,
    rounds: u64,
) -> (u64, u64) {
    let mut rng = Rng(99);
    let (writes, flushes) = (fs.io.dev.writes, fs.io.dev.flushes);
    for _ in 0..rounds {
        for _ in 0..per_commit {
            let n = rng.below(keys);
            fs.put_record(key(n), &record(&mut rng, len)).unwrap();
        }
        fs.commit().unwrap();
        fs.io.dev.checkpoint();
    }
    (fs.io.dev.writes - writes, fs.io.dev.flushes - flushes)
}

#[test]
fn write_amplification_of_4k_random_overwrites() {
    let rounds = 50;
    let mut fs = with_records(1 << 18, KEYS, 4096);
    // A few rounds first, so the figure is the steady state, not the first commits.
    overwrite_rounds(&mut fs, KEYS, 4096, 64, 10);
    let (writes, flushes) = overwrite_rounds(&mut fs, KEYS, 4096, 64, rounds);
    let changed = rounds * 64;
    let amp = writes as f64 / changed as f64;
    eprintln!(
        "kaifs write-amp: 4 KiB overwrites, 64 a commit, {KEYS} keys of 4 KiB records: {amp:.2}x \
         ({writes} blocks for {changed} changed; {:.1} blocks and {} flushes a commit) (bar <= 3.0x)",
        writes as f64 / rounds as f64,
        flushes / rounds
    );
    assert!(amp <= 3.0, "{amp:.2}x");
    assert_eq!(flushes, 2 * rounds);
    check(&mut fs, &mut Cache::default()).unwrap();
}

#[test]
fn write_amplification_inside_128k_records() {
    let (keys, rounds) = (1_000, 20);
    let mut fs = with_records(1 << 17, keys, 128 << 10);
    let (writes, _) = overwrite_rounds(&mut fs, keys, 128 << 10, 64, rounds);
    eprintln!(
        "kaifs write-amp: 4 KiB changes inside 128 KiB records, 64 a commit, {keys} keys: {:.1}x (reported only)",
        writes as f64 / (rounds * 64) as f64
    );
}

#[test]
fn a_snapshot_costs_no_flush_and_at_most_one_block() {
    // Each round mounts two copies of the same disk, makes the same 64 overwrites
    // on both and commits; only the second takes a snapshot first.
    let disk = with_records(1 << 18, KEYS, 4096).into_dev();
    let (mut extra_blocks, mut extra_flushes, mut worst) = (0i64, 0i64, 0i64);
    let rounds = 40;
    for round in 0..rounds {
        let mut costs = [(0u64, 0u64); 2];
        for (i, snapshot) in [false, true].into_iter().enumerate() {
            let mut fs = Fs::mount(disk.clone()).unwrap();
            let mut rng = Rng(1000 + round);
            let (w, f) = (fs.io.dev.writes, fs.io.dev.flushes);
            for _ in 0..64 {
                let n = rng.below(KEYS);
                fs.put_record(key(n), &record(&mut rng, 4096)).unwrap();
            }
            if snapshot {
                fs.snapshot().unwrap();
            }
            fs.commit().unwrap();
            costs[i] = (fs.io.dev.writes - w, fs.io.dev.flushes - f);
        }
        let blocks = costs[1].0 as i64 - costs[0].0 as i64;
        extra_blocks += blocks;
        worst = worst.max(blocks);
        extra_flushes += costs[1].1 as i64 - costs[0].1 as i64;
    }
    eprintln!(
        "kaifs snapshot cost: {} extra flushes, {:.2} extra blocks a commit on average, {worst} at most \
         (bar: 0 flushes, <= 1 block)",
        extra_flushes / rounds as i64,
        extra_blocks as f64 / rounds as f64
    );
    assert_eq!(extra_flushes, 0);
    assert!(worst <= 1);
}
