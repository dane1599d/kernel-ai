//! The lines `tlsbench` (ring 3) and `tlshost` (the host) print: each figure's
//! median over a few rounds against P5-S1's bars (handshake at most 300 ms
//! under KVM, AES-GCM at least 200 MB/s). Figures are reported, not asserted:
//! KVM runners stall (docs/decisions.md, "KVM CI stalls"). The run passes when
//! every handshake and transfer gives the right answer.

use crate::bench::{self, Clock, View, mb_per_s};
use crate::fixture::{self, Chain};
use crate::{TLS13_AES_128_GCM_SHA256, TLS13_AES_256_GCM_SHA384, TLS13_CHACHA20_POLY1305_SHA256};
use alloc::vec::Vec;
use core::fmt::Arguments;
use rustls::SupportedCipherSuite;

const ROUNDS: usize = 9;
/// No round starts after this long, so a TCG boot ends within its case timeout.
const BUDGET_NS: u64 = 150_000_000_000;
const TRANSFER_BYTES: usize = 8 << 20;

fn median(mut samples: Vec<u64>) -> u64 {
    samples.sort_unstable();
    samples.get(samples.len() / 2).copied().unwrap_or(0)
}

/// Runs `f` up to `rounds` times within the budget; the samples, or None if any
/// run failed.
fn rounds<T>(
    clock: Clock,
    started: u64,
    rounds: usize,
    mut f: impl FnMut() -> Option<T>,
) -> Option<Vec<T>> {
    let mut out = Vec::new();
    for r in 0..rounds {
        if r > 0 && clock() - started > BUDGET_NS {
            break;
        }
        out.push(f()?);
    }
    Some(out)
}

/// CPUID: AES-NI, PCLMULQDQ and AVX2, which the AEADs pick at run time.
fn cpu() -> (bool, bool, bool) {
    let leaf1 = core::arch::x86_64::__cpuid(1);
    let leaf7 = core::arch::x86_64::__cpuid_count(7, 0);
    (
        leaf1.ecx & (1 << 25) != 0,
        leaf1.ecx & (1 << 1) != 0,
        leaf7.ebx & (1 << 5) != 0,
    )
}

fn handshakes(
    tag: &str,
    chain: &Chain,
    clock: Clock,
    started: u64,
    out: &mut dyn FnMut(Arguments),
) -> bool {
    let samples = rounds(clock, started, ROUNDS, || {
        bench::handshake_as(chain, View::of(chain), clock).ok()
    });
    let Some(samples) = samples else {
        out(format_args!(
            "{tag}: handshake with the {} FAILED",
            chain.name
        ));
        return false;
    };
    let n = samples.len();
    let client = median(samples.iter().map(|h| h.client_ns).collect());
    let server = median(samples.iter().map(|h| h.server_ns).collect());
    out(format_args!(
        "{tag}: handshake, {}: client p50 {} us, test server p50 {} us, {n} rounds (bar 300 ms under KVM)",
        chain.name,
        client / 1000,
        server / 1000,
    ));
    true
}

fn transfers(
    tag: &str,
    name: &str,
    suite: SupportedCipherSuite,
    clock: Clock,
    started: u64,
    out: &mut dyn FnMut(Arguments),
) -> bool {
    let samples = rounds(clock, started, 3, || {
        bench::transfer(suite, TRANSFER_BYTES, clock).ok()
    });
    let Some(samples) = samples else {
        out(format_args!("{tag}: {name} records FAILED"));
        return false;
    };
    let open = median(samples.iter().map(|t| t.open_ns).collect());
    let seal = median(samples.iter().map(|t| t.seal_ns).collect());
    out(format_args!(
        "{tag}: {name} through rustls: client opens {} MB/s, server seals {} MB/s ({} MiB, p50 of {}; bar 200 MB/s for AES-GCM)",
        mb_per_s(TRANSFER_BYTES, open),
        mb_per_s(TRANSFER_BYTES, seal),
        TRANSFER_BYTES >> 20,
        samples.len(),
    ));
    true
}

/// Prints every figure; true when every handshake and transfer was right.
pub fn run(tag: &str, clock: Clock, out: &mut dyn FnMut(Arguments)) -> bool {
    let started = clock();
    let (aes, clmul, avx2) = cpu();
    out(format_args!(
        "{tag}: CPU AES-NI {aes}, PCLMULQDQ {clmul}, AVX2 {avx2}; host {}, now {}",
        fixture::HOST,
        fixture::NOW
    ));
    let mut right = handshakes(tag, &fixture::ECDSA, clock, started, out);
    right &= handshakes(tag, &fixture::RSA, clock, started, out);
    let suites: [(&str, SupportedCipherSuite); 3] = [
        (
            "AES-128-GCM",
            SupportedCipherSuite::Tls13(&TLS13_AES_128_GCM_SHA256),
        ),
        (
            "AES-256-GCM",
            SupportedCipherSuite::Tls13(&TLS13_AES_256_GCM_SHA384),
        ),
        (
            "ChaCha20-Poly1305",
            SupportedCipherSuite::Tls13(&TLS13_CHACHA20_POLY1305_SHA256),
        ),
    ];
    for (name, suite) in suites {
        right &= transfers(tag, name, suite, clock, started, out);
    }
    let aes = median(
        (0..3)
            .map(|_| bench::raw_seal::<aes_gcm::Aes128Gcm>(TRANSFER_BYTES, clock))
            .collect(),
    );
    let chacha = median(
        (0..3)
            .map(|_| bench::raw_seal::<chacha20poly1305::ChaCha20Poly1305>(TRANSFER_BYTES, clock))
            .collect(),
    );
    out(format_args!(
        "{tag}: bare AEAD, 16 KiB records: AES-128-GCM {} MB/s, ChaCha20-Poly1305 {} MB/s",
        mb_per_s(TRANSFER_BYTES, aes),
        mb_per_s(TRANSFER_BYTES, chacha),
    ));
    out(format_args!(
        "{tag}: {} in {} ms",
        if right {
            "every result right"
        } else {
            "a result was WRONG"
        },
        (clock() - started) / 1_000_000
    ));
    right
}
