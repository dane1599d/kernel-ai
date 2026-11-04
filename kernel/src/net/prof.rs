//! Spike P5-S2 only: where a download's time goes, in TSC ticks per phase.
//! A [`Span`] adds the ticks from its start to its drop to its phase. Phases
//! nest (`Poll` holds the card's work and the delay queue's), so the
//! `net-throughput` case subtracts the inner ones to get smoltcp's own share.

use core::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy)]
pub enum Phase {
    /// Taking the `NET` lock.
    Lock,
    /// `Net::poll`: smoltcp and everything it calls below.
    Poll,
    /// The card's received frame: poll the used ring and complete it.
    CardRx,
    /// A receive buffer given back to the card.
    CardRefill,
    /// A frame given to the card, and the sent ones reclaimed.
    CardTx,
    /// The spike's delay queue: held frames sent when due, new ones held.
    Delay,
    /// Copying received bytes out of the socket, 8 KiB at a time, as NET_RECV does.
    Copy,
    /// Between polls: the yield or the sleep.
    Wait,
}

pub const PHASES: usize = 8;
pub const NAMES: [&str; PHASES] = [
    "lock",
    "poll",
    "card rx",
    "card refill",
    "card tx",
    "delay queue",
    "copy",
    "wait",
];

static TICKS: [AtomicU64; PHASES] = [const { AtomicU64::new(0) }; PHASES];
/// Frames received and sent through the card.
pub static RX_FRAMES: AtomicU64 = AtomicU64::new(0);
pub static TX_FRAMES: AtomicU64 = AtomicU64::new(0);

pub fn tsc() -> u64 {
    // SAFETY: RDTSC has no side effects and every x86_64 CPU has it.
    unsafe { core::arch::x86_64::_rdtsc() }
}

/// Times one phase until it is dropped.
pub struct Span(Phase, u64);

impl Span {
    pub fn start(phase: Phase) -> Span {
        Span(phase, tsc())
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        TICKS[self.0 as usize].fetch_add(tsc().wrapping_sub(self.1), Ordering::Relaxed);
    }
}

/// Zeroes every counter, before a download.
pub fn reset() {
    TICKS.iter().for_each(|t| t.store(0, Ordering::Relaxed));
    RX_FRAMES.store(0, Ordering::Relaxed);
    TX_FRAMES.store(0, Ordering::Relaxed);
}

/// Every phase's ticks since [`reset`].
pub fn ticks() -> [u64; PHASES] {
    core::array::from_fn(|i| TICKS[i].load(Ordering::Relaxed))
}
