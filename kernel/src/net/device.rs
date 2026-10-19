//! smoltcp `Device` adapter over the virtio network card.

use super::Nic;
use crate::log;
use core::cell::RefCell;
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::time::Instant;
use virtio_drivers::device::net::RxBuffer;

/// Ethernet frame size including the 14-byte header: a 1500-byte IP MTU.
const FRAME_MTU: usize = 1514;

pub struct NicDevice {
    nic: RefCell<Nic>,
}

impl NicDevice {
    pub fn new(nic: Nic) -> Self {
        Self {
            nic: RefCell::new(nic),
        }
    }

    pub fn mac(&self) -> [u8; 6] {
        self.nic.borrow().mac_address()
    }
}

pub struct NicRx<'a> {
    nic: &'a RefCell<Nic>,
    buffer: Option<RxBuffer>,
}

pub struct NicTx<'a> {
    nic: &'a RefCell<Nic>,
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
                NicTx { nic: &self.nic },
            )),
            Err(virtio_drivers::Error::NotReady) => None,
            Err(error) => {
                log!("net: receive error {error:?}");
                None
            }
        }
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        self.nic
            .get_mut()
            .can_send()
            .then_some(NicTx { nic: &self.nic })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut capabilities = DeviceCapabilities::default();
        capabilities.medium = Medium::Ethernet;
        capabilities.max_transmission_unit = FRAME_MTU;
        capabilities.max_burst_size = Some(1);
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
