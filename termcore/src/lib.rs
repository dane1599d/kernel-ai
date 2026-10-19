//! The terminal's screen and input logic without hardware, tested on the host:
//! the console's cell grid and its `screenlog` copy, and the PS/2 mouse. The
//! kernel paints and drives them; the full-screen editor builds on them.

#![no_std]

#[cfg(test)]
extern crate std;

pub mod grid;
pub mod mouse;
pub mod screenlog;
