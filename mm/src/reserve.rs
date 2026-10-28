//! Anonymous reservations in a user address space (MEM_RESERVE, MEM_COMMIT,
//! MEM_RELEASE): address space taken first, pages committed to it as it grows,
//! and all of it given back at once. A wasm instance's linear memory lives in
//! one, so suspending an app returns its pages (P3 task 11).
//!
//! The book is kept without allocating, in a fixed table, under the address
//! space's lock. Addresses come from a bump pointer in the region the layout
//! gives and are never reused, so a stale pointer into a released reservation
//! faults instead of reaching someone else's pages. A reservation may be longer
//! than it will ever be committed: the rest stays unmapped, which leaves room
//! for guard regions.

/// Reservations one address space may hold at once.
pub const MAX: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Entry {
    start: u64,
    pages: u64,
    /// Pages committed, from `start`.
    committed: u64,
    /// MEM_RELEASE has begun: no more commits.
    releasing: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// Zero pages, or more than the region holds.
    BadSize,
    /// The region's address space is used up, or the table is full.
    Full,
    /// No reservation starts at, or holds, that address.
    NotFound,
    /// A commit that would leave a hole: commits grow from the start.
    Hole,
    /// The reservation is being released.
    Releasing,
}

/// What a commit asks for: map `pages` fresh pages from `at`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Commit {
    pub at: u64,
    pub pages: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reservations {
    base: u64,
    end: u64,
    next: u64,
    entries: [Option<Entry>; MAX],
}

impl Reservations {
    /// An empty book over the region `[base, end)`, page-aligned.
    pub const fn new(base: u64, end: u64) -> Self {
        Self {
            base,
            end,
            next: base,
            entries: [None; MAX],
        }
    }

    /// Takes `pages` pages of address space; the reservation's start.
    pub fn reserve(&mut self, pages: u64) -> Result<u64, Refused> {
        if pages == 0 || pages > (self.end - self.base) / crate::PAGE_SIZE {
            return Err(Refused::BadSize);
        }
        let len = pages * crate::PAGE_SIZE;
        let start = self.next;
        let after = start
            .checked_add(len)
            .filter(|&e| e <= self.end)
            .ok_or(Refused::Full)?;
        let slot = self
            .entries
            .iter_mut()
            .find(|e| e.is_none())
            .ok_or(Refused::Full)?;
        *slot = Some(Entry {
            start,
            pages,
            committed: 0,
            releasing: false,
        });
        self.next = after;
        Ok(start)
    }

    fn holding(&mut self, addr: u64) -> Option<&mut Entry> {
        self.entries
            .iter_mut()
            .flatten()
            .find(|e| addr >= e.start && addr - e.start < e.pages * crate::PAGE_SIZE)
    }

    /// What committing `[addr, addr + pages)` needs mapped: the pages past what
    /// is committed already. A range that starts past the committed end leaves
    /// a hole and is refused; one that ends past the reservation too.
    pub fn plan_commit(&mut self, addr: u64, pages: u64) -> Result<Commit, Refused> {
        let entry = self.holding(addr).ok_or(Refused::NotFound)?;
        if entry.releasing {
            return Err(Refused::Releasing);
        }
        let first = (addr - entry.start) / crate::PAGE_SIZE;
        if first > entry.committed {
            return Err(Refused::Hole);
        }
        let last = first.checked_add(pages).ok_or(Refused::BadSize)?;
        if pages == 0 || last > entry.pages {
            return Err(Refused::BadSize);
        }
        let from = entry.committed.max(first);
        Ok(Commit {
            at: entry.start + from * crate::PAGE_SIZE,
            pages: last.saturating_sub(from),
        })
    }

    /// Records that the reservation holding `addr` now has `pages` more pages
    /// committed, as [`Reservations::plan_commit`] planned (or fewer, if
    /// mapping stopped early).
    pub fn committed(&mut self, addr: u64, pages: u64) {
        if let Some(entry) = self.holding(addr) {
            entry.committed = (entry.committed + pages).min(entry.pages);
        }
    }

    /// MEM_RELEASE begins on the reservation starting at `start`: no commit
    /// takes it from now. Its committed pages, to unmap.
    pub fn begin_release(&mut self, start: u64) -> Result<u64, Refused> {
        let entry = self
            .entries
            .iter_mut()
            .flatten()
            .find(|e| e.start == start)
            .ok_or(Refused::NotFound)?;
        if entry.releasing {
            return Err(Refused::Releasing);
        }
        entry.releasing = true;
        Ok(entry.committed)
    }

    /// MEM_RELEASE is done: the reservation leaves the book.
    pub fn end_release(&mut self, start: u64) {
        for entry in self.entries.iter_mut() {
            if entry.is_some_and(|e| e.start == start) {
                *entry = None;
            }
        }
    }

    /// Pages committed across every reservation.
    pub fn committed_pages(&self) -> u64 {
        self.entries.iter().flatten().map(|e| e.committed).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PAGE_SIZE as P;

    const BASE: u64 = 0x4000_0000_0000;

    fn book() -> Reservations {
        Reservations::new(BASE, BASE + 1024 * P)
    }

    #[test]
    fn reservations_take_fresh_address_space_and_never_reuse_it() {
        let mut book = book();
        let a = book.reserve(16).unwrap();
        let b = book.reserve(16).unwrap();
        assert_eq!((a, b), (BASE, BASE + 16 * P));
        book.begin_release(a).unwrap();
        book.end_release(a);
        assert_eq!(
            book.reserve(16),
            Ok(BASE + 32 * P),
            "released space is not reused"
        );
        assert_eq!(book.reserve(0), Err(Refused::BadSize));
        assert_eq!(book.reserve(2000), Err(Refused::BadSize));
        assert_eq!(book.reserve(1000), Err(Refused::Full));
    }

    #[test]
    fn commits_grow_from_the_start_without_holes() {
        let mut book = book();
        let a = book.reserve(8).unwrap();
        assert_eq!(book.plan_commit(a, 2), Ok(Commit { at: a, pages: 2 }));
        book.committed(a, 2);
        // Overlapping the committed part maps only what is new.
        assert_eq!(
            book.plan_commit(a, 3),
            Ok(Commit {
                at: a + 2 * P,
                pages: 1
            })
        );
        assert_eq!(
            book.plan_commit(a + 2 * P, 2),
            Ok(Commit {
                at: a + 2 * P,
                pages: 2
            })
        );
        assert_eq!(book.plan_commit(a + 4 * P, 1), Err(Refused::Hole));
        assert_eq!(book.plan_commit(a, 9), Err(Refused::BadSize));
        assert_eq!(book.plan_commit(a + 100 * P, 1), Err(Refused::NotFound));
        assert_eq!(book.committed_pages(), 2);
    }

    #[test]
    fn a_release_stops_commits_and_returns_what_was_committed() {
        let mut book = book();
        let a = book.reserve(8).unwrap();
        book.committed(a, 5);
        assert_eq!(book.begin_release(a), Ok(5));
        assert_eq!(book.plan_commit(a, 1), Err(Refused::Releasing));
        assert_eq!(book.begin_release(a), Err(Refused::Releasing));
        book.end_release(a);
        assert_eq!(book.begin_release(a), Err(Refused::NotFound));
        assert_eq!(book.committed_pages(), 0);
    }

    #[test]
    fn the_table_holds_at_most_max() {
        let mut book = Reservations::new(BASE, BASE + 10_000 * P);
        for _ in 0..MAX {
            book.reserve(1).unwrap();
        }
        assert_eq!(book.reserve(1), Err(Refused::Full));
    }
}
