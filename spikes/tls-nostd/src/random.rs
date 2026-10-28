//! Entropy for key shares and the ClientHello's random. With `std`, the host's
//! (getrandom). Without, the CPU's RDRAND, which ring 3 may execute: a spike
//! stand-in for P5 task 3's RANDOM call, which mixes RDSEED, RDRAND and
//! virtio-rng into a kernel ChaCha20 generator.

use rustls::crypto::{GetRandomFailed, SecureRandom};

#[derive(Debug)]
pub struct Random;

impl SecureRandom for Random {
    fn fill(&self, buf: &mut [u8]) -> Result<(), GetRandomFailed> {
        fill(buf)
    }
}

#[cfg(feature = "std")]
pub fn fill(buf: &mut [u8]) -> Result<(), GetRandomFailed> {
    getrandom::getrandom(buf).map_err(|_| GetRandomFailed)
}

/// RDRAND may run dry briefly; Intel's guidance is ten tries.
#[cfg(not(feature = "std"))]
const RDRAND_TRIES: usize = 10;

#[cfg(not(feature = "std"))]
pub fn fill(buf: &mut [u8]) -> Result<(), GetRandomFailed> {
    if !has_rdrand() {
        return Err(GetRandomFailed);
    }
    for chunk in buf.chunks_mut(8) {
        let word = (0..RDRAND_TRIES)
            .find_map(|_| {
                let mut value = 0u64;
                // SAFETY: CPUID said RDRAND exists (has_rdrand above).
                (unsafe { rdrand(&mut value) } == 1).then_some(value)
            })
            .ok_or(GetRandomFailed)?;
        chunk.copy_from_slice(&word.to_le_bytes()[..chunk.len()]);
    }
    Ok(())
}

#[cfg(not(feature = "std"))]
#[target_feature(enable = "rdrand")]
unsafe fn rdrand(value: &mut u64) -> i32 {
    core::arch::x86_64::_rdrand64_step(value)
}

#[cfg(not(feature = "std"))]
fn has_rdrand() -> bool {
    // CPUID leaf 1, ECX bit 30.
    let leaf = core::arch::x86_64::__cpuid(1);
    leaf.ecx & (1 << 30) != 0
}
