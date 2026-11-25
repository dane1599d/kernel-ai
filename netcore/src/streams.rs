//! The stream table's sums, without a socket (P5 task 2; bug hunt #13): which
//! streams a process may still open, what becomes of a closed one, and the
//! most heap all of them can take. The kernel's `net::stream` keeps the table
//! and the smoltcp sockets, and asks these.
//!
//! A closed stream keeps its socket, and so its buffers, until its close is
//! done. So it counts against every limit as an open one does, and it has a
//! deadline: a peer that never sends its FIN cannot keep it for ever.

/// A stream the limits count: whose it is, and whether it is bulk. Open and
/// closing streams alike.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Held {
    pub owner: u64,
    pub bulk: bool,
}

/// How many streams there may be, open and closing together.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// One process's streams.
    pub per_process: usize,
    /// Streams in all.
    pub total: usize,
    /// Bulk streams in all, and one process's.
    pub bulk: usize,
    pub bulk_per_process: usize,
}

impl Limits {
    /// Whether `owner` may open one more stream, a `bulk` one or not, beside
    /// `held`: every stream open or still closing now.
    pub fn allow(&self, held: impl IntoIterator<Item = Held>, owner: u64, bulk: bool) -> bool {
        let (mut mine, mut total, mut bulks, mut my_bulks) = (0, 0, 0, 0);
        for stream in held {
            let own = stream.owner == owner;
            mine += usize::from(own);
            total += 1;
            bulks += usize::from(stream.bulk);
            my_bulks += usize::from(own && stream.bulk);
        }
        let bulk_room = bulks < self.bulk && my_bulks < self.bulk_per_process;
        mine < self.per_process && total < self.total && (!bulk || bulk_room)
    }

    /// The most bytes every stream's buffers can take at once: each of the
    /// `total` slots full with `rx` and `tx`, and `bulk` of them with
    /// `bulk_rx` in place of `rx`. Closing streams are in the slots too, so
    /// this is the whole of it.
    pub const fn worst_case(&self, rx: usize, bulk_rx: usize, tx: usize) -> usize {
        let bulk = if self.bulk < self.total {
            self.bulk
        } else {
            self.total
        };
        self.total * (rx + tx) + bulk * bulk_rx.saturating_sub(rx)
    }
}

/// What becomes of a closed stream's socket at a look.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Linger {
    /// Its close is done (CLOSED or TIME-WAIT): remove it.
    Done,
    /// Its deadline has passed with the close not done: abort it. The reset
    /// goes out on the next poll, and the socket, CLOSED by then, is removed.
    Abort,
    /// Still closing, within its time.
    Wait,
}

/// The deadline of a stream closed at `closed_ns`, which may linger `max_ms`.
pub const fn deadline(closed_ns: u64, max_ms: u64) -> u64 {
    closed_ns.saturating_add(max_ms.saturating_mul(1_000_000))
}

