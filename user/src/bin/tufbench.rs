//! Spikes S08-3's and S11-1's ring-3 figures (ADR 0008 §7, ADR 0011 §3), in a
//! soft-float program as `updated` and `pkg` will be: one ECDSA P-256 and one
//! Ed25519 verification, the trusted set over a 1,000-target `packages` role
//! (root, timestamp, snapshot, targets, then packages, each P-256), and SHA-256
//! over 16 MiB. Five rounds of each; it prints each figure's median against the
//! spikes' bars (P-256 at most 50 ms and the 1,000 targets at most 100 ms under
//! KVM, SHA-256 at least 100 MB/s; under TCG, P-256 under 1 s). The figures are
//! reported, not asserted, as KVM runners stall (docs/decisions.md, "KVM CI
//! stalls"). It exits 0 when every verification gives the right answer. The
//! fixture is signed at build time (build.rs). Boot case `test=tuf-bench`.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use kaituf::keys::Key;
use kaituf::meta::Root;
use kaituf::trusted::{ROOT, TrustedSet};
use sha2::{Digest, Sha256};
use user::{println, sys};

user::entry!(main);

macro_rules! fixture {
    ($name:literal) => {
        include_bytes!(concat!(env!("OUT_DIR"), "/tufbench/", $name))
    };
}

static ROOT_JSON: &[u8] = fixture!("1.root.json");
static TIMESTAMP: &[u8] = fixture!("timestamp.json");
static SNAPSHOT: &[u8] = fixture!("1.snapshot.json");
static TARGETS: &[u8] = fixture!("1.targets.json");
static PACKAGES: &[u8] = fixture!("1.packages.json");
static DEVICE: &[u8] = fixture!("ed25519.json");

/// Before the fixture's expiry; the bench needs no wall clock.
const NOW: u64 = 1_800_000_000;
const ROUNDS: usize = 5;
/// No round starts after this long, so a TCG boot ends within its case timeout.
const BUDGET_NS: u64 = 120_000_000_000;
const SHA_BYTES: usize = 16 << 20;

fn median(mut samples: Vec<u64>) -> u64 {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

/// Runs `f` up to `ROUNDS` times within the budget; the median time and
/// whether every run gave true.
fn rounds(started: u64, mut f: impl FnMut() -> bool) -> (u64, bool) {
    let mut times = Vec::new();
    let mut right = true;
    for r in 0..ROUNDS {
        if r > 0 && sys::time_ns() - started > BUDGET_NS {
            break;
        }
        let start = sys::time_ns();
        right &= f();
        times.push(sys::time_ns() - start);
    }
    (median(times), right)
}

/// The whole chain to the `packages` role, as `pkg` checks it.
fn chain() -> bool {
    let run = || -> Result<usize, kaituf::Error> {
        let mut set = TrustedSet::new(ROOT_JSON, NOW)?;
        set.update_timestamp(TIMESTAMP)?;
        set.update_snapshot(SNAPSHOT, false)?;
        set.update_targets(TARGETS, "targets", ROOT)?;
        set.update_targets(PACKAGES, "packages", "targets")?;
        Ok(set
            .targets("packages")
            .map_or(0, |m| m.signed.targets.len()))
    };
    matches!(run(), Ok(1000))
}

fn ms(ns: u64) -> u64 {
    ns / 1_000_000
}

fn main() -> i32 {
    let started = sys::time_ns();
    let Ok(root) = Root::parse(ROOT_JSON) else {
        println!("tuf-bench: the fixture's root does not parse");
        return 1;
    };
    let (id, sig) = &root.sigs[0];
    let key = &root.signed.keys[id];
    let (p256, p256_right) = rounds(started, || key.verifies(&root.canonical, sig));
    let device: serde_json::Value = serde_json::from_slice(DEVICE).unwrap_or_default();
    let ed25519 = Key::parse(&serde_json::json!({
        "keytype": "ed25519", "scheme": "ed25519", "keyval": { "public": device["public"] },
    }));
    let device_sig = device["sig"].as_str().unwrap_or("");
    let (ed, ed_right) = match &ed25519 {
        Ok(key) => rounds(started, || key.verifies(&root.canonical, device_sig)),
        Err(_) => (0, false),
    };
    let tampered = !key.verifies(b"another message", sig);
    let (packages, chain_right) = rounds(started, chain);
    let data = vec![0x5a_u8; SHA_BYTES];
    let (sha, _) = rounds(started, || {
        core::hint::black_box(Sha256::digest(core::hint::black_box(&data)));
        true
    });
    let mb_per_s = (SHA_BYTES as u64 * 1000) / sha.max(1);
    println!(
        "tuf-bench: P-256 verify p50 {} us (bar 50 ms under KVM, 1 s under TCG)",
        p256 / 1000
    );
    println!("tuf-bench: Ed25519 verify p50 {} us", ed / 1000);
    println!(
        "tuf-bench: root, timestamp, snapshot, targets and a 1,000-target packages role verified in {} ms p50 ({} KiB of metadata; bar 100 ms under KVM)",
        ms(packages),
        (ROOT_JSON.len() + TIMESTAMP.len() + SNAPSHOT.len() + TARGETS.len() + PACKAGES.len())
            / 1024
    );
    println!(
        "tuf-bench: SHA-256 over 16 MiB in {} ms p50, {} MB/s (bar 100 MB/s under KVM)",
        ms(sha),
        mb_per_s
    );
    let right = p256_right && ed_right && tampered && chain_right;
    if !right {
        println!(
            "tuf-bench: a wrong answer: P-256 {p256_right}, Ed25519 {ed_right}, a tampered message refused {tampered}, the chain {chain_right}"
        );
        return 1;
    }
    println!("tuf-bench: done");
    0
}
