//! The DEV_CFG filter (ADR 0007 section 5, DEV_CFG: "Own function. Writes to
//! BARs, MSI and MSI-X capabilities, and command bits other than Memory Space
//! and Bus Master are refused."): [`allow_write`] says whether a driver's write
//! to its own function's configuration space may go through. It is a pure
//! function of the space as the function reads now, so it is host-tested and
//! fuzzed; the kernel's handler (not written yet) asks it before every write.
//!
//! A driver may move a device's BARs, steal its interrupts or turn off its
//! own isolation (Interrupt Disable, SERR) only by writing here, so the filter
//! fails closed: whatever it cannot read with confidence is refused.
//!
//! `space` is the function's configuration space as read: 256 bytes, or 4096
//! for PCIe, and no other size. The write is `width` bytes (1, 2 or 4) of
//! `value` at `offset`, aligned to `width` and inside `space`; anything else is
//! refused. Then:
//!
//! - **The header, 0..0x40**: only the command register (0x04) may be written,
//!   and only its Memory Space (bit 1) and Bus Master (bit 2) bits may change
//!   from what `space` holds. A write that reaches Status or any other byte of
//!   the header is refused (this covers type 0's BARs and ROM BAR and type 1's
//!   bridge windows), and so is one that changes Interrupt Disable.
//! - **The standard capability list**: walked only when Status bit 4 is set,
//!   from the pointer at 0x34. A pointer is masked with 0xFC and must lie in
//!   0x40..=0xFC; the walk ends at a zero pointer. A capability's ID and next
//!   bytes are refused, and the whole body of MSI (10, 14, 20 or 24 bytes by
//!   its message control) and MSI-X (12 bytes). A list that has a pointer out of
//!   range, that loops, or that has more than [`MAX_CAPABILITIES`] entries is
//!   malformed, and then every write from 0x40 up, extended space included, is
//!   refused.
//! - **Extended space, 0x100..0x1000**, when `space` has 4096 bytes: walked
//!   from 0x100 (a zero header dword there means none). Each header dword is
//!   refused, and the bodies of SR-IOV (ID 0x0010), Resizable BAR (0x0015)
//!   and VF Resizable BAR (0x0024), which move BARs too, up to the next
//!   capability that starts after them, or the end of the space. `next` is
//!   masked with 0xFFC; it is 0, which ends the walk, or at least 0x100. More
//!   than [`MAX_EXTENDED`] entries, a loop, an all-ones header or a pointer out
//!   of range is malformed, and then every extended write is refused.
//! - A write that overlaps any refused byte is refused.

/// Entries of the standard list that are followed (as `pci::msix` does).
pub const MAX_CAPABILITIES: usize = 48;
/// Entries of the extended list that are followed: dwords from 0x100 to 0xFFC.
pub const MAX_EXTENDED: usize = 960;

const COMMAND: usize = 0x04;
/// Command bits a driver may change: Memory Space and Bus Master.
const COMMAND_ALLOWED: u16 = 0b110;
const STATUS: usize = 0x06;
const STATUS_CAPABILITIES: u16 = 1 << 4;
const CAPABILITY_POINTER: usize = 0x34;
const HEADER_END: usize = 0x40;
const STANDARD_END: usize = 0x100;
const EXTENDED_END: usize = 0x1000;

const CAP_MSI: u8 = 0x05;
const CAP_MSIX: u8 = 0x11;
const MSIX_BYTES: usize = 12;
const EXT_SRIOV: u16 = 0x0010;
const EXT_RESIZABLE_BAR: u16 = 0x0015;
const EXT_VF_RESIZABLE_BAR: u16 = 0x0024;

/// Why a write was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// `space` is neither 256 nor 4096 bytes.
    SpaceSize,
    /// The width is not 1, 2 or 4.
    Width,
    /// The offset is not a multiple of the width.
    Misaligned,
    /// The write ends past `space`.
    Outside,
    /// A header byte other than the command register's.
    Header,
    /// A command bit other than Memory Space and Bus Master would change.
    CommandBits,
    /// A capability's ID or next byte, or the body of MSI, MSI-X, SR-IOV or a
    /// Resizable BAR.
    Capability,
    /// The standard list is malformed, so nothing from 0x40 up is written.
    BadList,
    /// The extended list is malformed, so nothing from 0x100 up is written.
    BadExtended,
}

/// Whether the write of `width` bytes of `value` at `offset` into the
/// function's configuration space `space` may go through.
pub fn allow_write(space: &[u8], offset: u16, width: u8, value: u32) -> Result<(), Refused> {
    if space.len() != STANDARD_END && space.len() != EXTENDED_END {
        return Err(Refused::SpaceSize);
    }
    if !matches!(width, 1 | 2 | 4) {
        return Err(Refused::Width);
    }
    let (start, width) = (usize::from(offset), usize::from(width));
    if start % width != 0 {
        return Err(Refused::Misaligned);
    }
    let end = start + width;
    if end > space.len() {
        return Err(Refused::Outside);
    }
    if start < HEADER_END {
        return header(space, start, end, value);
    }
    // An aligned write of at most 4 bytes never straddles 0x40 or 0x100.
    let list = standard(space);
    if start < STANDARD_END {
        return match list {
            Err(()) => Err(Refused::BadList),
            Ok(list) => overlaps(list.refused(), start, end),
        };
    }
    if list.is_err() {
        return Err(Refused::BadList);
    }
    match extended(space) {
        Err(()) => Err(Refused::BadExtended),
        Ok(list) => overlaps(list.refused(), start, end),
    }
}

