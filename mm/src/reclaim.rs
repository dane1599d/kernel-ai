//! Swappable page-reclaim policy. The kernel owns the mechanism (tracking pages and
//! reading accessed bits); a policy only decides which pages to give up.
//! The LLM will later choose between policies; it never runs in this path.

use alloc::vec::Vec;

pub trait ReclaimPolicy {
    fn name(&self) -> &'static str;

    /// Starts tracking a reclaimable page (identified by its virtual address).
    fn track(&mut self, page: u64);

    /// Stops tracking a page, for example because it was unmapped.
    fn untrack(&mut self, page: u64);

    fn tracked(&self) -> usize;

    /// Appends up to `want` victims to `out` and stops tracking them.
    /// `accessed(page)` reports whether the page was used since the last check and
    /// clears that information, like testing and clearing a page-table accessed bit.
    fn select(&mut self, want: usize, accessed: &mut dyn FnMut(u64) -> bool, out: &mut Vec<u64>);
}

/// The CLOCK (second-chance) algorithm: sweep a ring of pages, give recently used
/// pages a second chance, and reclaim the first page found unused.
#[derive(Default)]
pub struct Clock {
    pages: Vec<u64>,
    hand: usize,
}

impl Clock {
    pub fn new() -> Self {
        Self::default()
    }
}

impl ReclaimPolicy for Clock {
    fn name(&self) -> &'static str {
        "clock"
    }

    fn track(&mut self, page: u64) {
        if !self.pages.contains(&page) {
            self.pages.push(page);
        }
    }

    fn untrack(&mut self, page: u64) {
        if let Some(index) = self.pages.iter().position(|&p| p == page) {
            self.pages.remove(index);
            if index < self.hand {
                self.hand -= 1;
            }
            if self.hand >= self.pages.len() {
                self.hand = 0;
            }
        }
    }

    fn tracked(&self) -> usize {
        self.pages.len()
    }

    fn select(&mut self, want: usize, accessed: &mut dyn FnMut(u64) -> bool, out: &mut Vec<u64>) {
        let mut taken = 0;
        // Two full sweeps are enough: the first clears every accessed bit it passes.
        let mut budget = self.pages.len() * 2;
        while taken < want && !self.pages.is_empty() && budget > 0 {
            budget -= 1;
            if self.hand >= self.pages.len() {
                self.hand = 0;
            }
            let page = self.pages[self.hand];
            if accessed(page) {
                self.hand += 1;
            } else {
                self.pages.remove(self.hand);
                out.push(page);
                taken += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeSet;
    use alloc::vec;

    /// Test double for page-table accessed bits.
    struct Bits(BTreeSet<u64>);

    impl Bits {
        fn check(&mut self, page: u64) -> bool {
            self.0.remove(&page)
        }
    }

    fn clock_with(pages: &[u64]) -> Clock {
        let mut clock = Clock::new();
        pages.iter().for_each(|&p| clock.track(p));
        clock
    }

    #[test]
    fn reclaims_unused_pages_first() {
        let mut clock = clock_with(&[0, 1, 2, 3, 4, 5]);
        let mut bits = Bits([0, 2, 4].into_iter().collect());
        let mut out = vec![];
        clock.select(3, &mut |p| bits.check(p), &mut out);
        assert_eq!(out, vec![1, 3, 5]);
        assert_eq!(clock.tracked(), 3);
    }

    #[test]
    fn gives_used_pages_a_second_chance() {
        let mut clock = clock_with(&[10, 20]);
        let mut bits = Bits([10, 20].into_iter().collect());
        let mut out = vec![];
        clock.select(1, &mut |p| bits.check(p), &mut out);
        // Both were used once; the first sweep clears them, the second reclaims one.
        assert_eq!(out.len(), 1);
        assert_eq!(clock.tracked(), 1);
    }

    #[test]
    fn untrack_and_duplicates_keep_ring_consistent() {
        let mut clock = clock_with(&[1, 2, 3, 3]);
        assert_eq!(clock.tracked(), 3);
        clock.untrack(2);
        clock.untrack(99);
        let mut out = vec![];
        clock.select(10, &mut |_| false, &mut out);
        assert_eq!(out, vec![1, 3]);
        assert_eq!(clock.tracked(), 0);
    }

    #[test]
    fn empty_ring_selects_nothing() {
        let mut clock = Clock::new();
        let mut out = vec![];
        clock.select(4, &mut |_| true, &mut out);
        assert!(out.is_empty());
        assert_eq!(clock.name(), "clock");
    }
}
