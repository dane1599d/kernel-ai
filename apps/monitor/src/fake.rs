//! A stand-in for the OS's effect queue, for the host tests of this crate and of
//! `comp-monitor-ui`: it keeps each call and gives tickets from 1.

use crate::{Os, rows::Call};
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
    /// The ticket the next call will get, which is also how many were asked.
    pub fn count(&self) -> u32 {
        self.calls.len() as u32
    }

    /// The ticket of the newest call named `name`.
    pub fn ticket_of(&self, name: &str) -> Option<u32> {
        self.calls
            .iter()
            .rposition(|c| c.name == name)
            .map(|i| i as u32 + 1)
    }
}
