//! The bound devices (ADR 0007 section 3): one `kabi::device::DeviceRecord` for
//! each PCI function a driver took, kept as the probe binds them, read by
//! DEV_LIST. An interrupt count is read from the vector when a record is read.

use crate::irq;
use alloc::vec::Vec;
use kabi::device::{DeviceRecord, MAX_RECORDS, VERSION, irq_mode, place, state};
use spin::Mutex;

/// The records, and the MSI-X vector of each record that has one.
static RECORDS: Mutex<Vec<(DeviceRecord, Option<u8>)>> = Mutex::new(Vec::new());

/// What a bind attempt found, for its record.
pub enum Outcome {
    /// The driver has the device, with this MSI-X vector or polled.
    Running(Option<u8>),
    /// The driver could not take it; `code` says why (see [`error`]).
    Failed(u32),
}

/// `last_error` codes.
pub mod error {
    /// The virtio transport could not be set up.
    pub const TRANSPORT: u32 = 1;
    /// BAR0 was not an assigned memory BAR.
    pub const BAR: u32 = 2;
}

/// Records `driver` taking the function at `address`; nothing once
/// [`MAX_RECORDS`] are kept.
pub fn bind(function: &pci::Function, driver: &str, outcome: Outcome) {
    let mut records = RECORDS.lock();
    if records.len() == MAX_RECORDS {
        return;
    }
    let address = function.address;
    let (class, subclass, interface) = function.class;
    let mut record = DeviceRecord {
        version: VERSION,
        index: records.len() as u16,
        segment: address.segment(),
        bus: address.bus(),
        devfn: address.device() << 3 | address.function(),
        vendor: function.vendor,
        device: function.device,
        class: [class, subclass, interface],
        place: place::KERNEL,
        ..DeviceRecord::default()
    };
    record.set_driver(driver);
    let vector = match outcome {
        Outcome::Running(vector) => {
            record.state = state::RUNNING;
            record.irq_mode = match vector {
                Some(_) => irq_mode::MSI_X,
                None => irq_mode::POLLED,
            };
            vector
        }
        Outcome::Failed(code) => {
            record.state = state::FAILED;
            record.last_error = code;
            None
        }
    };
    records.push((record, vector));
}

/// Copies the records into `out`, with their interrupt counts now; returns how
/// many it copied.
pub fn list(out: &mut [DeviceRecord]) -> usize {
    let records = RECORDS.lock();
    for (slot, (record, vector)) in out.iter_mut().zip(records.iter()) {
        *slot = DeviceRecord {
            interrupts: vector.map_or(0, irq::count),
            ..*record
        };
    }
    out.len().min(records.len())
}
