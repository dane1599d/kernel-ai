//! COM1 serial logger. In QEMU, COM1 is wired to the test harness.

use crate::port::{inb, outb};
use core::fmt::{self, Write};

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

fn write_byte(byte: u8) {
    // SAFETY: polling the line status register and writing the data register of COM1.
    unsafe {
        while inb(LINE_STATUS) & TRANSMIT_EMPTY == 0 {
            core::hint::spin_loop();
        }
        outb(COM1, byte);
    }
}

pub struct Serial;

impl Write for Serial {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            if byte == b'\n' {
                write_byte(b'\r');
            }
            write_byte(byte);
        }
        Ok(())
    }
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    // Serial writes cannot fail; the Result exists only to satisfy fmt::Write.
    let _ = Serial.write_fmt(args);
    crate::console::print(args);
}

/// Prints a line to COM1 and the framebuffer console.
#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {
        $crate::serial::_print(format_args!("{}\n", format_args!($($arg)*)))
    };
}