/// What becomes of a closed stream's socket at `now_ns`, given its
/// `deadline_ns` and whether its close is `done`.
pub fn linger(done: bool, deadline_ns: u64, now_ns: u64) -> Linger {
    if done {
        Linger::Done
    } else if now_ns >= deadline_ns {
        Linger::Abort
    } else {
        Linger::Wait
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    const LIMITS: Limits = Limits {
        per_process: 16,
        total: 32,
        bulk: 4,
        bulk_per_process: 2,
    };
    const KIB: usize = 1024;
    const MIB: usize = 1024 * KIB;

    fn plain(owner: u64, n: usize) -> Vec<Held> {
        vec![Held { owner, bulk: false }; n]
    }

    fn bulk(owner: u64, n: usize) -> Vec<Held> {
        vec![Held { owner, bulk: true }; n]
    }

    #[test]
    fn a_process_stops_at_its_own_limit() {
        assert!(LIMITS.allow(plain(1, 15), 1, false));
        assert!(!LIMITS.allow(plain(1, 16), 1, false));
        // Another process is not held back by it.
        assert!(LIMITS.allow(plain(1, 16), 2, false));
    }

    #[test]
    fn every_process_together_stops_at_the_total() {
        let mut held = plain(1, 16);
        held.extend(plain(2, 15));
        assert!(LIMITS.allow(held.clone(), 3, false));
        held.extend(plain(2, 1));
        assert!(!LIMITS.allow(held, 3, false));
    }

    #[test]
    fn bulk_streams_stop_at_two_a_process_and_four_in_all() {
        assert!(LIMITS.allow(bulk(1, 1), 1, true));
        assert!(!LIMITS.allow(bulk(1, 2), 1, true));
        // A plain stream is not a bulk one.
        assert!(LIMITS.allow(bulk(1, 2), 1, false));
        let mut held = bulk(1, 2);
        held.extend(bulk(2, 2));
        assert!(!LIMITS.allow(held.clone(), 3, true));
        assert!(LIMITS.allow(held, 3, false));
    }

    /// Bug hunt #13: the closed streams used to count for nothing, so 32
    /// closes, each still holding a bulk buffer, filled the heap. Every
    /// closing stream the kernel holds is in `held`, so the 33rd connect,
    /// and the 3rd bulk one, are Busy however many were closed.
    #[test]
    fn closing_streams_count_as_open_ones() {
        // One process: 2 bulk streams closed but not done, none open.
        let closing = bulk(1, 2);
        assert!(!LIMITS.allow(closing.clone(), 1, true));
        assert!(LIMITS.allow(closing, 1, false));
        // 16 closed and 16 open of two processes fill every slot.
        let mut held = plain(1, 16);
        held.extend(plain(2, 16));
        assert!(!LIMITS.allow(held, 3, false));
    }

    #[test]
    fn nothing_held_allows_any_stream() {
        assert!(LIMITS.allow([], 1, false));
        assert!(LIMITS.allow([], 1, true));
        let none = Limits {
            per_process: 0,
            ..LIMITS
        };
        assert!(!none.allow([], 1, false));
    }

    #[test]
    fn the_worst_case_is_every_slot_full_with_the_bulk_ones_larger() {
        // The kernel's figures: 32 x (256 KiB + 64 KiB) = 10 MiB, and 4 bulk
        // ones 768 KiB more each = 3 MiB.
        assert_eq!(LIMITS.worst_case(256 * KIB, MIB, 64 * KIB), 13 * MIB);
        // The old figures: 64 slots and no closing ones counted were 23 MiB
        // open, and 32 closing bulk ones 34 MiB more.
        let old = Limits {
            total: 64,
            ..LIMITS
        };
        assert_eq!(old.worst_case(256 * KIB, MIB, 64 * KIB), 23 * MIB);
        // More bulk streams than slots count only the slots.
        let odd = Limits {
            total: 2,
            bulk: 8,
            ..LIMITS
        };
        assert_eq!(odd.worst_case(KIB, 2 * KIB, 0), 4 * KIB);
        // A bulk buffer no larger than a plain one adds nothing.
        assert_eq!(LIMITS.worst_case(MIB, KIB, 0), 32 * MIB);
    }

    #[test]
    fn a_closed_stream_is_removed_once_done_and_aborted_past_its_deadline() {
        let closed = 7_000_000_000;
        let due = deadline(closed, 5_000);
        assert_eq!(due, 12_000_000_000);
        assert_eq!(linger(false, due, closed), Linger::Wait);
        assert_eq!(linger(false, due, due - 1), Linger::Wait);
        assert_eq!(linger(false, due, due), Linger::Abort);
        assert_eq!(linger(false, due, u64::MAX), Linger::Abort);
        // A close that is done is removed, in time or not.
        assert_eq!(linger(true, due, closed), Linger::Done);
        assert_eq!(linger(true, due, due + 1), Linger::Done);
    }

    #[test]
    fn a_deadline_saturates() {
        assert_eq!(deadline(u64::MAX - 1, 5_000), u64::MAX);
        assert_eq!(deadline(0, u64::MAX), u64::MAX);
        assert_eq!(deadline(0, 0), 0);
    }
}
