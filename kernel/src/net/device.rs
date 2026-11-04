//! smoltcp `Device` adapter over the virtio network card, on virtio-drivers'
//! raw driver: the receive buffers are ours, and a frame sent is queued
//! without waiting for the card ([`Card::transmit`]). The card's queue is
//! reclaimed before each send.
//!
//! Spike P5-S2 (`net-throughput`) only:
//! - [`set_tx_delay_ms`] holds each frame the guest sends for that long before
//!   the card gets it, so every ACK comes late and the sender sees the added
//!   round trip. slirp ends TCP inside QEMU, so a delay on the host side would
//!   never reach the guest's window.
//! - `net_tx=sync` sends as virtio-drivers' `send` does: kick and wait for the
//!   card on every frame.

use super::prof::{self, Phase, Span};
use super::{BUFFER_LEN, Nic, QUEUE_SIZE};
use crate::{hpet, log};
use alloc::collections::VecDeque;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::time::Instant;

/// Ethernet frame size including the 14-byte header: a 1500-byte IP MTU.
const FRAME_MTU: usize = 1514;
/// Frames held at most while a delay is set; past it smoltcp waits to send.
const DELAYED_MAX: usize = 8192;

/// The added one-way delay on sent frames, in ns: 0 sends at once.
static TX_DELAY_NS: AtomicU64 = AtomicU64::new(0);
/// `max_burst_size` as smoltcp sees it: it caps every window this socket
/// advertises at that many segments. 0 is no cap.
static BURST: AtomicU64 = AtomicU64::new(0);
/// Send as virtio-drivers' `send` does, waiting for the card each time.
static SYNC_TX: AtomicBool = AtomicBool::new(false);

/// Sets the burst smoltcp's window cap uses; 0 removes the cap. Only before
/// the interface is made: smoltcp reads it once then.
pub fn set_burst(frames: u64) {
    BURST.store(frames, Ordering::Release);
}

/// The burst the interface was made with.
pub fn burst() -> u64 {
    BURST.load(Ordering::Acquire)
}

/// Sends wait for the card each time (`net_tx=sync`), or not.
pub fn set_sync_tx(sync: bool) {
    SYNC_TX.store(sync, Ordering::Release);
}

pub fn sync_tx() -> bool {
    SYNC_TX.load(Ordering::Acquire)
}

/// Holds every frame sent from now on for `ms` before the card gets it.
pub fn set_tx_delay_ms(ms: u64) {
    TX_DELAY_NS.store(ms.saturating_mul(1_000_000), Ordering::Release);
}

/// A frame waiting for its time: when it may go, and its bytes.
type Delayed = VecDeque<(u64, Vec<u8>)>;

/// A received frame: its buffer, and where the frame lies in it.
type Received = (Vec<u8>, usize, usize);

struct Card {
    nic: Nic,
    /// The receive buffers the card holds, by token.
    rx: [Option<Vec<u8>>; QUEUE_SIZE],
    /// The frames the card is sending, header first, by token.
    tx: [Option<Vec<u8>>; QUEUE_SIZE],
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

    /// Gives the card `frame`, without waiting for it, or with (`net_tx=sync`).
    fn transmit(&mut self, frame: &[u8]) {
        let _span = Span::start(Phase::CardTx);
        prof::TX_FRAMES.fetch_add(1, Ordering::Relaxed);
        if SYNC_TX.load(Ordering::Acquire) {
            if let Err(error) = self.nic.send(frame) {
                log!("net: transmit error {error:?}");
            }
            return;
        }
        self.reclaim();
        let mut buffer = vec![0u8; BUFFER_LEN];
        let header = match self.nic.fill_buffer_header(&mut buffer) {
            Ok(header) => header,
            Err(error) => {
                log!("net: transmit error {error:?}");
                return;
            }
        };
        buffer.truncate(header + frame.len());
        buffer[header..].copy_from_slice(frame);
        // SAFETY: the buffer is kept in `tx` until the card gives its token back.
        match unsafe { self.nic.transmit_begin(&buffer) } {
            Ok(token) => self.tx[usize::from(token)] = Some(buffer),
            Err(error) => log!("net: transmit error {error:?}"),
        }
    }

