//! Our own virtio-net driver (VirtIO 1.2, 5.1), on virtio-drivers' public
//! [`VirtQueue`]: two queues, with receive buffers added without a kick and
//! each batch checked once ([`VirtioNet::kick_receive`]): with
//! VIRTIO_F_EVENT_IDX the device is kicked only when its `avail_event` asks,
//! and without it on every batch.
//!
//! virtio-drivers' `VirtIONetRaw` kicks on every receive buffer given back.
//! With the ring nearly empty, as it is when the host sends faster than the
//! guest reads, QEMU asks for a kick each time, and each kick is a VM exit in
//! which it delivers the next frames itself: 50-62 µs of a frame's 52-98 µs
//! under KVM (P5-S2's profile). A batch of buffers and one kick lets one exit
//! fill them all.
//!
//! Even one kick a batch was a VM exit for every poll that handed a buffer
//! back: 331 k of them per GiB, about 46 µs each, 92% of a download's time
//! with the TSC as the clock (G67). So with VIRTIO_F_EVENT_IDX a batch is
//! kicked only when the device asked to hear of it, read from its
//! `avail_event` in wrapping arithmetic ([`netcore::kick::need_event`]), which
//! `VirtQueue::should_notify` gets wrong at the 16-bit wrap.

use crate::virtio::split::SplitTransport;
use bitflags::bitflags;
use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering, fence};
use virtio_drivers::queue::VirtQueue;
use virtio_drivers::transport::Transport;
use virtio_drivers::{Error, Hal};

bitflags! {
    /// The feature bits this driver asks for (VirtIO 1.2, 5.1.3, 6).
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    struct Features: u64 {
        const MAC = 1 << 5;
        const STATUS = 1 << 16;
        const RING_INDIRECT_DESC = 1 << 28;
        const RING_EVENT_IDX = 1 << 29;
        const VERSION_1 = 1 << 32;
    }
}

const RECEIVE: u16 = 0;
const TRANSMIT: u16 = 1;
/// `virtio_net_hdr` with VERSION_1: 12 bytes, all zero on a send.
pub const HEADER: usize = 12;

/// Kicks of the receive and the transmit queue since boot.
static RX_KICKS: AtomicU64 = AtomicU64::new(0);
static TX_KICKS: AtomicU64 = AtomicU64::new(0);

/// How many times the card was kicked for receive buffers and for frames to
/// send since boot.
pub fn kicks() -> (u64, u64) {
    (
        RX_KICKS.load(Ordering::Relaxed),
        TX_KICKS.load(Ordering::Relaxed),
    )
}

/// A transport that can read a queue's published available index and the
/// device's `avail_event` for it.
pub trait EventIndex {
    fn avail_idx(&self, queue: u16) -> Option<u16>;
    fn avail_event(&self, queue: u16) -> Option<u16>;
}

impl EventIndex for SplitTransport {
    fn avail_idx(&self, queue: u16) -> Option<u16> {
        SplitTransport::avail_idx(self, queue)
    }

    fn avail_event(&self, queue: u16) -> Option<u16> {
        SplitTransport::avail_event(self, queue)
    }
}

/// One queue's kick check.
#[derive(Default)]
struct Kick {
    /// Buffers added since the last check.
    added: usize,
    /// The published available index at the last check.
    checked: u16,
}

pub struct VirtioNet<H: Hal, T: Transport, const SIZE: usize> {
    transport: T,
    rx: VirtQueue<H, SIZE>,
    tx: VirtQueue<H, SIZE>,
    mac: [u8; 6],
    /// Whether the device takes VIRTIO_F_EVENT_IDX.
    event_idx: bool,
    rx_kick: Kick,
    tx_kick: Kick,
}

impl<H: Hal, T: Transport, const SIZE: usize> fmt::Debug for VirtioNet<H, T, SIZE> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VirtioNet").field("mac", &self.mac).finish()
    }
}

