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

    /// Records the head a completion reported, if it is one the queue can have:
    /// from the old head up to the tail, going round (the controller has taken
    /// no more than was submitted and never un-takes). Any other head, outside
    /// the queue, behind the old one or past the tail, is ignored and the queue
    /// keeps the room it thought it had: `false`. With commands run one at a
    /// time the only right head is the tail, so a queue looks full only after
    /// reports like that ([`SubmissionQueue::full`] would be stale, not true).
    pub fn advance_head(&mut self, head: u16) -> bool {
        if head >= self.entries {
            return false;
        }
        // In u32: the sum overflows u16 for a queue of more than 32768 entries.
        let ahead = |to: u16| {
            (u32::from(to) + u32::from(self.entries) - u32::from(self.head))
                % u32::from(self.entries)
        };
        let inside = ahead(head) <= ahead(self.tail);
        if inside {
            self.head = head;
        }
        inside
    }

    pub fn tail(&self) -> u16 {
        self.tail
    }

    pub fn head(&self) -> u16 {
        self.head
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
        assert!(sq.advance_head(2));
        assert_eq!(sq.push(), Some((3, 0)));
        assert_eq!(sq.push(), Some((0, 1)));
        assert!(sq.full());
        assert!(!sq.advance_head(9));
        assert!(sq.full());
        assert_eq!((sq.tail(), sq.head()), (1, 2));
    }

    /// A head the controller cannot have: past the tail, or behind the head it
    /// reported before. The queue keeps its head, so it does not look full.
    #[test]
    fn a_head_past_the_tail_or_behind_the_head_is_ignored() {
        let mut sq = SubmissionQueue::new(8);
        for _ in 0..5 {
            sq.push().expect("room");
        }
        assert_eq!((sq.head(), sq.tail()), (0, 5));
        assert!(!sq.advance_head(6));
        assert!(!sq.advance_head(7));
        assert_eq!(sq.head(), 0);
        assert!(sq.advance_head(3));
        assert!(!sq.advance_head(2));
        assert!(!sq.advance_head(0));
        assert_eq!(sq.head(), 3);
        assert!(sq.advance_head(3));
        assert!(sq.advance_head(5));
        // Round the end: tail 1 after wrapping, head 5 may move to 7, 0 or 1.
        for _ in 0..4 {
            sq.push().expect("room");
        }
        assert_eq!((sq.head(), sq.tail()), (5, 1));
        assert!(!sq.advance_head(2));
        assert!(!sq.advance_head(4));
        assert!(sq.advance_head(7));
        assert!(sq.advance_head(0));
        assert!(sq.advance_head(1));
    }

    /// One command at a time, as the driver runs them: each completion reports
    /// the head just past its command, and the queue is never full between
    /// commands, however many wrap the queue.
    #[test]
    fn a_queue_run_one_command_at_a_time_never_fills() {
        let mut sq = SubmissionQueue::new(4);
        for _ in 0..100 {
            assert!(!sq.full());
            let (_, tail) = sq.push().expect("room");
            assert!(sq.advance_head(tail));
            assert_eq!(sq.head(), sq.tail());
        }
        // A controller that reports a head outside the queue never frees what
        // it holds: the queue fills after entries - 1 such commands.
        for _ in 0..3 {
            sq.push().expect("room");
            assert!(!sq.advance_head(u16::MAX));
        }
        assert!(sq.full());
        assert_eq!(sq.push(), None);
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

    /// A queue of more than 32768 entries: the distance sums stay in range.
    #[test]
    fn a_huge_queue_takes_a_head_near_its_end() {
        let mut sq = SubmissionQueue::new(65535);
        sq.head = 65000;
        sq.tail = 100;
        assert!(sq.advance_head(65500));
        assert!(sq.advance_head(50));
        assert!(!sq.advance_head(101));
        assert!(!sq.advance_head(65400));
    }
}
