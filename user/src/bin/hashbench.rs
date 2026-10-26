//! Spike S04-1 (ADR 0004), boot case `test=hashbench`: SHA-256 and seahash-64 over
//! 128 KiB records inside the `fs` build target, the soft-float ring-3 target
//! `fs` ships on, with sha2 choosing its backend as it would there. If SHA-256
//! here is under a third of the host's portable figure, `csum_kind` becomes
//! seahash-64. Built only on the never-merged branch spike/kaifs.

#![no_std]
#![no_main]

use core::hint::black_box;
use spike_kaifs::CsumKind;
use user::{println, sys};

user::entry!(main);

const RECORD: usize = 128 << 10;
const ROUNDS: u64 = 100;

/// Whether CPUID offers the SHA extensions (leaf 7, EBX bit 29), which sha2
/// uses at run time when present.
fn sha_ni() -> bool {
    #[allow(unused_unsafe)]
    // SAFETY: CPUID exists on every x86_64 CPU and only reads.
    let leaf7 = unsafe { core::arch::x86_64::__cpuid_count(7, 0) };
    leaf7.ebx & (1 << 29) != 0
}

fn main() -> i32 {
    let mut data = alloc::vec![0u8; RECORD];
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    for byte in data.iter_mut() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *byte = x as u8;
    }
    println!(
        "hashbench: CPUID SHA extensions {}",
        if sha_ni() { "present" } else { "absent" }
    );
    for kind in [CsumKind::Sha256, CsumKind::Seahash] {
        let mut best = u64::MAX;
        for _ in 0..3 {
            let start = sys::time_ns();
            for _ in 0..ROUNDS {
                black_box(kind.sum(black_box(&data)));
            }
            best = best.min(sys::time_ns() - start);
        }
        let bytes = u128::from(ROUNDS) * RECORD as u128;
        let mbs = bytes * 1_000_000_000 / u128::from(best.max(1)) / (1 << 20);
        println!(
            "hashbench: {kind:?} {mbs} MB/s over 128 KiB records in the fs build target ({} records in {best} ns)",
            ROUNDS
        );
    }
    println!("hashbench: done");
    0
}

extern crate alloc;
