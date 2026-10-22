//! The PS/2 controller: the keyboard on its first port, the mouse on its second
//! (aux) port. Both interrupt handlers drain the controller into one ring, each byte
//! tagged with its port, so a Ctrl press and a click keep the order they came in.
//! The handlers never take a lock; decoding happens in normal kernel context.

use crate::port::{inb, outb};
use crate::{apic, hpet, log};
use acpitables::Platform;
use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, AtomicUsize, Ordering};

const DATA: u16 = 0x60;
const STATUS_COMMAND: u16 = 0x64;
const OUTPUT_FULL: u8 = 1;
const INPUT_FULL: u8 = 1 << 1;
/// Status bit: the byte waiting came from the aux port.
const FROM_AUX: u8 = 1 << 5;
const READ_CONFIG: u8 = 0x20;
const WRITE_CONFIG: u8 = 0x60;
const ENABLE_AUX_PORT: u8 = 0xA8;
const ENABLE_FIRST_PORT: u8 = 0xAE;
const WRITE_AUX: u8 = 0xD4;
const CONFIG_IRQ1: u8 = 1;
const CONFIG_IRQ12: u8 = 1 << 1;
const CONFIG_CLOCK_DISABLED: u8 = 1 << 4;
const CONFIG_AUX_CLOCK_DISABLED: u8 = 1 << 5;
const CONFIG_TRANSLATE: u8 = 1 << 6;
const MOUSE_SET_DEFAULTS: u8 = 0xF6;
const MOUSE_ENABLE_STREAMING: u8 = 0xF4;
const MOUSE_ACK: u8 = 0xFA;
const SPIN_LIMIT: u32 = 1_000_000;
/// Most bytes one interrupt takes, so a stuck controller cannot hold the CPU.
const DRAIN_LIMIT: usize = 32;
const RING: usize = 256;
/// Set on a ring entry whose byte came from the aux port.
const AUX_TAG: u16 = 1 << 8;

/// One byte from either port.
#[derive(Clone, Copy)]
pub struct Input {
    pub byte: u8,
    /// The byte came from the mouse, not the keyboard.
    pub aux: bool,
}

static ENTRIES: [AtomicU16; RING] = [const { AtomicU16::new(0) }; RING];
/// HPET time at which each queued byte arrived, for wake-latency measurement.
static ARRIVED_NS: [AtomicU64; RING] = [const { AtomicU64::new(0) }; RING];
static HEAD: AtomicUsize = AtomicUsize::new(0);
static TAIL: AtomicUsize = AtomicUsize::new(0);
/// Thread blocked in [`wait`], or `NO_WAITER`.
static WAITER: AtomicUsize = AtomicUsize::new(NO_WAITER);
const NO_WAITER: usize = usize::MAX;
/// Wake-latency statistics: measured wake-ups and the worst one.
static WAKES: AtomicU64 = AtomicU64::new(0);
static WORST_WAKE_NS: AtomicU64 = AtomicU64::new(0);
/// Set once a mouse acknowledged stream mode.
static MOUSE: AtomicBool = AtomicBool::new(false);

fn status() -> u8 {
    // SAFETY: reading the i8042 status register has no side effects.
    unsafe { inb(STATUS_COMMAND) }
}

fn wait_until(ready: impl Fn(u8) -> bool) -> Result<(), &'static str> {
    for _ in 0..SPIN_LIMIT {
        if ready(status()) {
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err("i8042: PS/2 controller did not respond")
}

fn command(byte: u8) -> Result<(), &'static str> {
    wait_until(|s| s & INPUT_FULL == 0)?;
    // SAFETY: the controller is ready for a command byte.
    unsafe { outb(STATUS_COMMAND, byte) };
    Ok(())
}

fn write_data(byte: u8) -> Result<(), &'static str> {
    wait_until(|s| s & INPUT_FULL == 0)?;
    // SAFETY: the controller expects a data byte after the command just sent.
    unsafe { outb(DATA, byte) };
    Ok(())
}

/// Enables both ports with IRQ1, IRQ12 and scancode translation, turns on mouse
/// streaming, and routes IRQ1 and IRQ12. A mouse that does not answer is logged
/// and the keyboard still works.
pub fn init(platform: &Platform, apic_id: u8) -> Result<(), &'static str> {
    while status() & OUTPUT_FULL != 0 {
        // SAFETY: draining stale bytes from the i8042 data port.
        unsafe { inb(DATA) };
    }
    command(ENABLE_AUX_PORT)?;
    command(READ_CONFIG)?;
    wait_until(|s| s & OUTPUT_FULL != 0)?;
    // SAFETY: the controller has the configuration byte ready.
    let config = unsafe { inb(DATA) };
    let config = (config | CONFIG_IRQ1 | CONFIG_IRQ12 | CONFIG_TRANSLATE)
        & !(CONFIG_CLOCK_DISABLED | CONFIG_AUX_CLOCK_DISABLED);
    command(WRITE_CONFIG)?;
    write_data(config)?;
    command(ENABLE_FIRST_PORT)?;
    let mouse = enable_mouse();
    MOUSE.store(mouse.is_ok(), Ordering::Relaxed);
    apic::route_isa_irq(platform, 1, apic::KEYBOARD_VECTOR, apic_id)?;
    apic::route_isa_irq(platform, 12, apic::MOUSE_VECTOR, apic_id)?;
    match mouse {
        Ok(()) => log!("i8042: keyboard on IRQ1, mouse on IRQ12 (config {config:#04x})"),
        Err(message) => log!("i8042: keyboard on IRQ1 (config {config:#04x}); {message}"),
    }
    Ok(())
}

