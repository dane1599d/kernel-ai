//! The controller's registers (HD Audio 1.0a section 3.3), by offset from BAR0,
//! with the bits the driver uses. Each register is read and written at its own
//! offset and width: some sit at byte offsets, and an emulated controller may
//! decode an access only at the register it starts on.

/// Global capabilities (16 bits): the stream counts and 64-bit addressing.
pub const GCAP: u64 = 0x00;
/// Minor and major version (8 bits each).
pub const VMIN: u64 = 0x02;
pub const VMAJ: u64 = 0x03;
/// Global control (32 bits).
pub const GCTL: u64 = 0x08;
/// Codec presence after reset (16 bits, write 1 to clear): bit n is codec n.
pub const STATESTS: u64 = 0x0E;
/// Interrupt control (32 bits).
pub const INTCTL: u64 = 0x20;

pub const CORBLBASE: u64 = 0x40;
pub const CORBUBASE: u64 = 0x44;
/// CORB write pointer (16 bits): the last entry the driver wrote.
pub const CORBWP: u64 = 0x48;
/// CORB read pointer (16 bits): the last entry the controller sent.
pub const CORBRP: u64 = 0x4A;
pub const CORBCTL: u64 = 0x4C;
pub const CORBSTS: u64 = 0x4D;
pub const CORBSIZE: u64 = 0x4E;

pub const RIRBLBASE: u64 = 0x50;
pub const RIRBUBASE: u64 = 0x54;
/// RIRB write pointer (16 bits): the last entry the controller wrote.
pub const RIRBWP: u64 = 0x58;
/// Responses per RIRB interrupt (16 bits; 0 is 256).
pub const RINTCNT: u64 = 0x5A;
pub const RIRBCTL: u64 = 0x5C;
/// RIRB status (8 bits, write 1 to clear).
pub const RIRBSTS: u64 = 0x5D;
pub const RIRBSIZE: u64 = 0x5E;

/// GCAP: 64-bit addresses are supported.
pub const GCAP_64OK: u16 = 1 << 0;
/// GCTL: out of reset when set.
pub const GCTL_CRST: u32 = 1 << 0;
/// GCTL: accept unsolicited responses.
pub const GCTL_UNSOL: u32 = 1 << 8;

/// CORBRP: resets the read pointer to 0.
pub const CORBRP_RST: u16 = 1 << 15;
/// CORBCTL and RIRBCTL: the DMA engine runs.
pub const RING_RUN: u8 = 1 << 1;
/// RIRBWP: resets the write pointer to 0 (write only).
pub const RIRBWP_RST: u16 = 1 << 15;
/// RIRBSTS: a response interrupt is pending.
pub const RIRBSTS_RINTFL: u8 = 1 << 0;
/// RIRBSTS: the RIRB overran.
pub const RIRBSTS_OIS: u8 = 1 << 2;

/// The first stream descriptor; each is 0x20 bytes, the input streams first,
/// then the output streams, then the bidirectional ones.
pub const SD_BASE: u64 = 0x80;
pub const SD_STRIDE: u64 = 0x20;
/// Within a stream descriptor: control, bytes 0 to 2.
pub const SD_CTL0: u64 = 0x00;
/// The control's byte 2, whose high nibble is the stream number.
pub const SD_CTL2: u64 = 0x02;
/// Status (8 bits, write 1 to clear).
pub const SD_STS: u64 = 0x03;
/// Link position in the buffer, in bytes (32 bits).
pub const SD_LPIB: u64 = 0x04;
/// Cyclic buffer length, in bytes (32 bits).
pub const SD_CBL: u64 = 0x08;
/// The last valid BDL entry (16 bits).
pub const SD_LVI: u64 = 0x0C;
/// The stream format (16 bits), as the converter's.
pub const SD_FMT: u64 = 0x12;
pub const SD_BDPL: u64 = 0x18;
pub const SD_BDPU: u64 = 0x1C;

/// SDnCTL byte 0: stream reset.
pub const SD_SRST: u8 = 1 << 0;
/// SDnCTL byte 0: the stream's DMA runs.
pub const SD_RUN: u8 = 1 << 1;
/// SDnSTS: buffer completion, FIFO error, descriptor error.
pub const SD_STS_CLEAR: u8 = (1 << 2) | (1 << 3) | (1 << 4);

/// A stream descriptor's register at `reg`.
pub fn sd(index: u8, reg: u64) -> u64 {
    SD_BASE + SD_STRIDE * u64::from(index) + reg
}

/// The streams GCAP reports: (input, output, bidirectional).
pub fn streams(gcap: u16) -> (u8, u8, u8) {
    (
        ((gcap >> 8) & 0xF) as u8,
        ((gcap >> 12) & 0xF) as u8,
        ((gcap >> 3) & 0x1F) as u8,
    )
}

/// The ring entries a CORBSIZE or RIRBSIZE register allows, largest first:
/// its capability nibble says which of 2, 16 and 256 the controller has.
/// Returns the size field to write and the entry count.
pub fn ring_size(size_reg: u8) -> Option<(u8, u16)> {
    let cap = size_reg >> 4;
    [(0b100, 2, 256), (0b010, 1, 16), (0b001, 0, 2)]
        .into_iter()
        .find(|(bit, _, _)| cap & bit != 0)
        .map(|(_, field, entries)| (field, entries))
}
