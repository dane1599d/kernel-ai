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
//! `value` at `offset`, aligned to `width`, inside `space`, with no `value` bit
//! above the width; anything else is refused. Then:
//!
//! - **The header, 0..0x40**: only the command register (0x04) may be written,
//!   and only its Memory Space (bit 1) and Bus Master (bit 2) bits may change
//!   from what `space` holds. A write that reaches Status or any other byte of
//!   the header is refused (this covers type 0's BARs and ROM BAR and type 1's
//!   bridge windows), and so is one that changes Interrupt Disable. A header
//!   type other than 0 or 1 (CardBus puts other things at 0x34) refuses
//!   everything above the header.
//! - **The standard capability list**: walked only when Status bit 4 is set
//!   (Status is read-only, and the kernel's own walk, `msix::capabilities`,
//!   takes the same gate, so nothing is ever programmed in a list that is not
//!   there), from the pointer at 0x34. A pointer is masked with 0xFC and must
//!   lie in 0x40..=0xFC; the walk ends at a zero pointer. A capability's ID and
//!   next bytes are refused, and the whole body of MSI (12, 16, 20 or 24 bytes
//!   by its message control, the first two with Extended Message Data) and
//!   MSI-X (12 bytes). The list is malformed, and then every write from 0x40
//!   up, extended space included, is refused, when it has a pointer out of
//!   range or a loop; an MSI or MSI-X body that runs past 0xFF; or an Enhanced
//!   Allocation (0x14) or HyperTransport (0x08) capability, which move decode
//!   and interrupt routing in ways not modelled here.
//! - **Extended space, 0x100..0x1000**, when `space` has 4096 bytes: walked
//!   from 0x100 (a zero header dword there means none). Each header dword is
//!   refused, and the bodies of SR-IOV (ID 0x0010, 0x40 bytes), Resizable BAR
//!   (0x0015) and VF Resizable BAR (0x0024), 0x34 bytes each, which move BARs
//!   too, from the capability's start whatever follows it. `next` is masked
//!   with 0xFFC; it is 0, which ends the walk, or at least 0x100. The list is
//!   malformed, and then every extended write is refused, when it has a loop, a
//!   pointer out of range or an all-ones header; a capability that starts
//!   inside a BAR mover's body; or a Multicast capability (0x0012), which has
//!   a base address.
//! - A write that overlaps any refused byte is refused.
//!
//! The walks bound their work and their stack: a bitmap of 512 bytes of refused
//! bytes and a few small seen sets, under 2 KB in all.
//!
//! **For the handler** (not written yet): take the old command value and both
//! lists from a fresh read, under the lock that serializes the write with the
//! kernel's own configuration writes; a bit the kernel set after an earlier
//! snapshot, such as Interrupt Disable, would look unchanged, and the driver's
//! write would clear it. And make real 1- and 2-byte ECAM stores of exactly
//! the low `width` bytes: a dword read-modify-write at 0x04 writes Status back,
//! which clears its write-1-to-clear bits.

/// Entries of the standard list that are followed (as `pci::msix` does). The
/// walk's `seen` set already ends any list in 48 distinct dwords, so this and
/// [`MAX_EXTENDED`] cannot be reached; they stay as defence in depth against a
/// change to the loop check.
pub const MAX_CAPABILITIES: usize = 48;
/// Entries of the extended list that are followed: dwords from 0x100 to 0xFFC
/// (unreachable for the same reason).
pub const MAX_EXTENDED: usize = 960;

const COMMAND: usize = 0x04;
/// Command bits a driver may change: Memory Space and Bus Master.
const COMMAND_ALLOWED: u16 = 0b110;
const STATUS: usize = 0x06;
const STATUS_CAPABILITIES: u16 = 1 << 4;
const HEADER_TYPE: usize = 0x0E;
const CAPABILITY_POINTER: usize = 0x34;
const HEADER_END: usize = 0x40;
const STANDARD_END: usize = 0x100;
const EXTENDED_END: usize = 0x1000;

