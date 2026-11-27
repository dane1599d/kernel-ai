//! When a virtio driver must notify the device of buffers it made available,
//! under VIRTIO_F_EVENT_IDX (VirtIO 1.2, 2.7.10): the device writes
//! `avail_event`, the available index it wants to hear of, and the driver
//! notifies only when the buffers it added since its last check pass it.
//!
//! virtio-drivers 0.13's `VirtQueue::should_notify` compares the 16-bit
//! indexes as plain numbers (`avail_idx >= avail_event + 1`), which goes wrong
//! once the index wraps at 65,536: a needed notify is skipped and receiving
//! stops (P5-S2, run 41896431110). So the network driver kicked on every batch,
//! a VM exit each under KVM, 92% of a download's time on the TSC (G67).

/// Whether the device asked to hear of one of the buffers added since the
/// driver's last check: `avail_event`, its word, lies in `old..new`, the
/// available indexes then and now. Linux's `vring_need_event`, in wrapping
/// 16-bit arithmetic, so it holds across the wrap.
pub fn need_event(avail_event: u16, new: u16, old: u16) -> bool {
    new.wrapping_sub(avail_event).wrapping_sub(1) < new.wrapping_sub(old)
}

#[cfg(test)]
mod tests {
    use super::need_event;

    /// What virtio-drivers 0.13's `should_notify` decides.
    fn plain(avail_event: u16, new: u16) -> bool {
        new >= avail_event.wrapping_add(1)
    }

    #[test]
    fn a_device_waiting_inside_the_batch_is_notified() {
        // The device stopped at 10 and waits for index 10: adding 10..14 notifies.
        assert!(need_event(10, 14, 10));
        // It waits at the batch's last buffer.
        assert!(need_event(13, 14, 10));
        // The first batch: the device waits at 0.
        assert!(need_event(0, 256, 0));
    }

    #[test]
    fn a_device_still_working_is_not_notified() {
        // The device asked at 5, before this batch: it has not run dry since,
        // so the buffers added from 10 need no exit.
        assert!(!need_event(5, 14, 10));
        // It waits past what was added.
        assert!(!need_event(14, 14, 10));
        // Nothing added.
        assert!(!need_event(10, 10, 10));
    }

    #[test]
    fn a_batch_across_the_wrap_still_notifies_a_waiting_device() {
        // The batch is 65,530..65,540, that is 65,530..4 after the wrap, and
        // the device waits at 65,534 or at 2.
        assert!(need_event(65_534, 4, 65_530));
        assert!(need_event(2, 4, 65_530));
        assert!(!need_event(65_000, 4, 65_530));
        // The plain comparison says no to the first, and the device never
        // hears: receiving stops.
        assert!(!plain(65_534, 4));
    }
}