impl<H: Hal, T: Transport + EventIndex, const SIZE: usize> VirtioNet<H, T, SIZE> {
    /// Negotiates VERSION_1 and MAC, and sets up both queues; Unsupported when
    /// the device lacks either.
    pub fn new(mut transport: T) -> Result<Self, Error> {
        let wanted = Features::MAC
            | Features::STATUS
            | Features::RING_INDIRECT_DESC
            | Features::RING_EVENT_IDX
            | Features::VERSION_1;
        let features = transport.begin_init(wanted);
        if !features.contains(Features::VERSION_1 | Features::MAC) {
            return Err(Error::Unsupported);
        }
        // The MAC is the first six bytes of the device's configuration.
        let mac: [u8; 6] = transport.read_consistent(|| transport.read_config_space(0))?;
        let indirect = features.contains(Features::RING_INDIRECT_DESC);
        let event_idx = features.contains(Features::RING_EVENT_IDX);
        let rx = VirtQueue::new(&mut transport, RECEIVE, indirect, event_idx)?;
        let tx = VirtQueue::new(&mut transport, TRANSMIT, indirect, event_idx)?;
        transport.finish_init();
        Ok(Self {
            transport,
            rx,
            tx,
            mac,
            event_idx,
            rx_kick: Kick::default(),
            tx_kick: Kick::default(),
        })
    }

    /// Whether the device took VIRTIO_F_EVENT_IDX.
    pub fn event_idx(&self) -> bool {
        self.event_idx
    }

