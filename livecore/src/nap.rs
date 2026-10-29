//! When an unused app naps (P3 task 12b, the founder's decision of
//! 2026-10-28): an app not focused, with no request and no input for
//! [`crate::settings::Settings::nap_ms`] (2 minutes), is suspended by its own
//! host, giving its memory back, and wakes on its next request or focus.
//! comphost keeps one [`Nap`] and asks it; the suspend and resume themselves
//! are task 11's (`ctl suspend`, `ctl resume`).
//!
//! The host wakes only what it napped. An app suspended by a standing
//! instruction or the agent stays suspended until that rule's `until`, or a
//! request, brings it back.

/// One app's use, as its host sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Nap {
    /// Unused time before a nap; 0 never naps.
    after_ms: u64,
    last_use_ms: u64,
    focused: bool,
    /// The keep-running switch: on, the app never naps.
    keep_running: bool,
    /// Background-work leases held (a download, a timer, audio).
    leases: u32,
    /// Whether this host suspended it, and so may wake it.
    napped: bool,
}

impl Nap {
    pub fn new(after_ms: u64, now_ms: u64) -> Self {
        Self {
            after_ms,
            last_use_ms: now_ms,
            focused: false,
            keep_running: false,
            leases: 0,
            napped: false,
        }
    }

    /// A request, or input to one of its windows.
    pub fn used(&mut self, now_ms: u64) {
        self.last_use_ms = now_ms;
    }

    /// One of its windows gained or lost focus; either counts as use.
    pub fn focus(&mut self, focused: bool, now_ms: u64) {
        self.focused = focused;
        self.last_use_ms = now_ms;
    }

    pub fn keep_running(&mut self, on: bool) {
        self.keep_running = on;
    }

    /// A background-work lease taken (`true`) or given back.
    pub fn lease(&mut self, taken: bool) {
        self.leases = if taken {
            self.leases.saturating_add(1)
        } else {
            self.leases.saturating_sub(1)
        };
    }

    /// Whether nothing keeps it awake: not napping already, not focused, no
    /// keep-running switch, no lease.
    fn may_nap(&self) -> bool {
        self.after_ms > 0 && !self.napped && !self.focused && !self.keep_running && self.leases == 0
    }

    /// Whether it should nap now.
    pub fn due(&self, now_ms: u64) -> bool {
        self.may_nap() && now_ms.saturating_sub(self.last_use_ms) >= self.after_ms
    }

    /// How long until it is due, for the host's wait; `None` when nothing
    /// will make it due without a change first.
    pub fn wait_ms(&self, now_ms: u64) -> Option<u64> {
        self.may_nap()
            .then(|| (self.last_use_ms + self.after_ms).saturating_sub(now_ms))
    }

    /// The host suspended it for being unused.
    pub fn napped(&mut self) {
        self.napped = true;
    }

    /// Whether this host's nap holds it, so its next use wakes it.
    pub fn is_napping(&self) -> bool {
        self.napped
    }

    /// It is awake again, from this nap or any other resume.
    pub fn woke(&mut self, now_ms: u64) {
        self.napped = false;
        self.last_use_ms = now_ms;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN2: u64 = 120_000;

    #[test]
    fn an_unused_unfocused_app_is_due_after_the_time_and_not_before() {
        let mut n = Nap::new(MIN2, 1_000);
        assert!(!n.due(1_000 + MIN2 - 1));
        assert_eq!(n.wait_ms(1_000), Some(MIN2));
        assert!(n.due(1_000 + MIN2));
        n.used(200_000);
        assert!(!n.due(200_000 + MIN2 - 1) && n.due(200_000 + MIN2));
    }

    #[test]
    fn focus_the_switch_and_a_lease_keep_it_awake() {
        let mut n = Nap::new(MIN2, 0);
        n.focus(true, 0);
        assert!(!n.due(10 * MIN2) && n.wait_ms(0).is_none());
        n.focus(false, 0);
        n.keep_running(true);
        assert!(!n.due(10 * MIN2));
        n.keep_running(false);
        n.lease(true);
        n.lease(true);
        n.lease(false);
        assert!(!n.due(10 * MIN2), "one lease is still held");
        n.lease(false);
        assert!(n.due(10 * MIN2));
        assert!(!Nap::new(0, 0).due(u64::MAX), "nap_ms=0 never naps");
    }

    #[test]
    fn only_its_own_nap_is_woken_and_it_naps_once() {
        let mut n = Nap::new(MIN2, 0);
        assert!(!n.is_napping());
        n.napped();
        assert!(n.is_napping() && !n.due(10 * MIN2));
        n.woke(5 * MIN2);
        assert!(!n.is_napping() && !n.due(5 * MIN2) && n.due(6 * MIN2));
    }
}
