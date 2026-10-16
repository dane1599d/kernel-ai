//! Raw x86 port I/O.

use core::arch::asm;

/// Writes one byte to an I/O port.
///
/// # Safety
/// The caller must know the port's effect on the device behind it.
pub unsafe fn outb(port: u16, value: u8) {
    unsafe {
        asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags))
    }
}

/// Reads one byte from an I/O port.
///
/// # Safety
/// The caller must know the port's effect on the device behind it.
pub unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    unsafe {
        asm!("in al, dx", out("al") value, in("dx") port, options(nomem, nostack, preserves_flags))
    }
    value
}

/// Writes one 32-bit value to an I/O port.
///
/// # Safety
/// The caller must know the port's effect on the device behind it.
pub unsafe fn outl(port: u16, value: u32) {
    unsafe {
        asm!("out dx, eax", in("dx") port, in("eax") value, options(nomem, nostack, preserves_flags))
    }
}
