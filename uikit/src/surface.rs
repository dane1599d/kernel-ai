//! Where frames go. comphost's real one writes the header page and the
//! `a11y.tree_update` records of `kai-surface 1`; until `surfabi` lands,
//! [`MemSurface`] keeps them for tests.

use crate::window::Frame;
use alloc::vec::Vec;

pub trait Surface {
    fn present(&mut self, frame: &Frame);
}

/// Keeps every frame presented.
#[derive(Default)]
pub struct MemSurface {
    pub frames: Vec<Frame>,
}

impl Surface for MemSurface {
    fn present(&mut self, frame: &Frame) {
        self.frames.push(frame.clone());
    }
}