    /// The next frame the card received.
    fn receive(&mut self) -> Option<Received> {
        let _span = Span::start(Phase::CardRx);
        let token = self.nic.poll_receive()?;
        prof::RX_FRAMES.fetch_add(1, Ordering::Relaxed);
        let mut buffer = self.rx.get_mut(usize::from(token))?.take()?;
        // SAFETY: `buffer` is the one `receive_begin` got for `token`.
        match unsafe { self.nic.receive_complete(token, &mut buffer) } {
            Ok((header, len)) => Some((buffer, header, len)),
            Err(error) => {
                log!("net: receive error {error:?}");
                self.give_rx(buffer);
                None
            }
        }
    }

    /// Hands a receive buffer back to the card.
    fn give_rx(&mut self, mut buffer: Vec<u8>) {
        let _span = Span::start(Phase::CardRefill);
        // SAFETY: the buffer is kept in `rx` until the card gives its token back.
        match unsafe { self.nic.receive_begin(&mut buffer) } {
            Ok(token) => self.rx[usize::from(token)] = Some(buffer),
            Err(error) => log!("net: cannot give back a receive buffer: {error:?}"),
        }
    }
}

pub struct NicDevice {
    card: RefCell<Card>,
    delayed: RefCell<Delayed>,
}

impl NicDevice {
    /// The card with every receive buffer given to it.
    pub fn new(nic: Nic) -> Self {
        let mut card = Card {
            nic,
            rx: [const { None }; QUEUE_SIZE],
            tx: [const { None }; QUEUE_SIZE],
        };
        for _ in 0..QUEUE_SIZE {
            card.give_rx(vec![0u8; BUFFER_LEN]);
        }
        Self {
            card: RefCell::new(card),
            delayed: RefCell::new(VecDeque::new()),
        }
    }

    pub fn mac(&self) -> [u8; 6] {
        self.card.borrow().nic.mac_address()
    }

    /// Gives the card every held frame whose time has come, in order.
    pub fn send_due(&mut self) {
        let now = hpet::nanos();
        let delayed = self.delayed.get_mut();
        let card = self.card.get_mut();
        loop {
            // The queue's own work only: the send it releases is the card's.
            let span = Span::start(Phase::Delay);
            if !(delayed.front().is_some_and(|(due, _)| *due <= now) && card.can_send()) {
                break;
            }
            let Some((_, frame)) = delayed.pop_front() else {
                break;
            };
            drop(span);
            card.transmit(&frame);
        }
    }
}

pub struct NicRx<'a> {
    card: &'a RefCell<Card>,
    buffer: Option<Received>,
}

pub struct NicTx<'a> {
    card: &'a RefCell<Card>,
    delayed: &'a RefCell<Delayed>,
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
            NicTx {
                card: &self.card,
                delayed: &self.delayed,
            },
        ))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        let held = self.delayed.get_mut().len();
        let ready = match TX_DELAY_NS.load(Ordering::Acquire) {
            0 if held == 0 => self.card.get_mut().can_send(),
            _ => held < DELAYED_MAX,
        };
        ready.then_some(NicTx {
            card: &self.card,
            delayed: &self.delayed,
        })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut capabilities = DeviceCapabilities::default();
        capabilities.medium = Medium::Ethernet;
        capabilities.max_transmission_unit = FRAME_MTU;
        capabilities.max_burst_size = match BURST.load(Ordering::Acquire) {
            0 => None,
            frames => Some(frames as usize),
        };
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
        self.card.borrow_mut().give_rx(buffer);
        result
    }
}

impl Drop for NicRx<'_> {
    /// A token dropped unconsumed must still give its buffer back to the card.
    fn drop(&mut self) {
        if let Some((buffer, ..)) = self.buffer.take() {
            self.card.borrow_mut().give_rx(buffer);
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
        let delay = TX_DELAY_NS.load(Ordering::Acquire);
        if delay != 0 || !self.delayed.borrow().is_empty() {
            let _span = Span::start(Phase::Delay);
            // Behind the held frames, so the order on the wire stays.
            let due = hpet::nanos().saturating_add(delay);
            self.delayed.borrow_mut().push_back((due, frame));
            return result;
        }
        self.card.borrow_mut().transmit(&frame);
        result
    }
}
