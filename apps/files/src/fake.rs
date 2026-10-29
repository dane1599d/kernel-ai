//! A stand-in for the OS's effect queue, for the host tests of this crate and of
//! `comp-files-ui`: it keeps each call and gives tickets from 1.

use crate::{Os, calls::Call};
use alloc::vec::Vec;

#[derive(Default)]
pub struct Queue {
    pub calls: Vec<Call>,
}

impl Os for Queue {
    fn request(&mut self, call: &Call) -> u32 {
        self.calls.push(call.clone());
        self.calls.len() as u32
    }
}

impl Queue {
    /// The newest call's ticket, and the call as `name args`.
    pub fn last(&self) -> Option<(u32, alloc::string::String)> {
        let call = self.calls.last()?;
        Some((
            self.calls.len() as u32,
            alloc::format!("{} {}", call.name, call.args),
        ))
    }
}
