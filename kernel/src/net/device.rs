//! smoltcp `Device` adapter over the virtio network card ([`super::virtio_net`]).
//!
//! - **Receive:** the card holds [`QUEUE_SIZE`] buffers. One handed back after
//!   its frame is read waits in a stash, and the stash goes back to the card in
//!   one batch with one kick after each poll ([`NicDevice::refill`]). A kick for
//!   each buffer cost a VM exit per frame under KVM, 50-62 µs of 52-98
//!   (spike P5-S2).
//! - **Send:** a frame is queued without waiting for the card and kicked with
//!   the rest of its poll's frames; sent buffers are reclaimed before each
//!   send.

use super::{BUFFER_LEN, Nic, QUEUE_SIZE};
use crate::log;
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, Ordering};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::time::Instant;

/// Ethernet frame size including the 14-byte header: a 1500-byte IP MTU.
const FRAME_MTU: usize = 1514;

/// Set once the card filled a receive buffer it was never given, so that is
/// logged once, not on every poll after.
static STALLED: AtomicBool = AtomicBool::new(false);

/// A received frame: its buffer, and where the frame lies in it.
type Received = (Vec<u8>, usize, usize);

struct Card {
    /// On the heap with the tables below: 256 slots each would be tens of
    /// kilobytes on the stack that builds them (lane C's review).
    nic: Box<Nic>,
    /// The receive buffers the card holds, by token.
    rx: Vec<Option<Vec<u8>>>,
    /// The frames the card is sending, header first, by token.
    tx: Vec<Option<Vec<u8>>>,
    /// Receive buffers handed back and not yet given to the card.
    stash: Vec<Vec<u8>>,
}

impl Card {
    /// Takes back every frame the card has sent.
    fn reclaim(&mut self) {
        while let Some(token) = self.nic.poll_transmit() {
            let Some(buffer) = self.tx.get_mut(usize::from(token)).and_then(Option::take) else {
                log!("net: the card finished a send it was never given");
                return;
            };
            // SAFETY: `buffer` is the one `transmit_begin` got for `token`.
            if let Err(error) = unsafe { self.nic.transmit_complete(token, &buffer) } {
                log!("net: transmit error {error:?}");
            }
        }
    }

    fn can_send(&mut self) -> bool {
        self.reclaim();
        self.nic.can_send()
    }

    /// Queues `frame` for the card, behind a zero header; the poll's kick sends it.
    fn transmit(&mut self, frame: &[u8]) {
        self.reclaim();
        let header = super::virtio_net::HEADER;
        let mut buffer = vec![0u8; header + frame.len()];
        buffer[header..].copy_from_slice(frame);
        // SAFETY: the buffer is kept in `tx` until the card gives its token back.
        match unsafe { self.nic.transmit_begin(&buffer) } {
            Ok(token) => self.tx[usize::from(token)] = Some(buffer),
            Err(error) => log!("net: transmit error {error:?}"),
        }
    }

    /// The next frame the card received.
    fn receive(&mut self) -> Option<Received> {
        let token = self.nic.poll_receive()?;
        let Some(mut buffer) = self.rx.get_mut(usize::from(token)).and_then(Option::take) else {
            // The device names a buffer it was never given: nothing can be
            // popped safely, so receiving stops, and the log says why.
            if !STALLED.swap(true, Ordering::Relaxed) {
                log!("net: the card filled receive buffer {token}, which it was never given");
            }
            return None;
        };
        // SAFETY: `buffer` is the one `add_receive` got for `token`.
        match unsafe { self.nic.receive_complete(token, &mut buffer) } {
            Ok((header, len)) => Some((buffer, header, len)),
            Err(error) => {
                log!("net: receive error {error:?}");
                self.stash.push(buffer);
                None
            }
        }
    }

    /// Gives the card every stashed receive buffer, then kicks it once for
    /// them and once for the frames queued to send.
    fn refill(&mut self) {
        while let Some(mut buffer) = self.stash.pop() {
            // SAFETY: the buffer is kept in `rx` until the card gives its token back.
            match unsafe { self.nic.add_receive(&mut buffer) } {
                Ok(token) => self.rx[usize::from(token)] = Some(buffer),
                Err(error) => {
                    log!("net: cannot give back a receive buffer: {error:?}");
                    self.stash.push(buffer);
                    break;
                }
            }
        }
        self.nic.kick_receive();
        self.nic.kick_transmit();
    }
}

pub struct NicDevice {
    card: RefCell<Card>,
}

impl NicDevice {
    /// The card with every receive buffer given to it.
    pub fn new(nic: Nic) -> Self {
        let mut card = Card {
            nic: Box::new(nic),
            rx: (0..QUEUE_SIZE).map(|_| None).collect(),
            tx: (0..QUEUE_SIZE).map(|_| None).collect(),
            stash: Vec::with_capacity(QUEUE_SIZE),
        };
        card.stash
            .extend((0..QUEUE_SIZE).map(|_| vec![0u8; BUFFER_LEN]));
        card.refill();
        Self {
            card: RefCell::new(card),
        }
    }

    pub fn mac(&self) -> [u8; 6] {
        self.card.borrow().nic.mac_address()
    }

    /// Gives the card back the receive buffers read since the last time and
    /// sends what was queued, with one kick each: after each poll.
    pub fn refill(&mut self) {
        self.card.get_mut().refill();
    }
}

pub struct NicRx<'a> {
    card: &'a RefCell<Card>,
    buffer: Option<Received>,
}

pub struct NicTx<'a> {
    card: &'a RefCell<Card>,
}

impl Device for NicDevice {
    type RxToken<'a>
        = NicRx<'a>
    where
        Self: 'a;
    type TxToken<'a>
        = NicTx<'a>
    where
        Self: 'a;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let buffer = self.card.get_mut().receive()?;
        Some((
            NicRx {
                card: &self.card,
                buffer: Some(buffer),
            },
            NicTx { card: &self.card },
        ))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        self.card
            .get_mut()
            .can_send()
            .then_some(NicTx { card: &self.card })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut capabilities = DeviceCapabilities::default();
        capabilities.medium = Medium::Ethernet;
        capabilities.max_transmission_unit = FRAME_MTU;
        // No burst: smoltcp caps every window it advertises at burst x MSS
        // (`iface/packet.rs`), and 1 held a download to one segment per round
        // trip, 0.1 MB/s (spike P5-S2). The card's queue is QEMU's to fill; a
        // frame it cannot take waits there, and TCP recovers any lost on a NIC.
        capabilities.max_burst_size = None;
        capabilities
    }
}

impl RxToken for NicRx<'_> {
    fn consume<R, F>(mut self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        let (buffer, header, len) = self.buffer.take().expect("receive token holds a buffer");
        // No RefCell borrow while `f` runs: smoltcp may transmit from inside it.
        let result = f(&buffer[header..header + len]);
        self.card.borrow_mut().stash.push(buffer);
        result
    }
}

impl Drop for NicRx<'_> {
    /// A token dropped unconsumed must still hand its buffer back.
    fn drop(&mut self) {
        if let Some((buffer, ..)) = self.buffer.take() {
            self.card.borrow_mut().stash.push(buffer);
        }
    }
}

impl TxToken for NicTx<'_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut frame = vec![0u8; len];
        let result = f(&mut frame);
        self.card.borrow_mut().transmit(&frame);
        result
    }
}
