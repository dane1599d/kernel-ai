//! COM1 serial logger. In QEMU, COM1 is wired to the test harness.

use crate::port::{inb, outb};
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use x86_64::instructions::interrupts;

const COM1: u16 = 0x3F8;
const LINE_STATUS: u16 = COM1 + 5;
const TRANSMIT_EMPTY: u8 = 0x20;

/// Programs COM1 for 38400 baud, 8N1, FIFO on, interrupts off.
pub fn init() {
    // SAFETY: standard 16550 UART programming sequence on the COM1 ports.
    unsafe {
        outb(COM1 + 1, 0x00); // interrupts off
        outb(COM1 + 3, 0x80); // DLAB on
        outb(COM1, 0x03); // divisor low: 38400 baud
        outb(COM1 + 1, 0x00); // divisor high
        outb(COM1 + 3, 0x03); // 8 bits, no parity, 1 stop bit, DLAB off
        outb(COM1 + 2, 0xC7); // FIFO on, cleared, 14-byte threshold
        outb(COM1 + 4, 0x0B); // DTR, RTS, OUT2
    }
}

/// Bytes sent so far, for a measurement window that must stay quiet (M10a).
static SENT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

pub fn bytes_sent() -> u64 {
    SENT.load(core::sync::atomic::Ordering::Relaxed)
}

fn write_byte(byte: u8) {
    SENT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    // SAFETY: polling the line status register and writing the data register of COM1.
    unsafe {
        while inb(LINE_STATUS) & TRANSMIT_EMPTY == 0 {
            core::hint::spin_loop();
        }
        outb(COM1, byte);
    }
}

pub struct Serial;

/// Whether the last byte sent ended a line.
static AT_LINE_START: AtomicBool = AtomicBool::new(true);

impl Write for Serial {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            if byte == b'\n' {
                write_byte(b'\r');
            }
            write_byte(byte);
        }
        if let Some(&last) = s.as_bytes().last() {
            AT_LINE_START.store(last == b'\n', Ordering::Relaxed);
        }
        Ok(())
    }
}

/// Whether serial output is at the start of a line.
pub fn at_line_start() -> bool {
    AT_LINE_START.load(Ordering::Relaxed)
}

/// CPU index plus one of the CPU printing a line, or 0. Keeps lines from different
/// CPUs apart. A CPU may re-enter (a fault while printing) without deadlocking.
static LINE_OWNER: AtomicUsize = AtomicUsize::new(0);

/// Runs `f` with interrupts off while holding the line lock, so nothing it prints
/// mixes with other CPUs' lines. Take it before the console lock, and allocate
/// nothing under it.
pub fn with_line<R>(f: impl FnOnce() -> R) -> R {
    interrupts::without_interrupts(|| {
        let me = crate::percpu::index() + 1;
        let reentered = LINE_OWNER.load(Ordering::Acquire) == me;
        if !reentered {
            while LINE_OWNER
                .compare_exchange(0, me, Ordering::Acquire, Ordering::Relaxed)
                .is_err()
            {
                core::hint::spin_loop();
            }
        }
        let result = f();
        if !reentered {
            LINE_OWNER.store(0, Ordering::Release);
        }
        result
    })
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    with_line(|| {
        // Serial writes cannot fail; the Result exists only to satisfy fmt::Write.
        let _ = Serial.write_fmt(args);
        crate::console::print(args);
    });
}

#[doc(hidden)]
pub fn _print_quiet(args: fmt::Arguments) {
    with_line(|| {
        let _ = Serial.write_fmt(args);
        crate::logbook::record(args);
    });
}

/// Releases the line lock unconditionally. Only for fatal-error reporting, after the
/// other CPUs were halted, possibly while holding it.
pub fn break_line_lock() {
    LINE_OWNER.store(0, Ordering::Release);
}

/// Prints a line to COM1 and the framebuffer console.
#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {
        $crate::serial::_print(format_args!("{}\n", format_args!($($arg)*)))
    };
}

/// Prints a line to COM1 and the log book (`:log`), not the framebuffer: an audit
/// trail or a note the person at the screen did not ask for.
#[macro_export]
macro_rules! quiet {
    ($($arg:tt)*) => {
        $crate::serial::_print_quiet(format_args!("{}\n", format_args!($($arg)*)))
    };
}