    pub fn mac_address(&self) -> [u8; 6] {
        self.mac
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    pub fn disable_interrupts(&mut self) {
        self.rx.set_dev_notify(false);
        self.tx.set_dev_notify(false);
    }

    /// Asks for an interrupt on received frames only. With EVENT_IDX the
    /// device signals once a frame passes the last one taken back, which
    /// `pop_used` keeps up to date; the flag is for a device without it.
    pub fn receive_interrupts(&mut self) {
        self.rx.set_dev_notify(true);
        self.tx.set_dev_notify(false);
    }

    /// Gives the device `buffer` to receive into, without a kick: its token.
    ///
    /// # Safety
    /// `buffer` must not be touched, moved or freed until its token comes back
    /// from [`Self::poll_receive`] and [`Self::receive_complete`] took it.
    pub unsafe fn add_receive(&mut self, buffer: &mut [u8]) -> Result<u16, Error> {
        // SAFETY: the caller keeps `buffer` for the device until it is popped.
        let token = unsafe { self.rx.add(&[], &mut [buffer])? };
        self.rx_kick.added += 1;
        Ok(token)
    }

    /// Kicks the device for the receive buffers added since the last check,
    /// one exit for the whole batch, when it asked to hear of one of them
    /// ([`notify`]). How many there were.
    pub fn kick_receive(&mut self) -> usize {
        let event_idx = self.event_idx;
        notify::<T, SIZE>(&mut self.transport, &mut self.rx_kick, RECEIVE, event_idx)
    }

    /// The token of the next filled receive buffer, if any. The fence orders
    /// the last `pop_used`'s store of `used_event` before this load of the used
    /// index (x86 may reorder a store with a later load), so a frame the
    /// device used meanwhile is seen here or raises the interrupt, never
    /// neither (VirtIO 1.2, 2.7.10).
    pub fn poll_receive(&self) -> Option<u16> {
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        self.rx.peek_used()
    }

    /// Takes back the receive buffer for `token`: the header's and the
    /// frame's lengths in it. The length is the device's word, so one that does
    /// not fit `buffer`, or leaves no frame past the header, is an error and
    /// the frame is dropped (lane C's review).
    ///
    /// # Safety
    /// `buffer` must be the one [`Self::add_receive`] got for `token`.
    pub unsafe fn receive_complete(
        &mut self,
        token: u16,
        buffer: &mut [u8],
    ) -> Result<(usize, usize), Error> {
        // SAFETY: the caller passes the same buffer the token was given for.
        let len = unsafe { self.rx.pop_used(token, &[], &mut [buffer])? } as usize;
        if len <= HEADER || len > buffer.len() {
            return Err(Error::IoError);
        }
        Ok((HEADER, len - HEADER))
    }

    /// Whether a frame can be sent now.
    pub fn can_send(&self) -> bool {
        self.tx.available_desc() >= 1
    }

    /// Gives the device `buffer` (a zero header, then the frame) to send,
    /// without a kick: [`Self::kick_transmit`] sends the batch. Its token.
    ///
    /// # Safety
    /// `buffer` must not be touched, moved or freed until its token comes back
    /// from [`Self::poll_transmit`] and [`Self::transmit_complete`] took it.
    pub unsafe fn transmit_begin(&mut self, buffer: &[u8]) -> Result<u16, Error> {
        // SAFETY: the caller keeps `buffer` for the device until it is popped.
        let token = unsafe { self.tx.add(&[buffer], &mut [])? };
        self.tx_kick.added += 1;
        Ok(token)
    }

    /// Kicks the device for the frames queued since the last check, as
    /// [`Self::kick_receive`] does.
    pub fn kick_transmit(&mut self) -> usize {
        let event_idx = self.event_idx;
        notify::<T, SIZE>(&mut self.transport, &mut self.tx_kick, TRANSMIT, event_idx)
    }

    /// The token of the next sent buffer, if any.
    pub fn poll_transmit(&self) -> Option<u16> {
        self.tx.peek_used()
    }

    /// Takes back the sent buffer for `token`.
    ///
    /// # Safety
    /// `buffer` must be the one [`Self::transmit_begin`] got for `token`.
    pub unsafe fn transmit_complete(&mut self, token: u16, buffer: &[u8]) -> Result<(), Error> {
        // SAFETY: the caller passes the same buffer the token was given for.
        unsafe { self.tx.pop_used(token, &[buffer], &mut [])? };
        Ok(())
    }
}

/// Notifies the device of `queue`'s buffers added since the last check when
/// it asked to hear of one of them, or always without EVENT_IDX, its
/// `avail_event` or the published index. How many were added.
///
/// The range is the published index's, read from the ring, so a buffer added
/// some other way than `add_receive` or `transmit_begin` is in it too, but
/// only in a batch that also had an add through one of those: with none,
/// `added` is 0 and the ring is not read, so such a buffer waits for the
/// next batch that has one (#1633's review). Every buffer the driver adds
/// goes through those two today.
/// [`netcore::kick::need_event`] holds for under 65,536 adds between two
/// checks; more than a queue's `SIZE` cannot come from one poll's refill, so
/// that many kick without the check (#1623's review).
///
/// The fence orders `VirtQueue::add`'s store of the available index before
/// the load of `avail_event`. The device stores `avail_event`, then reads the
/// available index again before it waits (VirtIO 1.2, 2.7.10), so either it
/// sees these buffers or this sees its word, never neither.
fn notify<T: Transport + EventIndex, const SIZE: usize>(
    transport: &mut T,
    kick: &mut Kick,
    queue: u16,
    event_idx: bool,
) -> usize {
    let added = core::mem::take(&mut kick.added);
    if added == 0 {
        return 0;
    }
    let Some(new) = transport.avail_idx(queue) else {
        transport.notify(queue);
        count(queue);
        return added;
    };
    let old = core::mem::replace(&mut kick.checked, new);
    fence(Ordering::SeqCst);
    let asked = match transport.avail_event(queue) {
        _ if added > SIZE => true,
        Some(event) if event_idx => netcore::kick::need_event(event, new, old),
        _ => true,
    };
    if asked {
        transport.notify(queue);
        count(queue);
    }
    added
}

fn count(queue: u16) {
    match queue {
        RECEIVE => &RX_KICKS,
        _ => &TX_KICKS,
    }
    .fetch_add(1, Ordering::Relaxed);
}
