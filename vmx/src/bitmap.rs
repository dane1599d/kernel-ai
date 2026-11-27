//! The MSR bitmap (VMCS 0x2004; SDM vol. 3C, 25.6.9) and the two I/O bitmaps
//! (0x2000 and 0x2002; 25.6.4): which RDMSR, WRMSR, IN and OUT accesses exit.
//! A set bit exits. Each bitmap is one 4 KiB page.

/// One bitmap page's size.
pub const BITMAP_SIZE: usize = 4096;

/// Which quarter of the MSR bitmap: reads of the low MSRs (0-0x1FFF), reads
/// of the high ones (0xC000_0000-0xC000_1FFF), then writes of each, 1 KiB
/// each.
const READ_LOW: usize = 0x000;
const READ_HIGH: usize = 0x400;
const WRITE_LOW: usize = 0x800;
const WRITE_HIGH: usize = 0xC00;

/// Where MSR `msr`'s read bit is in the MSR bitmap: its byte and bit; the
/// write bit is 0x800 bytes on. None for an MSR outside the two ranges,
/// which always exits (25.6.9).
pub const fn msr_bit(msr: u32) -> Option<(usize, u8)> {
    let (base, from) = match msr {
        0x0000_0000..=0x0000_1FFF => (READ_LOW, 0x0000_0000),
        0xC000_0000..=0xC000_1FFF => (READ_HIGH, 0xC000_0000),
        _ => return None,
    };
    let bit = (msr - from) as usize;
    Some((base + bit / 8, (bit % 8) as u8))
}

const _: () = assert!(WRITE_LOW - READ_LOW == 0x800 && WRITE_HIGH - READ_HIGH == 0x800);

/// The MSR bitmap. It starts exiting on everything, and the kernel's
/// allow-list opens what the guest may touch itself.
#[derive(Clone, Debug, PartialEq, Eq)]
#[repr(C, align(4096))]
pub struct MsrBitmap(pub [u8; BITMAP_SIZE]);

impl MsrBitmap {
    pub const fn intercept_all() -> Self {
        Self([0xFF; BITMAP_SIZE])
    }

    /// Lets the guest read and/or write `msr` without an exit; the other
    /// direction is left as it was. Refused for an MSR outside the map.
    pub fn allow(&mut self, msr: u32, read: bool, write: bool) -> Result<(), u32> {
        let (byte, bit) = msr_bit(msr).ok_or(msr)?;
        if read {
            self.0[byte] &= !(1 << bit);
        }
        if write {
            self.0[byte + 0x800] &= !(1 << bit);
        }
        Ok(())
    }

    /// Whether a read of `msr`, and a write, exit.
    pub const fn intercepts(&self, msr: u32) -> (bool, bool) {
        match msr_bit(msr) {
            Some((byte, bit)) => (
                self.0[byte] & (1 << bit) != 0,
                self.0[byte + 0x800] & (1 << bit) != 0,
            ),
            None => (true, true),
        }
    }
}

/// The I/O bitmaps: A for ports 0-0x7FFF, B for 0x8000-0xFFFF, one bit a
/// port.
#[derive(Clone, Debug, PartialEq, Eq)]
#[repr(C, align(4096))]
pub struct IoBitmaps {
    pub a: [u8; BITMAP_SIZE],
    pub b: [u8; BITMAP_SIZE],
}

impl IoBitmaps {
    pub const fn intercept_all() -> Self {
        Self {
            a: [0xFF; BITMAP_SIZE],
            b: [0xFF; BITMAP_SIZE],
        }
    }

    const fn bit(&self, port: usize) -> bool {
        let (map, at) = match port < 0x8000 {
            true => (&self.a, port),
            false => (&self.b, port - 0x8000),
        };
        map[at / 8] & (1 << (at % 8)) != 0
    }

    /// Lets the guest use ports `first` to `first + count - 1` without an
    /// exit. Refused, with nothing changed, for a range that ends past port
    /// 0xFFFF.
    pub fn allow(&mut self, first: u16, count: u16) -> Result<(), (u16, u16)> {
        let end = first as usize + count as usize;
        if end > 0x1_0000 {
            return Err((first, count));
        }
        for port in first as usize..end {
            let (map, at) = match port < 0x8000 {
                true => (&mut self.a, port),
                false => (&mut self.b, port - 0x8000),
            };
            map[at / 8] &= !(1 << (at % 8));
        }
        Ok(())
    }

    /// Whether an access of `size` bytes at `port` exits: any of its ports'
    /// bits set, or the access wrapping past port 0xFFFF, which always exits
    /// (26.1.3).
    pub const fn intercepts(&self, port: u16, size: u8) -> bool {
        let end = port as usize + size as usize;
        if end > 0x1_0000 {
            return true;
        }
        let mut at = port as usize;
        while at < end {
            if self.bit(at) {
                return true;
            }
            at += 1;
        }
        false
    }
}
