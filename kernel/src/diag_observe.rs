//! DIAG (not for merge): where two observe boots part. From the aligned edge on,
//! every interrupt is counted by vector, and every timer tick folds the
//! interrupted RIP and how late the tick came into a hash kept per 100 ticks.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const BLOCKS: usize = 64;
const TICKS_PER_BLOCK: u64 = 100;

static ON: AtomicBool = AtomicBool::new(false);
static TICK: AtomicU64 = AtomicU64::new(0);
static HASH: AtomicU64 = AtomicU64::new(0xcbf2_9ce4_8422_2325);
static BLOCK_HASHES: [AtomicU64; BLOCKS] = [const { AtomicU64::new(0) }; BLOCKS];
static VECTORS: [AtomicU64; 256] = [const { AtomicU64::new(0) }; 256];

pub fn start() {
    ON.store(true, Ordering::SeqCst);
}

fn fold(hash: u64, value: u64) -> u64 {
    let mut h = hash;
    for byte in value.to_le_bytes() {
        h = (h ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Every interrupt, from `trap_dispatch`.
pub fn on_vector(vector: u64, rip: u64) {
    if !ON.load(Ordering::Relaxed) || crate::percpu::index() != 0 {
        return;
    }
    VECTORS[(vector & 0xFF) as usize].fetch_add(1, Ordering::Relaxed);
    if vector != u64::from(crate::apic::TIMER_VECTOR) {
        let h = fold(HASH.load(Ordering::Relaxed), vector << 56 | rip);
        HASH.store(h, Ordering::Relaxed);
        return;
    }
    let tick = TICK.fetch_add(1, Ordering::Relaxed);
    let late = crate::timer::lateness_ns();
    let h = fold(fold(HASH.load(Ordering::Relaxed), rip), late);
    HASH.store(h, Ordering::Relaxed);
    if (tick + 1).is_multiple_of(TICKS_PER_BLOCK) {
        let block = (tick / TICKS_PER_BLOCK) as usize;
        if block < BLOCKS {
            BLOCK_HASHES[block].store(h, Ordering::Relaxed);
        }
    }
}

pub fn report() {
    ON.store(false, Ordering::SeqCst);
    let mut line = alloc::string::String::new();
    for (vector, count) in VECTORS.iter().enumerate() {
        let count = count.load(Ordering::Relaxed);
        if count != 0 {
            line.push_str(&alloc::format!(" {vector:#x}={count}"));
        }
    }
    crate::log!("diag: vectors:{line}");
    for (block, hash) in BLOCK_HASHES.iter().enumerate() {
        let hash = hash.load(Ordering::Relaxed);
        if hash != 0 {
            crate::log!("diag: block {block} {:016x}", hash);
        }
    }
}
