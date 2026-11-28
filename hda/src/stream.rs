//! A PCM stream's format and its buffer descriptor list (HD Audio 1.0a
//! sections 3.6 and 3.7), and the tone the bring-up plays.

/// A stream's format: the same 16 bits in SDnFMT and the converter's
/// SET_CONVERTER_FORMAT (section 3.7.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    /// 48000 or 44100 times `mult` over `div`.
    pub base_44k1: bool,
    pub mult: u8,
    pub div: u8,
    pub bits: u8,
    pub channels: u8,
}

impl Format {
    /// 48 kHz, 16-bit, stereo: what every codec must take.
    pub const STEREO_48K_16: Format = Format {
        base_44k1: false,
        mult: 1,
        div: 1,
        bits: 16,
        channels: 2,
    };

    /// The register value, or None for a combination the format cannot say.
    pub fn encode(self) -> Option<u16> {
        let base = if self.base_44k1 { 1 << 14 } else { 0 };
        let mult = match self.mult {
            1..=4 => u16::from(self.mult - 1) << 11,
            _ => return None,
        };
        let div = match self.div {
            1..=8 => u16::from(self.div - 1) << 8,
            _ => return None,
        };
        let bits = match self.bits {
            8 => 0,
            16 => 1,
            20 => 2,
            24 => 3,
            32 => 4,
            _ => return None,
        } << 4;
        let channels = match self.channels {
            1..=16 => u16::from(self.channels - 1),
            _ => return None,
        };
        Some(base | mult | div | bits | channels)
    }

    pub fn rate(self) -> u32 {
        let base = if self.base_44k1 { 44_100 } else { 48_000 };
        base * u32::from(self.mult) / u32::from(self.div)
    }

    /// Bytes per frame, as stored in memory (20- and 24-bit in 32).
    pub fn frame_bytes(self) -> u32 {
        let container = match self.bits {
            8 => 1,
            16 => 2,
            _ => 4,
        };
        container * u32::from(self.channels)
    }
}

/// One buffer descriptor list entry: 16 bytes, an address, a length and
/// "interrupt on completion" (section 3.6.3).
pub const BDL_ENTRY: usize = 16;
/// The BDL's alignment and the buffers' (128 bytes, section 3.6.2).
pub const BDL_ALIGN: u64 = 128;

/// The BDL entry `index`'s 16 bytes for `len` bytes at `address`.
pub fn bdl_entry(address: u64, len: u32, ioc: bool) -> [u8; BDL_ENTRY] {
    let mut entry = [0u8; BDL_ENTRY];
    entry[0..8].copy_from_slice(&address.to_le_bytes());
    entry[8..12].copy_from_slice(&len.to_le_bytes());
    entry[12..16].copy_from_slice(&u32::from(ioc).to_le_bytes());
    entry
}

/// The tone's period in frames: a 375 Hz square wave at 48 kHz, so a 4 KiB
/// buffer of 16-bit stereo (1024 frames) holds eight periods and loops
/// without a click.
pub const TONE_PERIOD: usize = 128;
/// Its amplitude: a quarter of full scale.
pub const TONE_LEVEL: i16 = 0x2000;

/// Fills `buffer` with the tone as 16-bit little-endian stereo frames; a
/// trailing partial frame is left as it was.
pub fn tone(buffer: &mut [u8]) {
    for (frame, bytes) in buffer.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let high = frame % TONE_PERIOD < TONE_PERIOD / 2;
        let sample = if high { TONE_LEVEL } else { -TONE_LEVEL };
        bytes[0..2].copy_from_slice(&sample.to_le_bytes());
        bytes[2..4].copy_from_slice(&sample.to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_encode_as_the_spec_tables_say() {
        assert_eq!(Format::STEREO_48K_16.encode(), Some(0x0011));
        assert_eq!(Format::STEREO_48K_16.rate(), 48_000);
        assert_eq!(Format::STEREO_48K_16.frame_bytes(), 4);
        let cd = Format {
            base_44k1: true,
            ..Format::STEREO_48K_16
        };
        assert_eq!(cd.encode(), Some(0x4011));
        let hi = Format {
            mult: 2,
            bits: 24,
            ..Format::STEREO_48K_16
        };
        assert_eq!(
            (hi.encode(), hi.rate(), hi.frame_bytes()),
            (Some(0x0831), 96_000, 8)
        );
        let low = Format {
            div: 6,
            channels: 1,
            ..Format::STEREO_48K_16
        };
        assert_eq!((low.encode(), low.rate()), (Some(0x0510), 8_000));
        for bad in [
            Format {
                mult: 5,
                ..Format::STEREO_48K_16
            },
            Format {
                div: 0,
                ..Format::STEREO_48K_16
            },
            Format {
                bits: 12,
                ..Format::STEREO_48K_16
            },
            Format {
                channels: 17,
                ..Format::STEREO_48K_16
            },
        ] {
            assert_eq!(bad.encode(), None, "{bad:?}");
        }
    }

    #[test]
    fn a_bdl_entry_is_address_length_and_ioc() {
        let e = bdl_entry(0x1_2345_6780, 4096, true);
        assert_eq!(&e[0..8], &0x1_2345_6780u64.to_le_bytes());
        assert_eq!(&e[8..12], &4096u32.to_le_bytes());
        assert_eq!(&e[12..16], &[1, 0, 0, 0]);
        assert_eq!(&bdl_entry(0, 0, false)[12..16], &[0; 4]);
    }

    #[test]
    fn the_tone_is_a_square_wave_that_loops_in_a_page() {
        let mut page = [0u8; 4096];
        tone(&mut page);
        let left = |f: usize| i16::from_le_bytes([page[f * 4], page[f * 4 + 1]]);
        let right = |f: usize| i16::from_le_bytes([page[f * 4 + 2], page[f * 4 + 3]]);
        assert_eq!((left(0), right(0)), (TONE_LEVEL, TONE_LEVEL));
        assert_eq!(left(63), TONE_LEVEL);
        assert_eq!(left(64), -TONE_LEVEL);
        assert_eq!(left(127), -TONE_LEVEL);
        assert_eq!(left(128), TONE_LEVEL);
        assert_eq!(1024 % TONE_PERIOD, 0, "a page holds whole periods");
        assert_eq!(left(1023), -TONE_LEVEL);
        let mut odd = [0xAAu8; 6];
        tone(&mut odd);
        assert_eq!(&odd[4..], &[0xAA, 0xAA], "a partial frame is left alone");
    }
}
