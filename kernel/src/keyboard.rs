//! PS/2 keyboard. The interrupt handler only queues raw scancodes; decoding and echo
//! happen in normal kernel context, so the handler never takes a lock.

use crate::port::{inb, outb};
use crate::{apic, hpet, log, serial};
use acpitables::Platform;
use alloc::string::String;
use core::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};
use pc_keyboard::layouts::Us104Key;
use pc_keyboard::{DecodedKey, HandleControl, PS2Keyboard, ScancodeSet1};

const DATA: u16 = 0x60;
const STATUS_COMMAND: u16 = 0x64;
const OUTPUT_FULL: u8 = 1;
const INPUT_FULL: u8 = 1 << 1;
const READ_CONFIG: u8 = 0x20;
const WRITE_CONFIG: u8 = 0x60;
const ENABLE_FIRST_PORT: u8 = 0xAE;
const CONFIG_IRQ1: u8 = 1;
const CONFIG_CLOCK_DISABLED: u8 = 1 << 4;
const CONFIG_TRANSLATE: u8 = 1 << 6;
const SPIN_LIMIT: u32 = 1_000_000;
const RING: usize = 256;

static SCANCODES: [AtomicU8; RING] = [const { AtomicU8::new(0) }; RING];
/// HPET time at which each queued scancode arrived, for wake-latency measurement.
static ARRIVED_NS: [AtomicU64; RING] = [const { AtomicU64::new(0) }; RING];
static HEAD: AtomicUsize = AtomicUsize::new(0);
static TAIL: AtomicUsize = AtomicUsize::new(0);

fn wait_until(ready: impl Fn(u8) -> bool) -> Result<(), &'static str> {
    for _ in 0..SPIN_LIMIT {
        // SAFETY: reading the i8042 status register has no side effects.
        if ready(unsafe { inb(STATUS_COMMAND) }) {
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err("keyboard: PS/2 controller did not respond")
}

fn command(byte: u8) -> Result<(), &'static str> {
    wait_until(|s| s & INPUT_FULL == 0)?;
    // SAFETY: the controller is ready for a command byte.
    unsafe { outb(STATUS_COMMAND, byte) };
    Ok(())
}

/// Enables IRQ1 with scancode translation and routes it to `KEYBOARD_VECTOR`.
pub fn init(platform: &Platform, apic_id: u8) -> Result<(), &'static str> {
    // SAFETY: draining stale bytes from the i8042 data port.
    while unsafe { inb(STATUS_COMMAND) } & OUTPUT_FULL != 0 {
        unsafe { inb(DATA) };
    }
    command(READ_CONFIG)?;
    wait_until(|s| s & OUTPUT_FULL != 0)?;
    // SAFETY: the controller has the configuration byte ready.
    let config = unsafe { inb(DATA) };
    let config = (config | CONFIG_IRQ1 | CONFIG_TRANSLATE) & !CONFIG_CLOCK_DISABLED;
    command(WRITE_CONFIG)?;
    wait_until(|s| s & INPUT_FULL == 0)?;
    // SAFETY: the controller expects the configuration byte after WRITE_CONFIG.
    unsafe { outb(DATA, config) };
    command(ENABLE_FIRST_PORT)?;
    apic::route_isa_irq(platform, 1, apic::KEYBOARD_VECTOR, apic_id)?;
    log!("keyboard: PS/2 on IRQ1 (config {config:#04x})");
    Ok(())
}

/// Thread blocked in [`wait_scancode`], or `NO_WAITER`.
static WAITER: AtomicUsize = AtomicUsize::new(NO_WAITER);
const NO_WAITER: usize = usize::MAX;
/// Wake-latency statistics: measured wake-ups and the worst one.
static WAKES: AtomicU64 = AtomicU64::new(0);
static WORST_WAKE_NS: AtomicU64 = AtomicU64::new(0);

