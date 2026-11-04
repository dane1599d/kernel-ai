//! Spike P5-S1's ring-3 figures (plan p5-networking §5), in the hard-float
//! program class the TLS clients build for (§4, item 3): `kainet`'s TLS 1.3
//! on our RustCrypto provider, with no std and no C. A client and a server of
//! ours handshake in memory, each AEAD seals 16 MiB, and 16 MiB goes through
//! rustls's record layer for each suite. It prints the figures against the
//! spike's bars (a handshake within 300 ms, AES-GCM at least 200 MB/s); they
//! are reported, not asserted. There is no TCP for ring 3 yet (P5 task 2), so
//! nothing here reaches a network.
//!
//! Random numbers come from RDRAND until the kernel's RANDOM call exists
//! (task 3); the bench refuses to run on a CPU without it.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::sync::Arc;
use kainet::{bench, provider};
use rustls::crypto::{GetRandomFailed, SecureRandom};
use user::{println, sys};

user::entry!(main);

const HANDSHAKES: u32 = 10;
const BYTES: usize = 16 << 20;
/// The spike's bars.
const HANDSHAKE_BAR_MS: u64 = 300;
const AES_BAR_MBPS: u64 = 200;
/// Tries at RDRAND before a fill fails, as Intel's guide advises.
const RDRAND_TRIES: usize = 10;

#[derive(Debug)]
struct Rdrand;

fn rdrand64() -> Option<u64> {
    for _ in 0..RDRAND_TRIES {
        let mut value = 0u64;
        // SAFETY: main checked CPUID's RDRAND bit before any key was made.
        if unsafe { core::arch::x86_64::_rdrand64_step(&mut value) } == 1 {
            return Some(value);
        }
    }
    None
}

impl SecureRandom for Rdrand {
    fn fill(&self, buf: &mut [u8]) -> Result<(), GetRandomFailed> {
        for chunk in buf.chunks_mut(8) {
            let value = rdrand64().ok_or(GetRandomFailed)?;
            chunk.copy_from_slice(&value.to_le_bytes()[..chunk.len()]);
        }
        Ok(())
    }
}

static RANDOM: Rdrand = Rdrand;

/// CPUID leaf 1, ECX bit 30.
fn has_rdrand() -> bool {
    let leaf = core::arch::x86_64::__cpuid(1);
    leaf.ecx & (1 << 30) != 0
}

fn main() -> i32 {
    if !has_rdrand() {
        println!("tlsbench: no RDRAND on this CPU, so no random numbers; nothing run");
        return 1;
    }
    let clock = || sys::time_ns();
    let provider = Arc::new(provider::provider(&RANDOM));
    match bench::handshake_ns(&provider, HANDSHAKES, &clock) {
        Ok(ns) => {
            let ms = ns / 1_000_000;
            let mark = if ms <= HANDSHAKE_BAR_MS {
                "within"
            } else {
                "over"
            };
            println!(
                "tlsbench: fastest of {HANDSHAKES} full handshakes (client and server): {} us, {mark} the {HANDSHAKE_BAR_MS} ms bar",
                ns / 1000
            );
        }
        Err(error) => {
            println!("tlsbench: the handshake failed: {error:?}");
            return 1;
        }
    }
    for (name, mbps) in bench::aead_mbps(BYTES, &clock) {
        println!("tlsbench: {name} seals {mbps} MB/s");
    }
    let aes = bench::aead_mbps(BYTES, &clock)[0].1;
    let mark = if aes >= AES_BAR_MBPS {
        "at or above"
    } else {
        "below"
    };
    println!("tlsbench: AES-128-GCM {mark} the {AES_BAR_MBPS} MB/s bar");
    for suite in provider.cipher_suites.clone() {
        match bench::record_mbps(&provider, suite, BYTES, &clock) {
            Ok(mbps) => println!("tlsbench: {:?} through rustls: {mbps} MB/s", suite.suite()),
            Err(error) => {
                println!("tlsbench: {:?} failed: {error:?}", suite.suite());
                return 1;
            }
        }
    }
    println!("tlsbench: done");
    0
}
