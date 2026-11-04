//! The kernel's random numbers (ENTROPY, P5 task 3's RANDOM): a
//! [`csprng::Csprng`] seeded at boot from every source the machine has, and
//! reseeded from them every [`csprng::RESEED_MS`] or [`csprng::RESEED_BYTES`]:
//! the CPU's RDSEED and RDRAND, and a virtio-rng device when there is one
//! ([`attach`]). Each source's 32 bytes are mixed into the key in turn
//! ([`Csprng::reseed`]), so no single source decides it. A machine with none
//! leaves it unseeded, and ENTROPY answers NoEntropy rather than weak bytes,
//! so a key is never made from them.

use crate::log;
use crate::virtio::hal::KernelHal;
use alloc::vec::Vec;
use core::arch::x86_64::{__cpuid, __cpuid_count, _rdrand64_step, _rdseed64_step};
use csprng::{Csprng, SEED_BYTES};
use spin::Mutex;
use virtio_drivers::device::rng::VirtIORng;
use virtio_drivers::transport::pci::PciTransport;
use x86_64::instructions::interrupts::without_interrupts;

/// None until seeded, and for good on a machine with no source.
static RNG: Mutex<Option<Csprng>> = Mutex::new(None);
/// The virtio-rng device, once the PCI walk found one.
static DEVICE: Mutex<Option<Device>> = Mutex::new(None);

/// RDSEED may run dry for a moment under load (Intel's guide: retry it).
const RDSEED_TRIES: u32 = 128;
/// RDRAND fails only when broken (Intel's guide: ten tries).
const RDRAND_TRIES: u32 = 10;
/// Requests to the device for one seed: it may answer short.
const DEVICE_TRIES: u32 = 8;

struct Device(VirtIORng<KernelHal, PciTransport>);

// SAFETY: the device is only used under the DEVICE mutex, by one thread at a
// time; nothing in it is tied to a particular CPU.
unsafe impl Send for Device {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Rdseed,
    Rdrand,
    VirtioRng,
}

impl Source {
    fn name(self) -> &'static str {
        match self {
            Source::Rdseed => "RDSEED",
            Source::Rdrand => "RDRAND",
            Source::VirtioRng => "virtio-rng",
        }
    }
}

/// The random instructions the CPU offers (CPUID 7 EBX bit 18, 1 ECX bit 30).
fn cpu_sources() -> Vec<Source> {
    let mut sources = Vec::new();
    // Leaf 7 only when the CPU has it.
    if __cpuid(0).eax >= 7 && __cpuid_count(7, 0).ebx & (1 << 18) != 0 {
        sources.push(Source::Rdseed);
    }
    if __cpuid(1).ecx & (1 << 30) != 0 {
        sources.push(Source::Rdrand);
    }
    sources
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

/// 32 bytes from the CPU's `source`, or None when it keeps failing.
fn cpu_seed(source: Source) -> Option<[u8; SEED_BYTES]> {
    let mut seed = [0u8; SEED_BYTES];
    for chunk in seed.as_chunks_mut::<8>().0 {
        // SAFETY: `source` came from CPUID, so the instruction exists.
        let word = unsafe {
            match source {
                Source::Rdseed => rdseed(),
                Source::Rdrand => rdrand(),
                Source::VirtioRng => None,
            }
        }?;
        *chunk = word.to_le_bytes();
    }
    Some(seed)
}

/// 32 bytes from the virtio-rng device, or None when there is none or it does
/// not fill them.
fn device_seed() -> Option<[u8; SEED_BYTES]> {
    let mut guard = DEVICE.lock();
    let device = guard.as_mut()?;
    let mut seed = [0u8; SEED_BYTES];
    let mut filled = 0;
    for _ in 0..DEVICE_TRIES {
        if filled == SEED_BYTES {
            break;
        }
        match device.0.request_entropy(&mut seed[filled..]) {
            Ok(n) => filled += n.min(SEED_BYTES - filled),
            Err(_) => return None,
        }
    }
    (filled == SEED_BYTES).then_some(seed)
}

/// Fresh bytes from every source that gave them, best first.
fn fresh_seeds() -> Vec<(Source, [u8; SEED_BYTES])> {
    let cpu = cpu_sources()
        .into_iter()
        .filter_map(|source| cpu_seed(source).map(|seed| (source, seed)));
    let device = device_seed().map(|seed| (Source::VirtioRng, seed));
    cpu.chain(device).collect()
}

fn names(seeds: &[(Source, [u8; SEED_BYTES])]) -> Vec<&'static str> {
    seeds.iter().map(|(source, _)| source.name()).collect()
}

