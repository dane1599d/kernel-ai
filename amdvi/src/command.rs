//! Commands (doc 48882 rev 3.10, section 2.4): 128 bits each, as four
//! little-endian `u32`s, written into the command buffer ring and made
//! visible by moving the Command Buffer Tail register. The opcode is bits
//! 31:28 of the second word.
//!
//! | Command                 | Op | Words                                        |
//! |-------------------------|----|----------------------------------------------|
//! | COMPLETION_WAIT         | 1  | 0: S bit 0, I bit 1, F bit 2, store address 31:3; 1: store address 51:32 in 19:0; 2-3: store data |
//! | INVALIDATE_DEVTAB_ENTRY | 2  | 0: DeviceID in 15:0                          |
//! | INVALIDATE_IOMMU_PAGES  | 3  | 0: PASID in 19:0; 1: DomainID in 15:0; 2: S bit 0, PDE bit 1, GN bit 2, address 31:12; 3: address 63:32 |
//!
//! With S set, INVALIDATE_IOMMU_PAGES covers a naturally aligned power of
//! two of pages, told by the lowest clear address bit from bit 12: bit 12
//! clear is 2 pages, bits 12 set and 13 clear is 4, and so on; the address
//! `7FFF_FFFF_FFFF_F000` with S set is every page of the domain.

use crate::page_address;

pub const COMPLETION_WAIT: u32 = 0x1;
pub const INVALIDATE_DEVTAB_ENTRY: u32 = 0x2;
pub const INVALIDATE_IOMMU_PAGES: u32 = 0x3;

/// The head and tail registers' offset, bits 18:4.
pub const POINTER: u64 = 0x7_fff0;

/// Bytes in one command.
pub const COMMAND_BYTES: usize = 16;
/// The address that, with S set, invalidates every page of a domain.
pub const ALL_PAGES: u64 = 0x7fff_ffff_ffff_f000;

/// Which pages an INVALIDATE_IOMMU_PAGES covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pages {
    /// The 4 KiB page at this IOVA.
    One(u64),
    /// `count` pages from `iova`: a power of two, at least 2, and `iova` a
    /// multiple of `count` pages.
    Range { iova: u64, count: u64 },
    /// Every page of the domain.
    All,
}

/// One command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Waits for every earlier command; then, with `store`, writes its data
    /// at its address (8-byte aligned), and with `interrupt` raises the
    /// completion-wait interrupt. `flush` lets later commands start first.
    CompletionWait {
        store: Option<(u64, u64)>,
        interrupt: bool,
        flush: bool,
    },
    /// Drops the IOMMU's cached copy of a device table entry.
    InvalidateDevtabEntry { device: u16 },
    /// Drops cached translations of a domain; with `pde`, its cached page
    /// directory entries too (needed after a table is freed).
    InvalidateIommuPages {
        domain: u16,
        pages: Pages,
        pde: bool,
    },
}

/// Why a command cannot be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandError {
    /// A store address not 8-byte aligned or above bit 51.
    StoreAddress(u64),
    /// Pages not page-aligned, a count that is not a power of two of at
    /// least 2, or an IOVA not aligned to it.
    Pages(Pages),
    /// The ring has no room: the IOMMU has not read far enough.
    Full,
    /// The head the IOMMU gave is not an entry of this ring.
    Head(u64),
    /// The ring's memory is not [`CommandRing::bytes`] long.
    RingBytes(usize),
}

impl Command {
    /// The four words, in the order they sit in the ring.
    pub fn encode(&self) -> Result<[u32; 4], CommandError> {
        let op = |code: u32| code << 28;
        Ok(match *self {
            Command::CompletionWait {
                store,
                interrupt,
                flush,
            } => {
                let (address, data, s) = match store {
                    Some((address, data)) => {
                        if !address.is_multiple_of(8) || !page_address(address & !0xfff) {
                            return Err(CommandError::StoreAddress(address));
                        }
                        (address, data, 1)
                    }
                    None => (0, 0, 0),
                };
                [
                    address as u32 & !7 | s | u32::from(interrupt) << 1 | u32::from(flush) << 2,
                    (address >> 32) as u32 & 0xf_ffff | op(COMPLETION_WAIT),
                    data as u32,
                    (data >> 32) as u32,
                ]
            }
            Command::InvalidateDevtabEntry { device } => {
                [u32::from(device), op(INVALIDATE_DEVTAB_ENTRY), 0, 0]
            }
            Command::InvalidateIommuPages { domain, pages, pde } => {
                let (address, s) = match pages {
                    Pages::One(iova) if iova.is_multiple_of(4096) => (iova, 0),
                    Pages::Range { iova, count }
                        if count >= 2
                            && count.is_power_of_two()
                            && count
                                .checked_mul(4096)
                                .is_some_and(|len| iova.is_multiple_of(len)) =>
                    {
                        // The bits below the size's own are set; its bit is clear.
                        (iova | (count / 2 - 1) << 12, 1)
                    }
                    Pages::All => (ALL_PAGES, 1),
                    _ => return Err(CommandError::Pages(pages)),
                };
                [
                    0,
                    u32::from(domain) | op(INVALIDATE_IOMMU_PAGES),
                    address as u32 & !0xfff | s | u32::from(pde) << 1,
                    (address >> 32) as u32,
                ]
            }
        })
    }

    /// The command as the ring holds it, little-endian.
    pub fn to_bytes(&self) -> Result<[u8; COMMAND_BYTES], CommandError> {
        let mut bytes = [0; COMMAND_BYTES];
        for (chunk, word) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(self.encode()?) {
            chunk.copy_from_slice(&word.to_le_bytes());
        }
        Ok(bytes)
    }
}

/// The opcode of an encoded command.
pub const fn opcode(words: [u32; 4]) -> u32 {
    words[1] >> 28
}

/// The command buffer: a ring of `len` bytes, a power of two of commands
/// from 256 to 32768 (doc 48882's ComLen 8-15), and the tail the driver
/// writes. Head and tail are byte offsets, as the head and tail registers
/// hold them in bits 18:4; the ring is full one command before the head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRing {
    len: u64,
    tail: u64,
}

impl CommandRing {
    /// A ring of `entries` commands, empty, its tail at 0.
    pub fn new(entries: u32) -> Option<Self> {
        (entries.is_power_of_two() && (256..=32768).contains(&entries)).then_some(Self {
            len: u64::from(entries) * COMMAND_BYTES as u64,
            tail: 0,
        })
    }

    /// Bytes the ring's memory must have.
    pub fn bytes(&self) -> usize {
        self.len as usize
    }

    /// The value for the tail register.
    pub fn tail(&self) -> u64 {
        self.tail
    }

    /// Writes `command` at the tail in `ring` and moves the tail past it;
    /// `head` is the head register as the IOMMU left it, so it is checked.
    /// The caller then writes [`CommandRing::tail`] to the tail register.
    pub fn push(
        &mut self,
        ring: &mut [u8],
        head: u64,
        command: &Command,
    ) -> Result<(), CommandError> {
        let bytes = command.to_bytes()?;
        if ring.len() as u64 != self.len {
            return Err(CommandError::RingBytes(ring.len()));
        }
        let head = head & POINTER;
        if head >= self.len {
            return Err(CommandError::Head(head));
        }
        let next = (self.tail + COMMAND_BYTES as u64) % self.len;
        if next == head {
            return Err(CommandError::Full);
        }
        let at = self.tail as usize;
        ring[at..at + COMMAND_BYTES].copy_from_slice(&bytes);
        self.tail = next;
        Ok(())
    }
}
