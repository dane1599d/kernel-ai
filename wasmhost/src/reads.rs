//! The reads a granted interface's `read` queues (`kai:system`, `kai:models`):
//! each read is a ticket the host takes after the call and answers in a later
//! one. The queue is short ([`MAX_QUEUED`]) because the app wants the latest
//! answer, not a backlog, and a call that faults leaves nothing queued.
//!
//! Tickets count from 0 for each instance, so a read's answer must go to the
//! instance that made it and to no later one. The host keeps that by never
//! holding a read itself: a read waits in the instance's own queue
//! ([`Reads::waiting`] says whether one does, which also keeps the app from
//! napping), and the host takes it ([`Reads::take`]) only when it can answer in
//! the same step. A reloaded, resumed or swapped instance starts with an empty
//! queue, so it is never answered for its predecessor.

use alloc::vec::Vec;
use wasmi::Error;

/// Most reads waiting for the host; a call that makes more traps.
pub const MAX_QUEUED: usize = 4;

/// The tickets of the reads not yet taken, and the next ticket.
#[derive(Default)]
pub(crate) struct Reads {
    queue: Vec<u32>,
    next: u32,
    /// Where this call's reads start in the queue.
    call_start: usize,
}

impl Reads {
    /// `<interface>.read() -> u32`: the read's ticket.
    pub(crate) fn read(&mut self, interface: &str) -> Result<i32, Error> {
        if self.queue.len() >= MAX_QUEUED {
            return Err(Error::new(alloc::format!(
                "{interface} read: {MAX_QUEUED} reads waiting for comphost"
            )));
        }
        let ticket = self.next;
        self.next = self
            .next
            .checked_add(1)
            .ok_or_else(|| Error::new(alloc::format!("{interface} read: tickets used up")))?;
        self.queue.push(ticket);
        Ok(ticket as i32)
    }

    /// Starts counting a call's reads: where they start in the queue.
    pub(crate) fn call_begins(&mut self) {
        self.call_start = self.queue.len();
    }

    /// A call faulted: what it queued is not sent.
    pub(crate) fn call_faulted(&mut self) {
        let start = self.call_start.min(self.queue.len());
        self.queue.truncate(start);
    }

    /// Whether a read waits to be answered.
    pub(crate) fn waiting(&self) -> bool {
        !self.queue.is_empty()
    }

    /// The tickets of the reads made since the last take, oldest first.
    pub(crate) fn take(&mut self) -> Vec<u32> {
        core::mem::take(&mut self.queue)
    }
}
