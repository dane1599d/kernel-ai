//! The `vm-hello` guest (P6 row 3's exit case): a tiny flat guest of our own,
//! which the hypervisor's run loop boots. It starts in real mode at [`LOAD`],
//! goes through protected mode into long mode on an identity map of the first
//! 2 MiB, writes `vm-hello` and a newline to COM1 (port 0x3F8) and halts.
//!
//! No assembler or tool made it: each instruction is written here as its bytes
//! with the text it must decode to, and the host tests decode the bytes back
//! (`decode.rs`) and pin the image's hash. A change to the guest shows in both.
//!
//! The guest gets RAM from 0 up, at least 48 KiB, and this layout:
//! - [`LOAD`] (0x8000): the image, under 4 KiB, entered at its first byte, in real
//!   mode with `cs:ip` = 0:0x8000, interrupts off.
//! - 0x9000, 0xA000, 0xB000: the PML4, PDPT and PD it builds (a 2 MiB page).

mod listing;

#[cfg(test)]
mod decode;
#[cfg(test)]
mod tests;

pub use listing::Guest;

/// Where the image is loaded and entered.
pub const LOAD: u32 = 0x8000;
/// The guest's page tables, built by the guest.
pub const PML4: u32 = 0x9000;
pub const PDPT: u32 = 0xA000;
pub const PD: u32 = 0xB000;
/// The serial port it writes to.
pub const COM1: u16 = 0x3F8;
/// What the guest writes before its newline: the line a run of it must show.
pub const EXPECTED_SERIAL: &str = "vm-hello";

/// The flat image, byte for byte.
pub fn image() -> Vec<u8> {
    Guest::build().image
}
