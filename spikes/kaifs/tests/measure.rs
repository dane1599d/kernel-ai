//! S04-1's figures (ADR 0004): bytes written per byte changed, the cost of a
//! snapshot, the GC's mark, and SHA-256 against seahash-64. Release builds:
//! cargo test --release -p spike-kaifs --test measure -- --nocapture --test-threads 1
//! For the portable SHA-256 figure the ADR's bar names, add
//! RUSTFLAGS='--cfg sha2_backend="soft"'.

use spike_kaifs::check::{self, Cache};
use spike_kaifs::crash::Rng;
use spike_kaifs::{CsumKind, Fs, RecordingDisk};
use std::hint::black_box;
use std::time::Instant;

const KEYS: u64 = 100_000;

fn record(rng: &mut Rng, len: usize) -> Vec<u8> {
    (0..len).map(|_| rng.next_u64() as u8).collect()
}

/// A filesystem with `KEYS` keys, each a data record of `len` bytes, committed.
fn with_records(blocks: u64, keys: u64, len: usize, kind: CsumKind) -> Fs<RecordingDisk> {
    let mut fs = Fs::format(RecordingDisk::new(blocks), kind, [3; 16]).unwrap();
    let mut rng = Rng(11);
    for key in 0..keys {
        fs.put_record(key, &record(&mut rng, len)).unwrap();
        if key % 4096 == 4095 {
            fs.commit().unwrap();
            fs.io.dev.checkpoint();
        }
    }
    fs.commit().unwrap();
    fs.io.dev.checkpoint();
    fs
}

/// Blocks written and flushes for `rounds` commits of `per_commit` 4 KiB
/// overwrites at random keys of records `record_len` long.
fn overwrite_rounds(
    fs: &mut Fs<RecordingDisk>,
    keys: u64,
    record_len: usize,
    per_commit: u64,
    rounds: u64,
    snapshot: bool,
) -> (u64, u64) {
    let mut rng = Rng(99);
    let (writes, flushes) = (fs.io.dev.writes, fs.io.dev.flushes);
    for _ in 0..rounds {
        for _ in 0..per_commit {
            let key = rng.below(keys);
            // A 4 KiB change: inside a larger record the whole record is rewritten.
            fs.put_record(key, &record(&mut rng, record_len)).unwrap();
        }
        if snapshot {
            fs.snapshot().unwrap();
        }
        fs.commit().unwrap();
        fs.io.dev.checkpoint();
    }
    (fs.io.dev.writes - writes, fs.io.dev.flushes - flushes)
}

#[test]
fn write_amplification_of_4k_random_overwrites() {
    let rounds = 50;
    let mut fs = with_records(1 << 18, KEYS, 4096, CsumKind::Sha256);
    let (writes, flushes) = overwrite_rounds(&mut fs, KEYS, 4096, 64, rounds, false);
    let changed = rounds * 64;
    eprintln!(
        "kaifs write-amp: 4 KiB overwrites, 64 a commit, {KEYS} keys of 4 KiB records: {:.2}x ({} blocks for {} blocks changed; {:.1} blocks and {} flushes a commit) (bar <= 3.0x)",
        writes as f64 / changed as f64,
        writes,
        changed,
        writes as f64 / rounds as f64,
        flushes / rounds
    );
    assert!((writes as f64 / changed as f64) <= 3.0);
    check::check(&mut fs, &mut Cache::default()).unwrap();
}

#[test]
fn write_amplification_inside_128k_records() {
    // ADR 0004's consequence: a 4 KiB change rewrites its whole 128 KiB record.
    let (keys, rounds) = (1_000, 20);
    let mut fs = with_records(1 << 17, keys, 128 << 10, CsumKind::Sha256);
    let (writes, _) = overwrite_rounds(&mut fs, keys, 128 << 10, 64, rounds, false);
    eprintln!(
        "kaifs write-amp: 4 KiB changes inside 128 KiB records, 64 a commit, {keys} keys: {:.1}x (reported only)",
        writes as f64 / (rounds * 64) as f64
    );
}

