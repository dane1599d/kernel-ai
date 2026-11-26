//! smoltcp's `Device` over the card, as the kernel's `net::device` is, but with
//! every buffer in the DMA arena: a received frame is read where the card put
//! it, and a frame to send is written in place behind its header. The same
//! batching: receive buffers go back in one batch with one kick after each
//! poll, and frames to send are kicked with the rest of their poll's.

use crate::{BUFFER_LEN, Nic, QUEUE_SIZE, hal, nic};
use alloc::vec::Vec;
use core::cell::RefCell;
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::time::Instant;
use user::println;

const FRAME_MTU: usize = 1514;

type Buffer = &'static mut [u8];

struct Card {
    nic: Nic,
    /// The receive buffers the card holds, by token.
    rx: Vec<Option<Buffer>>,
    /// The frames the card is sending, and their lengths, by token.
    tx: Vec<Option<(Buffer, usize)>>,
    /// Receive buffers read and not yet given back.
    stash: Vec<Buffer>,
    /// Send buffers free.
    free: Vec<Buffer>,
}

impl Card {
    fn reclaim(&mut self) {
        while let Some(token) = self.nic.poll_transmit() {
            let Some((buffer, len)) = self.tx.get_mut(usize::from(token)).and_then(Option::take)
            else {
                println!("netd: the card finished a send it was never given");
                return;
            };
            // SAFETY: the buffer and length `transmit_begin` got for `token`.
            if let Err(error) = unsafe { self.nic.transmit_complete(token, &buffer[..len]) } {
                println!("netd: transmit error {error:?}");
            }
            self.free.push(buffer);
        }
    }

    fn receive(&mut self) -> Option<(Buffer, usize, usize)> {
        let token = self.nic.poll_receive()?;
        let Some(buffer) = self.rx.get_mut(usize::from(token)).and_then(Option::take) else {
            println!("netd: the card filled receive buffer {token}, which it was never given");
            return None;
        };
        // SAFETY: the buffer `add_receive` got for `token`.
        match unsafe { self.nic.receive_complete(token, &mut *buffer) } {
            Ok((header, len)) => Some((buffer, header, len)),
            Err(error) => {
                println!("netd: receive error {error:?}");
                self.stash.push(buffer);
                None
            }
        }
    }

    fn refill(&mut self) {
        while let Some(buffer) = self.stash.pop() {
            // SAFETY: the buffer is kept in `rx` until its token comes back.
            match unsafe { self.nic.add_receive(&mut *buffer) } {
                Ok(token) => self.rx[usize::from(token)] = Some(buffer),
                Err(error) => {
                    println!("netd: cannot give back a receive buffer: {error:?}");
                    self.stash.push(buffer);
                    break;
                }
            }
        }
        self.nic.kick_receive();
        self.nic.kick_transmit();
    }
}

pub struct CardDevice {
    card: RefCell<Card>,
}

impl CardDevice {
    pub fn new(nic: Nic) -> Self {
        let mut card = Card {
            nic,
            rx: (0..QUEUE_SIZE).map(|_| None).collect(),
            tx: (0..QUEUE_SIZE).map(|_| None).collect(),
            stash: (0..QUEUE_SIZE).map(|_| hal::buffer(BUFFER_LEN)).collect(),
            free: (0..QUEUE_SIZE).map(|_| hal::buffer(BUFFER_LEN)).collect(),
        };
        card.refill();
        Self {
            card: RefCell::new(card),
        }
    }

    pub fn mac(&self) -> [u8; 6] {
        self.card.borrow().nic.mac_address()
    }

    pub fn refill(&mut self) {
        self.card.get_mut().refill();
    }
}

pub struct CardRx<'a> {
    card: &'a RefCell<Card>,
    buffer: Option<(Buffer, usize, usize)>,
}

pub struct CardTx<'a> {
    card: &'a RefCell<Card>,
}

impl Device for CardDevice {
    type RxToken<'a>
        = CardRx<'a>
    where
        Self: 'a;
    type TxToken<'a>
        = CardTx<'a>
    where
        Self: 'a;

    fn receive(&mut self, _: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let buffer = self.card.get_mut().receive()?;
        Some((
            CardRx {
                card: &self.card,
                buffer: Some(buffer),
            },
            CardTx { card: &self.card },
        ))
    }

    fn transmit(&mut self, _: Instant) -> Option<Self::TxToken<'_>> {
        let card = self.card.get_mut();
        card.reclaim();
        (!card.free.is_empty() && card.nic.can_send()).then_some(CardTx { card: &self.card })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut capabilities = DeviceCapabilities::default();
        capabilities.medium = Medium::Ethernet;
        capabilities.max_transmission_unit = FRAME_MTU;
        capabilities.max_burst_size = None;
        capabilities
    }
}

impl RxToken for CardRx<'_> {
    fn consume<R, F>(mut self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        let (buffer, header, len) = self.buffer.take().expect("a receive token holds a buffer");
        let result = f(&buffer[header..header + len]);
        self.card.borrow_mut().stash.push(buffer);
        result
    }
}

impl Drop for CardRx<'_> {
    fn drop(&mut self) {
        if let Some((buffer, ..)) = self.buffer.take() {
            self.card.borrow_mut().stash.push(buffer);
        }
    }
}

impl TxToken for CardTx<'_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let header = nic::HEADER;
        let mut card = self.card.borrow_mut();
        card.reclaim();
        let Some(buffer) = card.free.pop() else {
            drop(card);
            // No buffer after all: the frame is lost, as on a full NIC.
            let mut scratch = alloc::vec![0u8; len];
            return f(&mut scratch);
        };
        drop(card);
        let total = header + len;
        buffer[..header].fill(0);
        let result = f(&mut buffer[header..total]);
        let mut card = self.card.borrow_mut();
        // SAFETY: the buffer is kept in `tx` until the card gives its token back.
        match unsafe { card.nic.transmit_begin(&buffer[..total]) } {
            Ok(token) => card.tx[usize::from(token)] = Some((buffer, total)),
            Err(error) => {
                println!("netd: transmit error {error:?}");
                card.free.push(buffer);
            }
        }
        result
    }
}
