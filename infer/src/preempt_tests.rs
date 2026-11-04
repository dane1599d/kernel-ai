//! S13-1 `kv-preempt`, the host half (ADR 0013 section 2): a KV slot of Qwen3-4B's
//! shape (36 layers of 1,024 keys and 1,024 values, 144 KiB a position) is
//! snapshotted into RAM and spilled to a file, and comes back bit for bit. The
//! cache is filled with seeded bits, not run through the model: the model's values
//! are checked by the engine's own tests (`snapshot_tests`, `segments_tests`).
//!
//! The bars (S13-1): a RAM snapshot and restore of 1,024 positions in at most 50 ms,
//! and a spill round trip in at most 1.5 s. The benchmark is ignored; run it as
//!
//! ```text
//! cargo test -p infer --release preempt -- --ignored --nocapture
//! ```
//!
//! and it prints a table, and fails in a release build when a bar is missed. The
//! guest run under KVM with the real model comes later, on a runner.

extern crate std;

use crate::segments::{self, Key, Refused, Snapshot};
use crate::{Config, KvCache};
use alloc::vec::Vec;
use quant::Tier;
use std::io::{BufReader, BufWriter, Read, Write};
use std::time::{Duration, Instant};

/// Qwen3-4B's shape, as llm loads it.
fn qwen3_4b() -> Config {
    Config {
        n_layer: 36,
        n_embd: 2560,
        n_ff: 9728,
        n_head: 32,
        n_head_kv: 8,
        head_dim: 128,
        vocab: 151_936,
        context_length: 40_960,
        rope_theta: 1_000_000.0,
        rms_eps: 1e-6,
    }
}

/// llm's context cap (`kabi::model::CONTEXT_CAP`): positions a slot holds.
const CAP: usize = 2_048;

fn key() -> Key {
    Key {
        model_sha256: [9; 32],
        tier: Tier::best(),
        pack_id: segments::spill_pack_id(7, 3),
        slot: 1,
    }
}

/// splitmix64.
struct Bits(u64);