/// Whether a mouse answered at boot.
pub fn has_mouse() -> bool {
    MOUSE.load(Ordering::Relaxed)
}

/// Resets the mouse's settings and starts stream mode. Runs before the IRQs are
/// routed, so the answers are polled.
fn enable_mouse() -> Result<(), &'static str> {
    for byte in [MOUSE_SET_DEFAULTS, MOUSE_ENABLE_STREAMING] {
        command(WRITE_AUX)?;
        write_data(byte)?;
        if !mouse_acked() {
            return Err("no PS/2 mouse answered");
        }
    }
    Ok(())
}

/// Waits for the mouse's answer, dropping keyboard bytes that come first.
fn mouse_acked() -> bool {
    for _ in 0..SPIN_LIMIT {
        let status = status();
        if status & OUTPUT_FULL != 0 {
            // SAFETY: the controller has a byte ready.
            let byte = unsafe { inb(DATA) };
            if status & FROM_AUX != 0 {
                return byte == MOUSE_ACK;
            }
        }
        core::hint::spin_loop();
    }
    false
}

/// Called from the keyboard and mouse interrupts: queues every byte waiting, tagged
/// with its port, dropping it if the ring is full. Returns the thread to wake,
/// which the handler wakes after the EOI.
pub fn on_interrupt() -> Option<usize> {
    for _ in 0..DRAIN_LIMIT {
        let status = status();
        if status & OUTPUT_FULL == 0 {
            break;
        }
        // SAFETY: the status register says a byte is waiting.
        let byte = unsafe { inb(DATA) };
        let tag = if status & FROM_AUX != 0 { AUX_TAG } else { 0 };
        enqueue(u16::from(byte) | tag);
    }
    take_waiter()
}

/// Queues one ring entry, dropping it if the ring is full.
fn enqueue(entry: u16) {
    let head = HEAD.load(Ordering::Relaxed);
    if head.wrapping_sub(TAIL.load(Ordering::Acquire)) < RING {
        ENTRIES[head % RING].store(entry, Ordering::Relaxed);
        ARRIVED_NS[head % RING].store(hpet::nanos(), Ordering::Relaxed);
        HEAD.store(head.wrapping_add(1), Ordering::Release);
    }
}

fn take_waiter() -> Option<usize> {
    match WAITER.swap(NO_WAITER, Ordering::AcqRel) {
        NO_WAITER => None,
        tid => Some(tid),
    }
}

/// Queues keyboard bytes that came another way (a USB keyboard's keys as set 1
/// scancodes), as if the keyboard port had sent them. Must run on the CPU that
/// takes IRQ1, with interrupts off, so it never races [`on_interrupt`]: the
/// ring has one producer. Returns the thread to wake, as `on_interrupt` does.
pub fn inject(bytes: &[u8]) -> Option<usize> {
    // All or nothing: an E0 prefix queued without its code would change the
    // meaning of the next key.
    let used = HEAD
        .load(Ordering::Relaxed)
        .wrapping_sub(TAIL.load(Ordering::Acquire));
    if used + bytes.len() > RING {
        return None;
    }
    bytes.iter().for_each(|&byte| enqueue(u16::from(byte)));
    take_waiter()
}

fn has_input() -> bool {
    TAIL.load(Ordering::Relaxed) != HEAD.load(Ordering::Acquire)
}

/// The oldest queued byte, if any.
pub fn next() -> Option<Input> {
    let tail = TAIL.load(Ordering::Relaxed);
    if tail == HEAD.load(Ordering::Acquire) {
        return None;
    }
    let entry = ENTRIES[tail % RING].load(Ordering::Relaxed);
    TAIL.store(tail.wrapping_add(1), Ordering::Release);
    Some(Input {
        byte: entry as u8,
        aux: entry & AUX_TAG != 0,
    })
}

/// Blocks the calling thread until a byte arrives, and records how long the
/// thread took to run after the interrupt that woke it.
pub fn wait() -> Input {
    loop {
        if let Some(input) = next() {
            return input;
        }
        // SeqCst orders this store before the ring check inside block_unless, so the
        // interrupt cannot both miss the waiter and have its byte go unseen.
        WAITER.store(crate::threads::current(), Ordering::SeqCst);
        if crate::threads::block_unless(has_input) && has_input() {
            // After a real block the ring was empty, so the byte at TAIL is the first
            // to arrive since. A stale wake on an empty ring is not counted.
            let tail = TAIL.load(Ordering::Relaxed);
            let arrived = ARRIVED_NS[tail % RING].load(Ordering::Relaxed);
            let latency = hpet::nanos().saturating_sub(arrived);
            WAKES.fetch_add(1, Ordering::Relaxed);
            WORST_WAKE_NS.fetch_max(latency, Ordering::Relaxed);
        }
    }
}

/// Number of measured wake-ups and the worst interrupt-to-running latency in ns.
pub fn wake_latency() -> (u64, u64) {
    (
        WAKES.load(Ordering::Relaxed),
        WORST_WAKE_NS.load(Ordering::Relaxed),
    )
}
