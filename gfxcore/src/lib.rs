//! The display's logic without hardware, tested on the host: screen rectangles,
//! the modes an EDID block lists, which mode the kernel picks (and `display=` on
//! the command line), and the test pattern the `display` boot cases draw and xtask
//! checks pixel for pixel.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod edid;
pub mod mode;
pub mod pattern;
pub mod rect;

pub use rect::Rect;