fn now_ms() -> u64 {
    crate::hpet::nanos() / 1_000_000
}

/// Seeds the generator from every source there is, once the clock runs; or,
/// when there is none yet, leaves that to the first [`attach`].
pub fn init() {
    let seeds = fresh_seeds();
    let Some(rng) = seeded(&seeds) else {
        log!(
            "entropy: the CPU gives no random numbers (no RDSEED or RDRAND that works); ENTROPY refuses until a virtio-rng is found"
        );
        return;
    };
    without_interrupts(|| *RNG.lock() = Some(rng));
    log!("entropy: seeded from {}", names(&seeds).join(" and "));
}

/// A generator keyed from the first seed with the rest mixed in, or None.
fn seeded(seeds: &[(Source, [u8; SEED_BYTES])]) -> Option<Csprng> {
    let ((_, first), rest) = seeds.split_first()?;
    let now = now_ms();
    let mut rng = Csprng::new(*first, now);
    for (_, seed) in rest {
        rng.reseed(*seed, now);
    }
    Some(rng)
}

/// The PCI walk found a virtio-rng: it joins the sources, and its first 32
/// bytes are mixed in now, or seed the generator if nothing else could.
pub fn attach(transport: PciTransport) {
    let device = match VirtIORng::<KernelHal, _>::new(transport) {
        Ok(device) => device,
        Err(error) => {
            log!("entropy: the virtio-rng did not set up ({error:?})");
            return;
        }
    };
    without_interrupts(|| *DEVICE.lock() = Some(Device(device)));
    let Some(seed) = device_seed() else {
        log!("entropy: the virtio-rng gave no bytes; it is left out");
        without_interrupts(|| *DEVICE.lock() = None);
        return;
    };
    let now = now_ms();
    let first = without_interrupts(|| {
        let mut rng = RNG.lock();
        match rng.as_mut() {
            Some(rng) => {
                rng.reseed(seed, now);
                false
            }
            None => {
                *rng = Some(Csprng::new(seed, now));
                true
            }
        }
    });
    match first {
        true => log!("entropy: seeded from virtio-rng"),
        false => log!("entropy: virtio-rng mixed in"),
    }
}

/// Fills `out` (at most [`csprng::MAX_REQUEST`] bytes), reseeding first when
/// due. False when the generator was never seeded.
pub fn fill(out: &mut [u8]) -> bool {
    let now = now_ms();
    // Read outside the lock: RDSEED and the device may take a while.
    let due = without_interrupts(|| RNG.lock().as_ref().map(|r| r.needs_reseed(now)));
    let fresh = match due {
        None => return false,
        Some(true) => fresh_seeds(),
        Some(false) => Vec::new(),
    };
    without_interrupts(|| {
        let mut rng = RNG.lock();
        let Some(rng) = rng.as_mut() else {
            return false;
        };
        match (due, fresh.is_empty()) {
            (Some(true), false) => fresh.iter().for_each(|(_, seed)| rng.reseed(*seed, now)),
            // Every read failed: the current key, which is still sound, stays.
            (Some(true), true) => rng.defer_reseed(now),
            _ => {}
        }
        rng.fill(out)
    })
}