impl Bits {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// A cache of `positions` computed positions of seeded bits, and the tokens behind
/// them.
fn filled(positions: usize) -> (KvCache, Vec<u32>) {
    let mut kv = KvCache::new(&qwen3_4b(), CAP).unwrap();
    let mut bits = Bits(positions as u64);
    for layer in 0..36 {
        for pos in 0..positions {
            let (keys, values) = kv.slot_mut(layer, pos);
            for half in keys.iter_mut().chain(values.iter_mut()) {
                *half = bits.next() as u16;
            }
        }
    }
    kv.set_len(positions);
    let tokens = (0..positions as u32).map(|t| t * 3 + 1).collect();
    (kv, tokens)
}

/// Whether the first `positions` of `a` and `b` hold the same bits, every layer.
fn same(a: &KvCache, b: &KvCache, positions: usize) -> bool {
    a.len() == positions
        && b.len() == positions
        && (0..36).all(|layer| a.positions(layer, positions) == b.positions(layer, positions))
}

/// A prompt the tokens start, and a token more: what a resume asks for.
fn prompt(tokens: &[u32]) -> Vec<u32> {
    let mut p = tokens.to_vec();
    p.push(99);
    p
}

/// The median of `runs` timings of `f`.
fn median(runs: usize, mut f: impl FnMut()) -> Duration {
    let mut times: Vec<Duration> = (0..runs)
        .map(|_| {
            let start = Instant::now();
            f();
            start.elapsed()
        })
        .collect();
    times.sort();
    times[runs / 2]
}

#[test]
fn a_snapshot_of_a_production_slot_comes_back_bit_for_bit() {
    // Not the full cap: enough for 36 layers of 1,024 and a restore that is not at
    // position 0 of every layer's stride.
    let (kv, tokens) = filled(48);
    let mut snapshot = Snapshot::new();
    segments::snapshot(&key(), &kv, &tokens, &mut snapshot).unwrap();
    assert_eq!(snapshot.bytes(), 48 * (144 << 10) + 4 * 48);
    let mut back = KvCache::new(&qwen3_4b(), CAP).unwrap();
    let restored = segments::restore_snapshot(&key(), &mut back, &prompt(&tokens), &snapshot);
    assert_eq!(restored, Ok(tokens.clone()));
    assert!(same(&kv, &back, 48));
    // A spill file, written in pieces and read in other pieces, is the same too.
    let mut file = Vec::new();
    segments::save(&key(), &kv, &tokens, &mut |b| {
        file.extend_from_slice(b);
        true
    })
    .unwrap();
    assert_eq!(file.len(), segments::file_bytes(&kv, 48));
    let mut again = KvCache::new(&qwen3_4b(), CAP).unwrap();
    let mut at = 0;
    let restored = segments::restore(&key(), &mut again, &prompt(&tokens), &mut |buf| {
        let Some(part) = file.get(at..at + buf.len()) else {
            return false;
        };
        buf.copy_from_slice(part);
        at += buf.len();
        true
    });
    assert_eq!(restored, Ok(tokens));
    assert!(same(&kv, &again, 48));
    // A flipped bit in a spill file is refused, and the slot is left empty.
    let mut bad = file.clone();
    *bad.last_mut().unwrap() ^= 0x80;
    let mut at = 0;
    let refused = segments::restore(&key(), &mut again, &prompt(&[1]), &mut |buf| {
        let Some(part) = bad.get(at..at + buf.len()) else {
            return false;
        };
        buf.copy_from_slice(part);
        at += buf.len();
        true
    });
    assert!(matches!(refused, Err(Refused::Tokens | Refused::Values)));
    assert!(again.is_empty());
}

/// What one size costs, in ms.
struct Row {
    positions: usize,
    mib: usize,
    cold_save: f64,
    save: f64,
    restore: f64,
    spill_save: f64,
    spill_restore: f64,
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn measure(positions: usize, dir: &std::path::Path) -> Row {
    let (kv, tokens) = filled(positions);
    let prompt = prompt(&tokens);
    let mib = (positions * (144 << 10)) >> 20;
    // RAM: the first save pays the memory; a pooled snapshot does not.
    let mut snapshot = Snapshot::new();
    let start = Instant::now();
    segments::snapshot(&key(), &kv, &tokens, &mut snapshot).unwrap();
    let cold_save = ms(start.elapsed());
    let save = ms(median(7, || {
        segments::snapshot(&key(), &kv, &tokens, &mut snapshot).unwrap();
    }));
    let mut back = KvCache::new(&qwen3_4b(), CAP).unwrap();
    let restore = ms(median(7, || {
        segments::restore_snapshot(&key(), &mut back, &prompt, &snapshot).unwrap();
    }));
    assert!(
        same(&kv, &back, positions),
        "{positions}: the snapshot differs"
    );
    // Disk: a spill file written whole and made durable, and read back.
    let path = dir.join(std::format!("kv-{positions}.kv"));
    let spill_save = ms(median(3, || {
        let mut out = BufWriter::with_capacity(1 << 20, std::fs::File::create(&path).unwrap());
        segments::save(&key(), &kv, &tokens, &mut |b| out.write_all(b).is_ok()).unwrap();
        out.into_inner().unwrap().sync_all().unwrap();
    }));
    let mut from_disk = KvCache::new(&qwen3_4b(), CAP).unwrap();
    let spill_restore = ms(median(3, || {
        let mut input = BufReader::with_capacity(1 << 20, std::fs::File::open(&path).unwrap());
        segments::restore(&key(), &mut from_disk, &prompt, &mut |buf| {
            input.read_exact(buf).is_ok()
        })
        .unwrap();
    }));
    assert!(
        same(&kv, &from_disk, positions),
        "{positions}: the file differs"
    );
    std::fs::remove_file(&path).unwrap();
    Row {
        positions,
        mib,
        cold_save,
        save,
        restore,
        spill_save,
        spill_restore,
    }
}

#[test]
#[ignore = "a benchmark: cargo test -p infer --release preempt -- --ignored --nocapture"]
fn s13_1_preempt_host_bars() {
    let dir = std::env::temp_dir().join(std::format!("kai-preempt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let rows: Vec<Row> = [512, 1_024, 2_048]
        .iter()
        .map(|n| measure(*n, &dir))
        .collect();
    std::fs::remove_dir_all(&dir).unwrap();
    std::println!("S13-1 host: Qwen3-4B KV shape, f16, 144 KiB a position (ms, median)");
    std::println!(
        "| positions | MiB | RAM save (cold) | RAM save | RAM restore | RAM both | spill save | spill restore | spill both |"
    );
    std::println!("|---|---|---|---|---|---|---|---|---|");
    for r in &rows {
        std::println!(
            "| {} | {} | {:.1} | {:.1} | {:.1} | {:.1} | {:.0} | {:.0} | {:.0} |",
            r.positions,
            r.mib,
            r.cold_save,
            r.save,
            r.restore,
            r.save + r.restore,
            r.spill_save,
            r.spill_restore,
            r.spill_save + r.spill_restore,
        );
    }
    let at_1024 = rows.iter().find(|r| r.positions == 1_024).unwrap();
    std::println!(
        "bars at 1,024 positions: RAM both {:.1} ms (at most 50), spill both {:.0} ms (at most 1,500)",
        at_1024.save + at_1024.restore,
        at_1024.spill_save + at_1024.spill_restore,
    );
    // A debug build is far slower than the guest's release build: only a release
    // build is held to the bars.
    if !cfg!(debug_assertions) {
        assert!(at_1024.save + at_1024.restore <= 50.0, "the RAM bar");
        assert!(
            at_1024.spill_save + at_1024.spill_restore <= 1_500.0,
            "the spill bar"
        );
    }
}