/// Called from the keyboard interrupt: queue the scancode, dropping it if the ring
/// is full. Returns the thread to wake, which the handler wakes after the EOI.
pub fn on_interrupt() -> Option<usize> {
    // SAFETY: the controller raised IRQ1, so a byte is waiting.
    let scancode = unsafe { inb(DATA) };
    let head = HEAD.load(Ordering::Relaxed);
    if head.wrapping_sub(TAIL.load(Ordering::Acquire)) < RING {
        SCANCODES[head % RING].store(scancode, Ordering::Relaxed);
        ARRIVED_NS[head % RING].store(hpet::nanos(), Ordering::Relaxed);
        HEAD.store(head.wrapping_add(1), Ordering::Release);
    }
    match WAITER.swap(NO_WAITER, Ordering::AcqRel) {
        NO_WAITER => None,
        tid => Some(tid),
    }
}

fn has_scancode() -> bool {
    TAIL.load(Ordering::Relaxed) != HEAD.load(Ordering::Acquire)
}

/// Blocks the calling thread until a scancode arrives, and records how long the
/// thread took to run after the interrupt that woke it.
fn wait_scancode() -> u8 {
    loop {
        if let Some(scancode) = next_scancode() {
            return scancode;
        }
        // SeqCst orders this store before the ring check inside block_unless, so the
        // interrupt cannot both miss the waiter and have its scancode go unseen.
        WAITER.store(crate::threads::current(), Ordering::SeqCst);
        if crate::threads::block_unless(has_scancode) && has_scancode() {
            // After a real block the ring was empty, so the scancode at TAIL is the first
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

fn next_scancode() -> Option<u8> {
    let tail = TAIL.load(Ordering::Relaxed);
    if tail == HEAD.load(Ordering::Acquire) {
        return None;
    }
    let scancode = SCANCODES[tail % RING].load(Ordering::Relaxed);
    TAIL.store(tail.wrapping_add(1), Ordering::Release);
    Some(scancode)
}

pub struct Decoder(PS2Keyboard<Us104Key, ScancodeSet1>);

impl Decoder {
    pub fn new() -> Self {
        Self(PS2Keyboard::new(
            ScancodeSet1::new(),
            Us104Key,
            HandleControl::Ignore,
        ))
    }

    fn decode(&mut self, scancode: u8) -> Option<char> {
        match self.0.add_byte(scancode) {
            Ok(Some(event)) => match self.0.process_keyevent(event) {
                Some(DecodedKey::Unicode(ch)) => Some(ch),
                _ => None,
            },
            _ => None,
        }
    }

    /// Decodes queued scancodes until one produces a character.
    pub fn next_char(&mut self) -> Option<char> {
        while let Some(scancode) = next_scancode() {
            if let Some(ch) = self.decode(scancode) {
                return Some(ch);
            }
        }
        None
    }

    /// Blocks the calling thread until a key produces a character.
    pub fn read_char(&mut self) -> char {
        loop {
            if let Some(ch) = self.decode(wait_scancode()) {
                return ch;
            }
        }
    }
}

/// Echoes typed characters to the console and serial port, forever.
pub fn echo_forever() -> ! {
    let mut decoder = Decoder::new();
    loop {
        let ch = decoder.read_char();
        serial::_print(format_args!("{ch}"));
    }
}

/// M3 exit test: the harness types `kai` and Enter; the kernel must echo and receive it.
pub fn typing_check() -> Result<(), &'static str> {
    const TIMEOUT_NS: u64 = 30_000_000_000;
    log!("keyboard: ready for keys, waiting for \"kai\" and Enter");
    let mut decoder = Decoder::new();
    let mut line = String::new();
    let start = hpet::nanos();
    loop {
        while let Some(ch) = decoder.next_char() {
            serial::_print(format_args!("{ch}"));
            if matches!(ch, '\n' | '\r') {
                if line == "kai" {
                    log!("keyboard: received \"kai\"");
                    return Ok(());
                }
                line.clear();
            } else {
                line.push(ch);
            }
        }
        if hpet::nanos() - start > TIMEOUT_NS {
            return Err("keyboard: no \"kai\" line typed within 30 s");
        }
        x86_64::instructions::interrupts::enable_and_hlt();
    }
}
