//! The terminal's screen and input logic without hardware, tested on the host:
//! the console's cell grid and its `screenlog` copy, key decoding and key tokens,
//! the line typed at a prompt, the Allow / Deny menu, the rows the terminal window
//! mirrors, the PS/2 mouse, and the kind
//! of each screen row,
//! which sets its colours. The kernel paints and drives them; the full-screen
//! editor builds on them.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod grid;
pub mod keys;
pub mod line;
pub mod menu;
pub mod mirror;
pub mod mouse;
pub mod once;
pub mod plain;
pub mod screenlog;
pub mod style;
