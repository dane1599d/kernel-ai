//! The bookkeeping of one queue pair: where the driver writes the next command,
//! where it reads the next completion, and which phase tag marks a new one. The
//! memory itself is the driver's.

/// A submission queue's tail, which the driver owns, and the head the controller
/// last reported, which says how much room is left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubmissionQueue {
    entries: u16,
    tail: u16,
    head: u16,
}

impl SubmissionQueue {
    pub fn new(entries: u16) -> Self {
        assert!(entries >= 2, "a queue needs two entries at least");
        Self {
            entries,
            tail: 0,
            head: 0,
        }
    }

    /// Whether one more command would fill the queue: one slot always stays empty.
    pub fn full(&self) -> bool {
        (self.tail + 1) % self.entries == self.head
    }

    /// The slot for the next command, and the tail to ring the doorbell with once
    /// it is written. None while the queue is full.
    pub fn push(&mut self) -> Option<(u16, u16)> {
        if self.full() {
            return None;
        }
        let slot = self.tail;
        self.tail = (self.tail + 1) % self.entries;
        Some((slot, self.tail))
    }

    /// Records the head a completion reported. A head outside the queue is
    /// ignored.
    pub fn advance_head(&mut self, head: u16) {
        if head < self.entries {
            self.head = head;
        }
    }

    pub fn tail(&self) -> u16 {
        self.tail
    }
}

/// A completion queue's head and the phase tag a new entry carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompletionQueue {
    entries: u16,
    head: u16,
    phase: bool,
}

impl CompletionQueue {
    /// A fresh queue: zeroed memory, so the first pass's entries carry phase 1.
    pub fn new(entries: u16) -> Self {
        assert!(entries >= 2, "a queue needs two entries at least");
        Self {
            entries,
            head: 0,
            phase: true,
        }
    }

    /// The slot to look at next.
    pub fn head(&self) -> u16 {
        self.head
    }

    /// Whether an entry with phase tag `phase` at the head is new.
    pub fn is_new(&self, phase: bool) -> bool {
        phase == self.phase
    }

    /// Consumes the head entry; returns the head to ring the doorbell with.
    pub fn pop(&mut self) -> u16 {
        self.head += 1;
        if self.head == self.entries {
            self.head = 0;
            self.phase = !self.phase;
        }
        self.head
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_submission_queue_keeps_one_slot_free() {
        let mut sq = SubmissionQueue::new(4);
        assert_eq!(sq.push(), Some((0, 1)));
        assert_eq!(sq.push(), Some((1, 2)));
        assert_eq!(sq.push(), Some((2, 3)));
        assert!(sq.full());
        assert_eq!(sq.push(), None);
        sq.advance_head(2);
        assert_eq!(sq.push(), Some((3, 0)));
        assert_eq!(sq.push(), Some((0, 1)));
        assert!(sq.full());
        sq.advance_head(9);
        assert!(sq.full());
        assert_eq!(sq.tail(), 1);
    }

    #[test]
    fn the_phase_flips_on_each_wrap() {
        let mut cq = CompletionQueue::new(3);
        assert!(cq.is_new(true));
        assert!(!cq.is_new(false));
        assert_eq!(cq.pop(), 1);
        assert_eq!(cq.pop(), 2);
        assert_eq!(cq.pop(), 0);
        // Second pass: the controller now writes phase 0.
        assert!(cq.is_new(false));
        assert!(!cq.is_new(true));
        assert_eq!(cq.head(), 0);
        for _ in 0..3 {
            cq.pop();
        }
        assert!(cq.is_new(true));
    }
}
