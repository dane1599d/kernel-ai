//! The keyboard layouts as data, one file each ([`crate::keymap::Layout`],
//! plan L-2). Each names every key of the main block by its HID usage.

pub mod de;
pub mod es;
pub mod fr;
pub mod gb;
pub mod nordic;
pub mod us;

#[cfg(test)]
mod tests;