const CAP_HYPERTRANSPORT: u8 = 0x08;
const CAP_MSI: u8 = 0x05;
const CAP_MSIX: u8 = 0x11;
const CAP_ENHANCED_ALLOCATION: u8 = 0x14;
const MSIX_BYTES: usize = 12;
const EXT_MULTICAST: u16 = 0x0012;
const EXT_SRIOV: u16 = 0x0010;
const EXT_RESIZABLE_BAR: u16 = 0x0015;
const EXT_VF_RESIZABLE_BAR: u16 = 0x0024;
/// The spec's lengths of the capabilities that move BARs, from their start.
const SRIOV_BYTES: usize = 0x40;
const RESIZABLE_BAR_BYTES: usize = 0x34;

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
    /// `value` has a bit above the write's width.
    Value,
    /// A header byte other than the command register's.
    Header,
    /// A command bit other than Memory Space and Bus Master would change.
    CommandBits,
    /// The header type is neither 0 nor 1: nothing above the header is written.
    HeaderType,
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
    if width < 4 && value >> (8 * width) != 0 {
        return Err(Refused::Value);
    }
    if start < HEADER_END {
        return header(space, start, end, value);
    }
    if space[HEADER_TYPE] & 0x7F > 1 {
        return Err(Refused::HeaderType);
    }
    // An aligned write of at most 4 bytes never straddles 0x40 or 0x100.
    let mut refused = Bytes::new();
    standard(space, &mut refused).map_err(|()| Refused::BadList)?;
    if start >= STANDARD_END {
        extended(space, &mut refused).map_err(|()| Refused::BadExtended)?;
    }
    match refused.any(start, end) {
        true => Err(Refused::Capability),
        false => Ok(()),
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

/// A bit per byte of the space: the bytes a write may not touch. 512 bytes.
struct Bytes([u64; EXTENDED_END / 64]);

impl Bytes {
    fn new() -> Self {
        Bytes([0; EXTENDED_END / 64])
    }

    /// Refuses the bytes `start..end`, clipped to the space.
    fn refuse(&mut self, start: usize, end: usize) {
        for at in start..end.min(EXTENDED_END) {
            self.0[at / 64] |= 1 << (at % 64);
        }
    }

    fn any(&self, start: usize, end: usize) -> bool {
        (start..end).any(|at| self.0[at / 64] >> (at % 64) & 1 != 0)
    }
}

/// Walks the standard list into `refused`; `Err` when it is malformed.
fn standard(space: &[u8], refused: &mut Bytes) -> Result<(), ()> {
    let status = u16::from_le_bytes([space[STATUS], space[STATUS + 1]]);
    if status & STATUS_CAPABILITIES == 0 {
        return Ok(());
    }
    let mut seen = 0u64; // a bit per dword of 0x40..0x100
    let mut at = usize::from(space[CAPABILITY_POINTER] & 0xFC);
    let mut entries = 0;
    while at != 0 {
        if !(HEADER_END..=0xFC).contains(&at) {
            return Err(());
        }
        let bit = 1u64 << ((at - HEADER_END) / 4);
        if seen & bit != 0 || entries == MAX_CAPABILITIES {
            return Err(());
        }
        seen |= bit;
        entries += 1;
        refused.refuse(at, at + 2);
        let body = match space[at] {
            CAP_MSI => {
                let control = u16::from_le_bytes([space[at + 2], space[at + 3]]);
                // 64-bit address: bit 7; per-vector masking: bit 8. Without
                // masking the 10 and 14 bytes are 12 and 16 with Extended
                // Message Data, which only MSI can own, the next capability
                // being dword aligned.
                match (control & 0x80 != 0, control & 0x100 != 0) {
                    (false, false) => 12,
                    (true, false) => 16,
                    (false, true) => 20,
                    (true, true) => 24,
                }
            }
            CAP_MSIX => MSIX_BYTES,
            // These move decode or interrupt routing in ways not modelled.
            CAP_ENHANCED_ALLOCATION | CAP_HYPERTRANSPORT => return Err(()),
            _ => 0,
        };
        if at + body > STANDARD_END {
            return Err(());
        }
        refused.refuse(at, at + body);
        at = usize::from(space[at + 1] & 0xFC);
    }
    Ok(())
}

/// Walks the extended list into `refused`; `Err` when it is malformed. A
/// 256-byte space has none. Two passes: the first checks the walk and notes
/// where each capability starts, the second refuses what each owns.
fn extended(space: &[u8], refused: &mut Bytes) -> Result<(), ()> {
    if space.len() < EXTENDED_END {
        return Ok(());
    }
    let dword =
        |at: usize| u32::from_le_bytes([space[at], space[at + 1], space[at + 2], space[at + 3]]);
    if dword(STANDARD_END) == 0 {
        return Ok(());
    }
    // A bit per dword of 0x100..0x1000: where a capability starts.
    let mut starts = [0u64; MAX_EXTENDED.div_ceil(64)];
    let slot = |at: usize| (at - STANDARD_END) / 4;
    let mut at = STANDARD_END;
    let mut entries = 0;
    loop {
        let header = dword(at);
        let (word, bit) = (slot(at) / 64, 1u64 << (slot(at) % 64));
        if header == u32::MAX || starts[word] & bit != 0 || entries == MAX_EXTENDED {
            return Err(());
        }
        starts[word] |= bit;
        entries += 1;
        let next = ((header >> 20) & 0xFFC) as usize;
        match next {
            0 => break,
            n if n < STANDARD_END => return Err(()),
            n => at = n,
        }
    }
    for dword_at in (STANDARD_END..EXTENDED_END).step_by(4) {
        if starts[slot(dword_at) / 64] >> (slot(dword_at) % 64) & 1 == 0 {
            continue;
        }
        refused.refuse(dword_at, dword_at + 4);
        let body = match (dword(dword_at) & 0xFFFF) as u16 {
            EXT_SRIOV => SRIOV_BYTES,
            EXT_RESIZABLE_BAR | EXT_VF_RESIZABLE_BAR => RESIZABLE_BAR_BYTES,
            // A base address of its own.
            EXT_MULTICAST => return Err(()),
            _ => continue,
        };
        // Another capability starting inside the body would leave it unowned
        // by the walk's order: a malformed list, however it came to be.
        let inside = (dword_at + 4..(dword_at + body).min(EXTENDED_END)).step_by(4);
        if inside
            .into_iter()
            .any(|a| starts[slot(a) / 64] >> (slot(a) % 64) & 1 != 0)
        {
            return Err(());
        }
        refused.refuse(dword_at, dword_at + body);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
