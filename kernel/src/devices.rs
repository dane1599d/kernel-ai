//! The bound devices (ADR 0007 section 3): one `kabi::device::DeviceRecord` for
//! each PCI function a driver took, kept as the probe binds them, read by
//! DEV_LIST. An interrupt count is read from the vector when a record is read.
//!
//! The list is a fixed table, not a heap vector: the probe runs before the M10
//! observe boots' aligned start, where an allocation's place depends on how
//! host-timed boot work interleaved, and a heap shaped differently would make
//! the instruction-timed part after it run differently.

use crate::irq;
use kabi::device::{DeviceRecord, MAX_RECORDS, VERSION, irq_mode, place, state};
use spin::Mutex;

/// The records in use, then the MSI-X vector of each record that has one.
struct Table {
    count: usize,
    records: [DeviceRecord; MAX_RECORDS],
    vectors: [Option<u8>; MAX_RECORDS],
}

static TABLE: Mutex<Table> = Mutex::new(Table {
    count: 0,
    records: [DeviceRecord::EMPTY; MAX_RECORDS],
    vectors: [None; MAX_RECORDS],
});

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

/// Records `driver` taking `function`; nothing once [`MAX_RECORDS`] are kept.
pub fn bind(function: &pci::Function, driver: &str, outcome: Outcome) {
    let mut table = TABLE.lock();
    let index = table.count;
    if index == MAX_RECORDS {
        return;
    }
    let address = function.address;
    let (class, subclass, interface) = function.class;
    let mut record = DeviceRecord {
        version: VERSION,
        index: index as u16,
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
    table.records[index] = record;
    table.vectors[index] = vector;
    table.count += 1;
}

/// Copies the records into `out`, with their interrupt counts now; returns how
/// many it copied.
pub fn list(out: &mut [DeviceRecord]) -> usize {
    let table = TABLE.lock();
    let count = out.len().min(table.count);
    for (i, slot) in out[..count].iter_mut().enumerate() {
        *slot = DeviceRecord {
            interrupts: table.vectors[i].map_or(0, irq::count),
            ..table.records[i]
        };
    }
    count
}
