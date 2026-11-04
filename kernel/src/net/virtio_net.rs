//! Our own virtio-net driver (VirtIO 1.2, 5.1), on virtio-drivers' public
//! [`VirtQueue`]: two queues, with receive buffers added without a kick and
//! the device kicked once for each batch ([`VirtioNet::kick_receive`]).
//!
//! virtio-drivers' `VirtIONetRaw` kicks on every receive buffer given back.
//! With the ring nearly empty, as it is when the host sends faster than the
//! guest reads, QEMU asks for a kick each time, and each kick is a VM exit in
//! which it delivers the next frames itself: 50-62 µs of a frame's 52-98 µs
//! under KVM (P5-S2's profile). A batch of buffers and one kick lets one exit
//! fill them all.

use bitflags::bitflags;
use core::fmt;
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

pub struct VirtioNet<H: Hal, T: Transport, const SIZE: usize> {
    transport: T,
    rx: VirtQueue<H, SIZE>,
    tx: VirtQueue<H, SIZE>,
    mac: [u8; 6],
    /// Receive buffers and frames to send added since each queue's last kick.
    unkicked: usize,
    unkicked_tx: usize,
}

impl<H: Hal, T: Transport, const SIZE: usize> fmt::Debug for VirtioNet<H, T, SIZE> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VirtioNet").field("mac", &self.mac).finish()
    }
}

impl<H: Hal, T: Transport, const SIZE: usize> VirtioNet<H, T, SIZE> {
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
            unkicked: 0,
            unkicked_tx: 0,
        })
    }

    pub fn mac_address(&self) -> [u8; 6] {
        self.mac
    }

    pub fn disable_interrupts(&mut self) {
        self.rx.set_dev_notify(false);
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
        self.unkicked += 1;
        Ok(token)
    }

    /// Kicks the device for the receive buffers added since the last kick: one
    /// exit for the whole batch. How many there were. Always, when any were
    /// added: `VirtQueue::should_notify` compares the 16-bit ring indexes
    /// without their wrap (`avail_idx >= avail_event + 1`), so once a batch
    /// crosses 65,536 adds it says no, the device never hears, and receiving
    /// stops (P5-S2, run 41896431110).
    pub fn kick_receive(&mut self) -> usize {
        let added = core::mem::take(&mut self.unkicked);
        if added != 0 {
            self.transport.notify(RECEIVE);
        }
        added
    }

    /// The token of the next filled receive buffer, if any.
    pub fn poll_receive(&self) -> Option<u16> {
        self.rx.peek_used()
    }

    /// Takes back the receive buffer for `token`: the header's and the
    /// frame's lengths in it.
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
        Ok((HEADER, len.saturating_sub(HEADER)))
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
        self.unkicked_tx += 1;
        Ok(token)
    }

    /// Kicks the device for the frames queued since the last kick, always
    /// when there are any, for the wrap [`Self::kick_receive`] describes.
    pub fn kick_transmit(&mut self) -> usize {
        let added = core::mem::take(&mut self.unkicked_tx);
        if added != 0 {
            self.transport.notify(TRANSMIT);
        }
        added
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
