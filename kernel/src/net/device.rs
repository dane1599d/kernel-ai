//! smoltcp `Device` adapter over the virtio network card.
//!
//! Spike P5-S2 (`net-throughput`) only: [`set_tx_delay_ms`] holds each frame the
//! guest sends for that long before the card gets it, so every ACK comes late and
//! the sender sees the added round trip. slirp ends TCP inside QEMU, so a delay
//! on the host side would never reach the guest's window.

use super::Nic;
use crate::{hpet, log};
use alloc::collections::VecDeque;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::sync::atomic::{AtomicU64, Ordering};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::time::Instant;
use virtio_drivers::device::net::RxBuffer;

/// Ethernet frame size including the 14-byte header: a 1500-byte IP MTU.
const FRAME_MTU: usize = 1514;
/// Frames held at most while a delay is set; past it smoltcp waits to send.
const DELAYED_MAX: usize = 8192;

/// The added one-way delay on sent frames, in ns: 0 sends at once.
static TX_DELAY_NS: AtomicU64 = AtomicU64::new(0);
/// `max_burst_size` as smoltcp sees it: it caps every window this socket
/// advertises at that many segments. 0 is no cap. 1 is what ships today.
static BURST: AtomicU64 = AtomicU64::new(1);

/// Sets the burst smoltcp's window cap uses; 0 removes the cap. Only before
/// the interface is made: smoltcp reads it once then.
pub fn set_burst(frames: u64) {
    BURST.store(frames, Ordering::Release);
}

/// The burst the interface was made with.
pub fn burst() -> u64 {
    BURST.load(Ordering::Acquire)
}

/// Holds every frame sent from now on for `ms` before the card gets it.
pub fn set_tx_delay_ms(ms: u64) {
    TX_DELAY_NS.store(ms.saturating_mul(1_000_000), Ordering::Release);
}

/// A frame waiting for its time: when it may go, and its bytes.
type Delayed = VecDeque<(u64, Vec<u8>)>;

pub struct NicDevice {
    nic: RefCell<Nic>,
    delayed: RefCell<Delayed>,
}

impl NicDevice {
    pub fn new(nic: Nic) -> Self {
        Self {
            nic: RefCell::new(nic),
            delayed: RefCell::new(VecDeque::new()),
        }
    }

    pub fn mac(&self) -> [u8; 6] {
        self.nic.borrow().mac_address()
    }

    /// Gives the card every held frame whose time has come, in order.
    pub fn send_due(&mut self) {
        let now = hpet::nanos();
        let delayed = self.delayed.get_mut();
        let nic = self.nic.get_mut();
        while delayed.front().is_some_and(|(due, _)| *due <= now) && nic.can_send() {
            let Some((_, frame)) = delayed.pop_front() else {
                break;
            };
            let mut buffer = nic.new_tx_buffer(frame.len());
            buffer.packet_mut().copy_from_slice(&frame);
            if let Err(error) = nic.send(buffer) {
                log!("net: transmit error {error:?}");
            }
        }
    }
}

pub struct NicRx<'a> {
    nic: &'a RefCell<Nic>,
    buffer: Option<RxBuffer>,
}

pub struct NicTx<'a> {
    nic: &'a RefCell<Nic>,
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
        match self.nic.get_mut().receive() {
            Ok(buffer) => Some((
                NicRx {
                    nic: &self.nic,
                    buffer: Some(buffer),
                },
                NicTx {
                    nic: &self.nic,
                    delayed: &self.delayed,
                },
            )),
            Err(virtio_drivers::Error::NotReady) => None,
            Err(error) => {
                log!("net: receive error {error:?}");
                None
            }
        }
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        let held = self.delayed.get_mut().len();
        let ready = match TX_DELAY_NS.load(Ordering::Acquire) {
            0 if held == 0 => self.nic.get_mut().can_send(),
            _ => held < DELAYED_MAX,
        };
        ready.then_some(NicTx {
            nic: &self.nic,
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
        let buffer = self.buffer.take().expect("receive token holds a buffer");
        // No RefCell borrow while `f` runs: smoltcp may transmit from inside it.
        let result = f(buffer.packet());
        if let Err(error) = self.nic.borrow_mut().recycle_rx_buffer(buffer) {
            log!("net: cannot recycle a receive buffer: {error:?}");
        }
        result
    }
}

impl Drop for NicRx<'_> {
    /// A token dropped unconsumed must still give its buffer back to the card.
    fn drop(&mut self) {
        if let Some(buffer) = self.buffer.take() {
            let _ = self.nic.borrow_mut().recycle_rx_buffer(buffer);
        }
    }
}

impl TxToken for NicTx<'_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let delay = TX_DELAY_NS.load(Ordering::Acquire);
        if delay != 0 || !self.delayed.borrow().is_empty() {
            // Behind the held frames, so the order on the wire stays.
            let mut frame = vec![0u8; len];
            let result = f(&mut frame);
            let due = hpet::nanos().saturating_add(delay);
            self.delayed.borrow_mut().push_back((due, frame));
            return result;
        }
        let mut nic = self.nic.borrow_mut();
        let mut buffer = nic.new_tx_buffer(len);
        let result = f(buffer.packet_mut());
        // Sending waits for the card to take the frame; a dropped frame is retransmitted by TCP.
        if let Err(error) = nic.send(buffer) {
            log!("net: transmit error {error:?}");
        }
        result
    }
}
