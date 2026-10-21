//! Ring bookkeeping. The command ring and each transfer ring are producer rings:
//! the driver writes TRBs, the controller consumes them, and the last slot holds
//! a Link TRB back to the start that flips the cycle state. The event ring runs
//! the other way: the controller writes, and a TRB is new while its cycle bit
//! matches the driver's consumer cycle state. Each ring here is one page.

use crate::trb::{self, Trb};

/// TRBs in a one-page ring.
pub const ENTRIES: u16 = (crate::PAGE / trb::BYTES) as u16;

/// Where one TRB goes and what to write there, cycle bit included.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Write {
    pub index: u16,
    pub trb: Trb,
}

/// A ring the driver produces into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Producer {
    base: u64,
    entries: u16,
    enqueue: u16,
    cycle: bool,
}

impl Producer {
    /// A ring of `entries` TRBs at physical address `base`, the last its link.
    pub fn new(base: u64, entries: u16) -> Self {
        assert!(entries >= 2, "a ring needs a TRB and its link");
        Self {
            base,
            entries,
            enqueue: 0,
            cycle: true,
        }
    }

    /// The writes that queue `trb`: the TRB at the enqueue slot, then, when that
    /// filled the ring's last usable slot, the link with the same cycle bit. The
    /// caller writes them in order; the controller sees nothing until the cycle
    /// bit lands, so it writes each TRB's control dword last.
    pub fn push(&mut self, trb: Trb) -> (Write, Option<Write>) {
        let write = Write {
            index: self.enqueue,
            trb: trb.with_cycle(self.cycle),
        };
        self.enqueue += 1;
        let mut link = None;
        if self.enqueue == self.entries - 1 {
            link = Some(Write {
                index: self.enqueue,
                trb: Trb::link(self.base).with_cycle(self.cycle),
            });
            self.enqueue = 0;
            self.cycle = !self.cycle;
        }
        (write, link)
    }

    /// The physical address of TRB `index`, as events name it.
    pub fn address(&self, index: u16) -> u64 {
        self.base + u64::from(index) * trb::BYTES as u64
    }

    /// Where the next TRB goes and its cycle state: what Set TR Dequeue Pointer
    /// takes after a halted endpoint is reset.
    pub fn dequeue(&self) -> (u64, bool) {
        (self.address(self.enqueue), self.cycle)
    }

    pub fn base(&self) -> u64 {
        self.base
    }
}

/// The event ring, which the driver consumes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Consumer {
    base: u64,
    entries: u16,
    dequeue: u16,
    cycle: bool,
}

impl Consumer {
    pub fn new(base: u64, entries: u16) -> Self {
        Self {
            base,
            entries,
            dequeue: 0,
            cycle: true,
        }
    }

    /// The slot to look at next.
    pub fn index(&self) -> u16 {
        self.dequeue
    }

    /// Whether `trb`, read from [`Self::index`], is an event the driver has not
    /// handled yet.
    pub fn is_new(&self, trb: &Trb) -> bool {
        trb.cycle() == self.cycle
    }

    /// Steps past the event just handled, flipping the cycle state at the end of
    /// the ring.
    pub fn advance(&mut self) {
        self.dequeue += 1;
        if self.dequeue == self.entries {
            self.dequeue = 0;
            self.cycle = !self.cycle;
        }
    }

    /// The value for ERDP: the next slot to read, with the busy bit cleared.
    pub fn erdp(&self) -> u64 {
        (self.base + u64::from(self.dequeue) * trb::BYTES as u64) | crate::regs::ERDP_BUSY
    }
}

/// One Event Ring Segment Table entry for a segment at `base` of `entries` TRBs.
pub fn segment_entry(base: u64, entries: u16) -> [u32; 4] {
    [base as u32, (base >> 32) as u32, u32::from(entries), 0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_holds_256_trbs() {
        assert_eq!(ENTRIES, 256);
    }

    #[test]
    fn pushes_fill_the_ring_then_link_back_with_the_cycle_flipped() {
        let mut ring = Producer::new(0x10_000, 4);
        let (first, link) = ring.push(Trb::noop_command());
        assert_eq!(first.index, 0);
        assert!(first.trb.cycle());
        assert_eq!(link, None);
        let (second, link) = ring.push(Trb::noop_command());
        assert_eq!(second.index, 1);
        assert_eq!(link, None);
        // The third TRB fills slot 2; slot 3 is the link, written with cycle 1.
        let (third, link) = ring.push(Trb::noop_command());
        assert_eq!(third.index, 2);
        let link = link.expect("the ring wraps after its last usable slot");
        assert_eq!(link.index, 3);
        assert_eq!(link.trb.kind(), trb::LINK);
        assert_eq!(link.trb.parameter(), 0x10_000);
        assert!(link.trb.cycle());
        // The next lap writes cycle 0.
        let (again, _) = ring.push(Trb::noop_command());
        assert_eq!(again.index, 0);
        assert!(!again.trb.cycle());
        assert_eq!(ring.dequeue(), (0x10_010, false));
        assert_eq!(ring.address(3), 0x10_030);
    }

    #[test]
    fn events_are_new_while_their_cycle_matches_and_the_state_flips_at_the_end() {
        let mut ring = Consumer::new(0x20_000, 2);
        let fresh = Trb::noop_command().with_cycle(true);
        let stale = Trb::noop_command().with_cycle(false);
        assert!(ring.is_new(&fresh));
        assert!(!ring.is_new(&stale));
        assert_eq!(ring.erdp(), 0x20_000 | crate::regs::ERDP_BUSY);
        ring.advance();
        assert_eq!(ring.index(), 1);
        assert_eq!(ring.erdp(), 0x20_010 | crate::regs::ERDP_BUSY);
        ring.advance();
        assert_eq!(ring.index(), 0);
        assert!(ring.is_new(&stale));
        assert!(!ring.is_new(&fresh));
    }

    #[test]
    fn segment_entries_split_the_address() {
        assert_eq!(segment_entry(0x1_0000_2000, 256), [0x2000, 1, 256, 0]);
    }
}
