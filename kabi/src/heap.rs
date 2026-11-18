//! What `user::heap` decides, apart from the system calls, so the host tests it:
//! which allocations get their own mapping, the table of those mappings, when
//! a heap growth is worth a log line, and a buffer to format that line in with
//! no allocation (the allocator's lock is held).
//!
//! talc's heap only grows (`brk`), and the kernel kills a process that grows it
//! past its page quota, with no error back. So an allocation of
//! [`BIG_BYTES`] or more takes its own reservation (MEM_RESERVE, MEM_COMMIT),
//! which goes back to the system when it is freed (MEM_RELEASE), and a commit
//! past the quota is NoMemory, not a kill: the program logs that allocation.

use core::fmt;

/// Allocations of this many bytes or more get their own mapping.
pub const BIG_BYTES: usize = 1 << 20;
/// A page.
pub const PAGE: u64 = 4096;
/// Big allocations at once; past this they come from the heap. The kernel
/// allows 64 reservations a process, and a program may make its own.
pub const BIG_SLOTS: usize = 32;
/// The heap logs the allocation that grows it past each multiple of this, so
/// a process killed over its quota leaves the last such line.
pub const MARK_BYTES: u64 = 16 << 20;

/// The bytes to reserve and commit for an allocation of `size` bytes aligned to
/// `align`, if it gets its own mapping: `size` rounded up to whole pages. A
/// reservation starts on a page, so an alignment beyond a page stays on the heap.
pub fn big_bytes(size: usize, align: usize) -> Option<u64> {
    if size < BIG_BYTES || align as u64 > PAGE {
        return None;
    }
    (size as u64).checked_next_multiple_of(PAGE)
}

/// The starts of the live big allocations.
#[derive(Debug)]
pub struct BigTable {
    starts: [u64; BIG_SLOTS],
}

impl BigTable {
    pub const fn new() -> Self {
        Self {
            starts: [0; BIG_SLOTS],
        }
    }

    /// Whether another big allocation fits.
    pub fn has_room(&self) -> bool {
        self.starts.contains(&0)
    }

    /// Records `start`; false, and nothing recorded, when the table is full.
    pub fn insert(&mut self, start: u64) -> bool {
        match self.starts.iter_mut().find(|slot| **slot == 0) {
            Some(slot) if start != 0 => {
                *slot = start;
                true
            }
            _ => false,
        }
    }

    /// Whether `start` is a big allocation's.
    pub fn contains(&self, start: u64) -> bool {
        start != 0 && self.starts.contains(&start)
    }

    /// Forgets `start`; whether it was there.
    pub fn remove(&mut self, start: u64) -> bool {
        match self
            .starts
            .iter_mut()
            .find(|slot| start != 0 && **slot == start)
        {
            Some(slot) => {
                *slot = 0;
                true
            }
            None => false,
        }
    }

    /// How many are live.
    pub fn len(&self) -> usize {
        self.starts.iter().filter(|&&start| start != 0).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for BigTable {
    fn default() -> Self {
        Self::new()
    }
}

/// The [`MARK_BYTES`] mark, in MiB, a heap growing from `old` to `new` bytes
/// passes, if any: the highest one when it passes several.
pub fn passed_mark(old: u64, new: u64) -> Option<u64> {
    let mark = new / MARK_BYTES;
    (mark > old / MARK_BYTES).then_some((mark * MARK_BYTES) >> 20)
}

/// A line formatted on the stack, cut short at its capacity.
pub struct LineBuf<const N: usize> {
    bytes: [u8; N],
    len: usize,
}

impl<const N: usize> LineBuf<N> {
    pub const fn new() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

impl<const N: usize> Default for LineBuf<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> fmt::Write for LineBuf<N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let room = N - self.len;
        let take = s.len().min(room);
        self.bytes[self.len..self.len + take].copy_from_slice(&s.as_bytes()[..take]);
        self.len += take;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::fmt::Write;

    #[test]
    fn a_mebibyte_or_more_with_at_most_page_alignment_gets_its_own_mapping() {
        assert_eq!(big_bytes(BIG_BYTES - 1, 8), None);
        assert_eq!(big_bytes(BIG_BYTES, 8), Some(BIG_BYTES as u64));
        assert_eq!(big_bytes(BIG_BYTES + 1, 64), Some(BIG_BYTES as u64 + PAGE));
        assert_eq!(big_bytes(BIG_BYTES, 4096), Some(BIG_BYTES as u64));
        assert_eq!(big_bytes(BIG_BYTES, 8192), None);
    }

    #[test]
    fn the_table_holds_its_slots_and_gives_them_back() {
        let mut table = BigTable::new();
        assert!(table.is_empty());
        for i in 1..=BIG_SLOTS as u64 {
            assert!(table.insert(i * PAGE));
        }
        assert!(!table.has_room());
        assert!(!table.insert(999 * PAGE), "a full table takes no more");
        assert!(table.contains(3 * PAGE));
        assert!(!table.contains(999 * PAGE));
        assert!(table.remove(3 * PAGE));
        assert!(!table.remove(3 * PAGE), "a start goes once");
        assert!(!table.contains(3 * PAGE));
        assert_eq!(table.len(), BIG_SLOTS - 1);
        assert!(table.insert(999 * PAGE));
        assert!(table.contains(999 * PAGE));
    }

    #[test]
    fn address_zero_is_never_a_big_allocation() {
        let mut table = BigTable::new();
        assert!(!table.insert(0));
        assert!(!table.contains(0));
        assert!(!table.remove(0));
        assert!(table.is_empty());
    }

    #[test]
    fn a_growth_logs_once_per_mark_it_passes() {
        let mib = 1 << 20;
        assert_eq!(passed_mark(0, 15 * mib), None);
        assert_eq!(passed_mark(15 * mib, 16 * mib), Some(16));
        assert_eq!(passed_mark(16 * mib, 17 * mib), None);
        assert_eq!(passed_mark(17 * mib, 50 * mib), Some(48));
    }

    #[test]
    fn a_line_is_cut_at_its_capacity() {
        let mut line = LineBuf::<8>::new();
        write!(line, "heap: {}", 123_456).unwrap();
        assert_eq!(line.as_bytes(), b"heap: 12");
        let mut line = LineBuf::<64>::new();
        write!(line, "a {}-byte allocation", 42).unwrap();
        assert_eq!(line.as_bytes(), b"a 42-byte allocation");
    }
}
