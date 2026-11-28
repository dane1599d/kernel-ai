//! The event log (doc 48882 rev 3.10, section 2.5): 128-bit entries, as four
//! little-endian `u32`s, that the IOMMU writes into a ring and makes visible
//! by moving the Event Log Tail register. The code is bits 31:28 of the
//! second word. Entries and registers are the IOMMU's words, so nothing here
//! trusts them: an unknown code is [`Event::Other`], and a head or tail
//! outside the ring is an error.
//!
//! | Event                    | Code | Fields                                  |
//! |--------------------------|------|-----------------------------------------|
//! | ILLEGAL_DEV_TABLE_ENTRY  | 1    | DeviceID, flags, address                |
//! | IO_PAGE_FAULT            | 2    | DeviceID, DomainID, flags, address      |
//! | DEV_TAB_HARDWARE_ERROR   | 3    | DeviceID, flags, address                |
//! | PAGE_TAB_HARDWARE_ERROR  | 4    | DeviceID, DomainID, flags, address      |
//! | ILLEGAL_COMMAND_ERROR    | 5    | the command's address                   |
//! | COMMAND_HARDWARE_ERROR   | 6    | flags, address                          |
//! | IOTLB_INV_TIMEOUT        | 7    | DeviceID, address                       |
//! | INVALID_DEVICE_REQUEST   | 8    | DeviceID, flags, address                |
//!
//! DeviceID is bits 15:0 of word 0, DomainID bits 15:0 of word 1, the flags
//! bits 27:16 of word 1, and the address words 2 (low) and 3 (high).

use crate::command::POINTER;
use alloc::vec::Vec;

/// Bytes in one entry.
pub const EVENT_BYTES: usize = 16;

/// Bits 27:16 of an entry's second word, by their bit in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flags(pub u16);

impl Flags {
    fn at(self, bit: u32) -> bool {
        u32::from(self.0) >> (bit - 16) & 1 == 1
    }
    /// GN (16): the address is a guest address, with a PASID.
    pub fn guest(self) -> bool {
        self.at(16)
    }
    /// NX (17): an instruction fetch.
    pub fn execute(self) -> bool {
        self.at(17)
    }
    /// US (18): a user-mode access.
    pub fn user(self) -> bool {
        self.at(18)
    }
    /// I (19): an interrupt request, not a memory access.
    pub fn interrupt(self) -> bool {
        self.at(19)
    }
    /// PR (20): the page was present (a permission fault, not a missing page).
    pub fn present(self) -> bool {
        self.at(20)
    }
    /// RW (21): a write.
    pub fn write(self) -> bool {
        self.at(21)
    }
    /// PE (22): the entry's permissions refused the access.
    pub fn permission(self) -> bool {
        self.at(22)
    }
    /// RZ (23): a reserved bit was set in an entry, or Next Level was bad.
    pub fn reserved_bits(self) -> bool {
        self.at(23)
    }
    /// TR (24): a translation request (ATS), not a plain access.
    pub fn translation_request(self) -> bool {
        self.at(24)
    }
    /// Type (26:25) of a hardware error: what the IOMMU was reading.
    pub fn kind(self) -> u8 {
        (u32::from(self.0) >> 9 & 3) as u8
    }
}

/// One event log entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    IllegalDevTableEntry {
        device: u16,
        flags: Flags,
        address: u64,
    },
    IoPageFault {
        device: u16,
        domain: u16,
        flags: Flags,
        address: u64,
    },
    DevTabHardwareError {
        device: u16,
        flags: Flags,
        address: u64,
    },
    PageTabHardwareError {
        device: u16,
        domain: u16,
        flags: Flags,
        address: u64,
    },
    IllegalCommand {
        address: u64,
    },
    CommandHardwareError {
        flags: Flags,
        address: u64,
    },
    IotlbInvTimeout {
        device: u16,
        address: u64,
    },
    InvalidDeviceRequest {
        device: u16,
        flags: Flags,
        address: u64,
    },
    /// A code this crate does not read, kept whole.
    Other {
        code: u8,
        words: [u32; 4],
    },
}

impl Event {
    /// The entry in `words`; any words are one.
    pub fn decode(words: [u32; 4]) -> Self {
        let device = words[0] as u16;
        let domain = words[1] as u16;
        let flags = Flags((words[1] >> 16 & 0xfff) as u16);
        let address = u64::from(words[2]) | u64::from(words[3]) << 32;
        match words[1] >> 28 {
            1 => Event::IllegalDevTableEntry {
                device,
                flags,
                address,
            },
            2 => Event::IoPageFault {
                device,
                domain,
                flags,
                address,
            },
            3 => Event::DevTabHardwareError {
                device,
                flags,
                address,
            },
            4 => Event::PageTabHardwareError {
                device,
                domain,
                flags,
                address,
            },
            5 => Event::IllegalCommand { address },
            6 => Event::CommandHardwareError { flags, address },
            7 => Event::IotlbInvTimeout { device, address },
            8 => Event::InvalidDeviceRequest {
                device,
                flags,
                address,
            },
            code => Event::Other {
                code: code as u8,
                words,
            },
        }
    }

    /// The entry in 16 bytes as the log holds them.
    pub fn from_bytes(bytes: [u8; EVENT_BYTES]) -> Self {
        let word = |n: usize| u32::from_le_bytes(bytes[4 * n..4 * n + 4].try_into().unwrap());
        Self::decode([word(0), word(1), word(2), word(3)])
    }
}

/// Why the log was not read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventError {
    /// A head or tail register past the ring.
    Pointer(u64),
    /// The ring's memory is not [`EventLog::bytes`] long.
    RingBytes(usize),
}

/// The event log ring: a power of two of entries from 256 to 32768 (doc
/// 48882's EventLen 8-15).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventLog {
    len: u64,
}

impl EventLog {
    pub fn new(entries: u32) -> Option<Self> {
        (entries.is_power_of_two() && (256..=32768).contains(&entries)).then_some(Self {
            len: u64::from(entries) * EVENT_BYTES as u64,
        })
    }

    /// Bytes the ring's memory must have.
    pub fn bytes(&self) -> usize {
        self.len as usize
    }

    /// The entries from `head` to `tail` (the registers as read) in `ring`,
    /// and the head to write back: `tail`, all of them read.
    pub fn drain(
        &self,
        ring: &[u8],
        head: u64,
        tail: u64,
    ) -> Result<(Vec<Event>, u64), EventError> {
        if ring.len() as u64 != self.len {
            return Err(EventError::RingBytes(ring.len()));
        }
        let (mut at, tail) = (head & POINTER, tail & POINTER);
        for pointer in [at, tail] {
            if pointer >= self.len {
                return Err(EventError::Pointer(pointer));
            }
        }
        let mut events = Vec::new();
        while at != tail {
            let start = at as usize;
            let bytes = ring[start..start + EVENT_BYTES].try_into().unwrap();
            events.push(Event::from_bytes(bytes));
            at = (at + EVENT_BYTES as u64) % self.len;
        }
        Ok((events, tail))
    }
}
