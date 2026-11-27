//! Where the network path's time goes (G67's measuring): counts and
//! nanoseconds per stage, read by `test=net-throughput` before and after its
//! download. Each stage is timed with the clock `hpet::nanos` gives, the TSC
//! under KVM.

use core::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy)]
pub enum Stage {
    /// `KernelHal::share` finding and taking bounce frames.
    BounceAlloc,
    /// The copies into and out of bounce frames.
    BounceCopy,
    /// `KernelHal::unshare` freeing bounce frames.
    BounceFree,
    /// smoltcp's `Interface::poll`, the device's receives and sends inside.
    IfacePoll,
    /// Giving receive buffers back and kicking the card.
    Refill,
    /// `stream::recv`'s whole call: both polls and the copy out.
    Recv,
    /// The copy out of the socket's buffer alone.
    RecvCopy,
    /// A waiter's sleep for frames.
    Wait,
    /// Waiting for the `NET` lock.
    Lock,
}

const STAGES: usize = 9;
pub const NAMES: [&str; STAGES] = [
    "bounce_alloc",
    "bounce_copy",
    "bounce_free",
    "iface_poll",
    "refill",
    "recv",
    "recv_copy",
    "wait",
    "lock",
];

static COUNT: [AtomicU64; STAGES] = [const { AtomicU64::new(0) }; STAGES];
static NANOS: [AtomicU64; STAGES] = [const { AtomicU64::new(0) }; STAGES];

/// Runs `f`, counting it and its time under `stage`.
pub fn timed<R>(stage: Stage, f: impl FnOnce() -> R) -> R {
    let start = crate::hpet::nanos();
    let out = f();
    let spent = crate::hpet::nanos().saturating_sub(start);
    COUNT[stage as usize].fetch_add(1, Ordering::Relaxed);
    NANOS[stage as usize].fetch_add(spent, Ordering::Relaxed);
    out
}

/// Every stage's count and nanoseconds now.
pub fn snapshot() -> [(u64, u64); STAGES] {
    core::array::from_fn(|i| {
        (
            COUNT[i].load(Ordering::Relaxed),
            NANOS[i].load(Ordering::Relaxed),
        )
    })
}