/// A write into 0..0x40: the command register's two bytes, changing only the
/// allowed bits.
fn header(space: &[u8], start: usize, end: usize, value: u32) -> Result<(), Refused> {
    if start < COMMAND || end > COMMAND + 2 {
        return Err(Refused::Header);
    }
    let old = u16::from_le_bytes([space[COMMAND], space[COMMAND + 1]]);
    let mut new = old.to_le_bytes();
    for (i, byte) in new.iter_mut().enumerate() {
        let at = COMMAND + i;
        if (start..end).contains(&at) {
            *byte = (value >> (8 * (at - start))) as u8;
        }
    }
    match (old ^ u16::from_le_bytes(new)) & !COMMAND_ALLOWED {
        0 => Ok(()),
        _ => Err(Refused::CommandBits),
    }
}

/// `Err` if a refused range overlaps `start..end`.
fn overlaps(ranges: &[(usize, usize)], start: usize, end: usize) -> Result<(), Refused> {
    match ranges.iter().any(|&(a, b)| a < end && start < b) {
        true => Err(Refused::Capability),
        false => Ok(()),
    }
}

/// The standard list's refused byte ranges.
struct Standard {
    ranges: [(usize, usize); 2 * MAX_CAPABILITIES],
    count: usize,
}

impl Standard {
    fn refused(&self) -> &[(usize, usize)] {
        &self.ranges[..self.count]
    }
}

/// Walks the standard list; `Err` when it is malformed.
fn standard(space: &[u8]) -> Result<Standard, ()> {
    let mut list = Standard {
        ranges: [(0, 0); 2 * MAX_CAPABILITIES],
        count: 0,
    };
    let status = u16::from_le_bytes([space[STATUS], space[STATUS + 1]]);
    if status & STATUS_CAPABILITIES == 0 {
        return Ok(list);
    }
    let mut seen = [false; STANDARD_END / 4];
    let mut at = usize::from(space[CAPABILITY_POINTER] & 0xFC);
    let mut entries = 0;
    while at != 0 {
        if !(HEADER_END..=0xFC).contains(&at) || seen[at / 4] || entries == MAX_CAPABILITIES {
            return Err(());
        }
        seen[at / 4] = true;
        entries += 1;
        // The ID and next bytes, then the body of the two that move interrupts.
        list.ranges[list.count] = (at, at + 2);
        list.count += 1;
        let body = match space[at] {
            CAP_MSI => {
                let control = u16::from_le_bytes([space[at + 2], space[at + 3]]);
                // 64-bit address: bit 7; per-vector masking: bit 8.
                match (control & 0x80 != 0, control & 0x100 != 0) {
                    (false, false) => 10,
                    (true, false) => 14,
                    (false, true) => 20,
                    (true, true) => 24,
                }
            }
            CAP_MSIX => MSIX_BYTES,
            _ => 0,
        };
        if body != 0 {
            list.ranges[list.count] = (at, (at + body).min(STANDARD_END));
            list.count += 1;
        }
        at = usize::from(space[at + 1] & 0xFC);
    }
    Ok(list)
}

/// The extended list's refused byte ranges.
struct Extended {
    ranges: [(usize, usize); 2 * MAX_EXTENDED],
    count: usize,
}

impl Extended {
    fn refused(&self) -> &[(usize, usize)] {
        &self.ranges[..self.count]
    }
}

/// Walks the extended list; `Err` when it is malformed. A 256-byte space has
/// none.
fn extended(space: &[u8]) -> Result<Extended, ()> {
    let mut list = Extended {
        ranges: [(0, 0); 2 * MAX_EXTENDED],
        count: 0,
    };
    if space.len() < EXTENDED_END {
        return Ok(list);
    }
    let dword =
        |at: usize| u32::from_le_bytes([space[at], space[at + 1], space[at + 2], space[at + 3]]);
    if dword(STANDARD_END) == 0 {
        return Ok(list);
    }
    // Each capability's start, to bound the bodies of the BAR movers.
    let mut starts = [0u16; MAX_EXTENDED];
    let mut seen = [false; MAX_EXTENDED];
    let mut entries = 0;
    let mut at = STANDARD_END;
    loop {
        let header = dword(at);
        if header == u32::MAX || seen[(at - STANDARD_END) / 4] || entries == MAX_EXTENDED {
            return Err(());
        }
        seen[(at - STANDARD_END) / 4] = true;
        starts[entries] = at as u16;
        entries += 1;
        list.ranges[list.count] = (at, at + 4);
        list.count += 1;
        let next = ((header >> 20) & 0xFFC) as usize;
        if next == 0 {
            break;
        }
        if next < STANDARD_END {
            return Err(());
        }
        at = next;
    }
    for &start in &starts[..entries] {
        let start = usize::from(start);
        let id = (dword(start) & 0xFFFF) as u16;
        if matches!(id, EXT_SRIOV | EXT_RESIZABLE_BAR | EXT_VF_RESIZABLE_BAR) {
            // To the next capability that starts after this one, else the end.
            let end = starts[..entries]
                .iter()
                .map(|&s| usize::from(s))
                .filter(|&s| s > start)
                .min()
                .unwrap_or(EXTENDED_END);
            list.ranges[list.count] = (start + 4, end);
            list.count += 1;
        }
    }
    Ok(list)
}

#[cfg(test)]
mod tests;
