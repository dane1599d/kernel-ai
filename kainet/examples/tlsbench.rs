//! P5-S1's bench on the host, the same code `tlsbench` runs in ring 3:
//! `cargo run -p kainet --release --example tlsbench`.

use kainet::{bench, provider};
use rustls::crypto::{GetRandomFailed, SecureRandom};
use std::sync::Arc;
use std::time::Instant;

#[derive(Debug)]
struct HostRandom;

impl SecureRandom for HostRandom {
    fn fill(&self, buf: &mut [u8]) -> Result<(), GetRandomFailed> {
        getrandom::fill(buf).map_err(|_| GetRandomFailed)
    }
}

static RANDOM: HostRandom = HostRandom;

/// Bytes through each AEAD and each suite.
const BYTES: usize = 64 << 20;
const HANDSHAKES: u32 = 50;

fn main() {
    let start = Instant::now();
    let clock = move || start.elapsed().as_nanos() as u64;
    let provider = Arc::new(provider::provider(&RANDOM));
    match bench::handshake_ns(&provider, HANDSHAKES, &clock) {
        Ok(ns) => println!(
            "tlsbench: fastest of {HANDSHAKES} full handshakes (client and server): {} us",
            ns / 1000
        ),
        Err(error) => println!("tlsbench: handshake failed: {error:?}"),
    }
    for (name, mbps) in bench::aead_mbps(BYTES, &clock) {
        println!("tlsbench: {name} seals {mbps} MB/s");
    }
    for suite in provider.cipher_suites.clone() {
        match bench::record_mbps(&provider, suite, BYTES, &clock) {
            Ok(mbps) => println!("tlsbench: {:?} through rustls: {mbps} MB/s", suite.suite()),
            Err(error) => println!("tlsbench: {:?} failed: {error:?}", suite.suite()),
        }
    }
}