#[test]
fn a_snapshot_costs_no_flush_and_at_most_one_block() {
    // Each round mounts two copies of the same disk, makes the same 64 overwrites
    // on both, and commits; only the second takes a snapshot first.
    let base = with_records(1 << 18, KEYS, 4096, CsumKind::Sha256);
    let disk = base.into_dev();
    let (mut extra_blocks, mut extra_flushes, mut worst) = (0i64, 0i64, 0i64);
    let rounds = 40;
    for round in 0..rounds {
        let mut costs = [(0u64, 0u64); 2];
        for (i, snapshot) in [false, true].into_iter().enumerate() {
            let mut fs = Fs::mount(disk.clone()).unwrap();
            let mut rng = Rng(1000 + round);
            let (w, f) = (fs.io.dev.writes, fs.io.dev.flushes);
            for _ in 0..64 {
                let key = rng.below(KEYS);
                fs.put_record(key, &record(&mut rng, 4096)).unwrap();
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
        "kaifs snapshot cost: {} extra flushes, {:.2} extra blocks a commit on average, {worst} at most (bar: 0 flushes, <= 1 block)",
        extra_flushes / rounds as i64,
        extra_blocks as f64 / rounds as f64
    );
    assert_eq!(extra_flushes, 0);
    assert!(worst <= 1);
}

fn gc_mark(kind: CsumKind) {
    let mut fs = Fs::format(RecordingDisk::new(1 << 17), kind, [5; 16]).unwrap();
    let mut rng = Rng(3);
    for key in 0..KEYS {
        fs.put(key, &record(&mut rng, 32)).unwrap();
        if key % 4096 == 4095 {
            fs.commit().unwrap();
            fs.io.dev.checkpoint();
        }
    }
    fs.commit().unwrap();
    for _ in 0..100 {
        for _ in 0..64 {
            let key = rng.below(KEYS);
            match rng.below(3) {
                0 => fs.put(key, &record(&mut rng, 32)).unwrap(),
                1 => fs
                    .put(KEYS + rng.below(KEYS), &record(&mut rng, 32))
                    .unwrap(),
                _ => {
                    fs.delete(key).unwrap();
                }
            }
        }
        fs.commit().unwrap();
        fs.snapshot().unwrap();
        fs.commit().unwrap();
        fs.io.dev.checkpoint();
    }
    // Drop half the snapshots, so the sweep has something to free.
    for id in (1..=100).step_by(2) {
        fs.drop_snapshot(id).unwrap();
    }
    fs.commit().unwrap();
    let start = Instant::now();
    let freed = check::gc(&mut fs).unwrap();
    let elapsed = start.elapsed();
    fs.commit().unwrap();
    check::check(&mut fs, &mut Cache::default()).unwrap();
    eprintln!(
        "kaifs gc: {KEYS} keys, 100 snapshots 64 random operations apart (50 then dropped), {kind:?}: mark and sweep {:.3} s, {freed} blocks freed (bar < 2 s)",
        elapsed.as_secs_f64()
    );
    assert!(elapsed.as_secs_f64() < 2.0);
}

#[test]
fn gc_mark_with_sha256() {
    gc_mark(CsumKind::Sha256);
}

#[test]
fn gc_mark_with_seahash() {
    gc_mark(CsumKind::Seahash);
}

/// SHA-256 against seahash-64 over 128 KiB records.
#[test]
fn checksum_speed() {
    let mut rng = Rng(1);
    let data = record(&mut rng, 128 << 10);
    for kind in [CsumKind::Sha256, CsumKind::Seahash] {
        let mut best = f64::MAX;
        for _ in 0..5 {
            let start = Instant::now();
            for _ in 0..200 {
                black_box(kind.sum(black_box(&data)));
            }
            best = best.min(start.elapsed().as_secs_f64());
        }
        let mbs = 200.0 * 128.0 / 1024.0 / best;
        eprintln!(
            "kaifs csum: {kind:?} {mbs:.0} MB/s over 128 KiB records (backend: {})",
            if cfg!(sha2_backend = "soft") {
                "portable, forced"
            } else {
                "sha2's run-time choice"
            }
        );
    }
}
