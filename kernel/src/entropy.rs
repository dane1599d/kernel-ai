//! The kernel's random numbers (ENTROPY): a [`csprng::Csprng`] seeded at boot
//! from the CPU's RDSEED, or RDRAND where there is no RDSEED, and reseeded from
//! it every [`csprng::RESEED_MS`] or [`csprng::RESEED_BYTES`]. A CPU with
//! neither leaves it unseeded, and ENTROPY answers NoEntropy rather than weak
//! bytes, so a key is never made from them.

use crate::log;
use core::arch::x86_64::{__cpuid, __cpuid_count, _rdrand64_step, _rdseed64_step};
use csprng::{Csprng, SEED_BYTES};
use spin::Mutex;
use x86_64::instructions::interrupts::without_interrupts;

/// None until seeded, and for good on a CPU with no random instruction.
static RNG: Mutex<Option<Csprng>> = Mutex::new(None);

/// RDSEED may run dry for a moment under load (Intel's guide: retry it).
const RDSEED_TRIES: u32 = 128;
/// RDRAND fails only when broken (Intel's guide: ten tries).
const RDRAND_TRIES: u32 = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Rdseed,
    Rdrand,
}

impl Source {
    fn name(self) -> &'static str {
        match self {
            Source::Rdseed => "RDSEED",
            Source::Rdrand => "RDRAND",
        }
    }
}

/// The best instruction the CPU offers (CPUID 7 EBX bit 18, 1 ECX bit 30).
fn source() -> Option<Source> {
    // Leaf 7 only when the CPU has it.
    if __cpuid(0).eax >= 7 && __cpuid_count(7, 0).ebx & (1 << 18) != 0 {
        return Some(Source::Rdseed);
    }
    if __cpuid(1).ecx & (1 << 30) != 0 {
        return Some(Source::Rdrand);
    }
    None
}

/// # Safety
/// Only on a CPU whose CPUID offers RDSEED.
#[target_feature(enable = "rdseed")]
unsafe fn rdseed() -> Option<u64> {
    let mut value = 0;
    for _ in 0..RDSEED_TRIES {
        if _rdseed64_step(&mut value) == 1 {
            return Some(value);
        }
        core::hint::spin_loop();
    }
    None
}

/// # Safety
/// Only on a CPU whose CPUID offers RDRAND.
#[target_feature(enable = "rdrand")]
unsafe fn rdrand() -> Option<u64> {
    let mut value = 0;
    for _ in 0..RDRAND_TRIES {
        if _rdrand64_step(&mut value) == 1 {
            return Some(value);
        }
    }
    None
}

/// 32 bytes from `source`, or None when it keeps failing.
fn hardware_seed(source: Source) -> Option<[u8; SEED_BYTES]> {
    let mut seed = [0u8; SEED_BYTES];
    for chunk in seed.as_chunks_mut::<8>().0 {
        // SAFETY: `source` came from CPUID, so the instruction exists.
        let word = unsafe {
            match source {
                Source::Rdseed => rdseed(),
                Source::Rdrand => rdrand(),
            }
        }?;
        *chunk = word.to_le_bytes();
    }
    Some(seed)
}

fn now_ms() -> u64 {
    crate::hpet::nanos() / 1_000_000
}

/// Seeds the generator, once the clock runs. On a CPU with neither instruction,
/// or one whose instruction keeps failing, it stays unseeded.
pub fn init() {
    let Some(source) = source() else {
        log!("entropy: the CPU has neither RDSEED nor RDRAND; ENTROPY refuses");
        return;
    };
    let Some(seed) = hardware_seed(source) else {
        log!("entropy: {} kept failing; ENTROPY refuses", source.name());
        return;
    };
    let rng = Csprng::new(seed, now_ms());
    without_interrupts(|| *RNG.lock() = Some(rng));
    log!("entropy: seeded from {}", source.name());
}

/// Fills `out` (at most [`csprng::MAX_REQUEST`] bytes), reseeding first when
/// due. False when the generator was never seeded.
pub fn fill(out: &mut [u8]) -> bool {
    let now = now_ms();
    // Read outside the lock: RDSEED may spin for a while.
    let due = without_interrupts(|| RNG.lock().as_ref().map(|r| r.needs_reseed(now)));
    let fresh = match due {
        None => return false,
        Some(true) => source().and_then(hardware_seed),
        Some(false) => None,
    };
    without_interrupts(|| {
        let mut rng = RNG.lock();
        let Some(rng) = rng.as_mut() else {
            return false;
        };
        match (due, fresh) {
            (Some(true), Some(fresh)) => rng.reseed(fresh, now),
            // A failed read keeps the current key, which is still sound.
            (Some(true), None) => rng.defer_reseed(now),
            _ => {}
        }
        rng.fill(out)
    })
}
