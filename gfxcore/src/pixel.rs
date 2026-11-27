//! How a 32-bit framebuffer pixel is packed. Limine reports each colour channel's
//! size and shift (from UEFI GOP's pixel format), and the order is not always
//! QEMU's blue in the low byte: RGBX has red there, and a 10-bit-per-channel
//! panel packs 10 bits a channel. Callers draw in `0x00RRGGBB` and pack once, at
//! the framebuffer.

/// One colour channel in a pixel: how many bits it has and where they start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Channel {
    // Private: `Format::from_masks` keeps `size` within 1..=16 and `size + shift`
    // within 32, which `pack` and `mask` rely on to never overflow a shift.
    size: u8,
    shift: u8,
}

impl Channel {
    /// An 8-bit channel value `v` as this channel's bits, in place. A channel
    /// narrower than 8 bits drops the low bits; a wider one repeats the high bits
    /// so that 0 stays 0 and 255 fills the channel.
    fn pack(self, v: u32) -> u32 {
        let size = u32::from(self.size);
        let bits = match size {
            0..=8 => v >> (8 - size),
            _ => (v << (size - 8)) | (v >> (16 - size)),
        };
        bits << self.shift
    }

    fn mask(self) -> u32 {
        (u32::MAX >> (32 - u32::from(self.size))) << self.shift
    }
}

/// A 32-bit pixel format: where red, green and blue go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    red: Channel,
    green: Channel,
    blue: Channel,
}

impl Format {
    /// `0x00RRGGBB`, which is what QEMU's GOP and virtio-gpu give, and what
    /// everything draws in.
    pub const NATIVE: Self = Self {
        red: Channel { size: 8, shift: 16 },
        green: Channel { size: 8, shift: 8 },
        blue: Channel { size: 8, shift: 0 },
    };

    /// The format for Limine's `(size, shift)` of each channel; None when one is
    /// empty, wider than 16 bits, past bit 31, or overlaps another.
    pub fn from_masks(red: (u8, u8), green: (u8, u8), blue: (u8, u8)) -> Option<Self> {
        let channel = |(size, shift): (u8, u8)| {
            ((1..=16).contains(&size) && u32::from(size) + u32::from(shift) <= 32)
                .then_some(Channel { size, shift })
        };
        let format = Self {
            red: channel(red)?,
            green: channel(green)?,
            blue: channel(blue)?,
        };
        let (r, g, b) = (format.red.mask(), format.green.mask(), format.blue.mask());
        (r & g == 0 && r & b == 0 && g & b == 0).then_some(format)
    }

    /// Whether `0x00RRGGBB` already is this format's pixel.
    pub fn is_native(self) -> bool {
        self == Self::NATIVE
    }

    /// The pixel for `rgb` (`0x00RRGGBB`); bits above 24 are ignored.
    pub fn pack(self, rgb: u32) -> u32 {
        self.red.pack((rgb >> 16) & 0xFF)
            | self.green.pack((rgb >> 8) & 0xFF)
            | self.blue.pack(rgb & 0xFF)
    }
}

#[cfg(test)]
mod tests {
    use super::Format;

    /// Limine's masks for the byte order R, G, B, X in memory (little endian:
    /// red in the low byte).
    fn rgbx() -> Format {
        Format::from_masks((8, 0), (8, 8), (8, 16)).unwrap()
    }

    /// B, G, R, X in memory: blue in the low byte, QEMU's.
    fn bgrx() -> Format {
        Format::from_masks((8, 16), (8, 8), (8, 0)).unwrap()
    }

    /// 2:10:10:10, red at the top: the format a 10-bit panel's GOP offers.
    fn rgb10() -> Format {
        Format::from_masks((10, 20), (10, 10), (10, 0)).unwrap()
    }

    #[test]
    fn bgrx_is_the_native_format_and_leaves_a_colour_alone() {
        assert!(bgrx().is_native());
        assert_eq!(bgrx(), Format::NATIVE);
        for rgb in [0, 0xFF_FFFF, 0x11_2233, 0xE8_5D2A, 0x0B_0F1A] {
            assert_eq!(bgrx().pack(rgb), rgb);
        }
        assert_eq!(
            bgrx().pack(0xFF11_2233),
            0x11_2233,
            "the top byte is dropped"
        );
    }

    #[test]
    fn rgbx_puts_red_in_the_low_byte() {
        assert!(!rgbx().is_native());
        assert_eq!(rgbx().pack(0x11_2233), 0x33_2211);
        assert_eq!(rgbx().pack(0xFF_0000), 0x00_00FF);
        assert_eq!(rgbx().pack(0x00_00FF), 0xFF_0000);
        assert_eq!(rgbx().pack(0xFF_FFFF), 0xFF_FFFF);
        assert_eq!(rgbx().pack(0), 0);
    }

    #[test]
    fn rgbx_and_bgrx_swap_red_and_blue_for_every_colour() {
        for rgb in (0..=0xFF_FFFFu32).step_by(0x1_0101 / 3 + 1) {
            let (r, g, b) = ((rgb >> 16) & 0xFF, (rgb >> 8) & 0xFF, rgb & 0xFF);
            assert_eq!(rgbx().pack(rgb), (b << 16) | (g << 8) | r, "{rgb:06x}");
            assert_eq!(bgrx().pack(rgb), rgb);
        }
    }

    #[test]
    fn ten_bits_a_channel_fill_the_channel_at_white_and_stay_empty_at_black() {
        assert_eq!(rgb10().pack(0xFF_FFFF), 0x3FFF_FFFF);
        assert_eq!(rgb10().pack(0), 0);
        // 0xFF in one channel is all ten bits of it, and only of it.
        assert_eq!(rgb10().pack(0xFF_0000), 0x3FF << 20);
        assert_eq!(rgb10().pack(0x00_FF00), 0x3FF << 10);
        assert_eq!(rgb10().pack(0x00_00FF), 0x3FF);
    }

    #[test]
    fn ten_bits_repeat_the_high_bits_so_mid_greys_stay_even() {
        // 0x80 -> 0b10_0000_0010: the top bits again below.
        assert_eq!(rgb10().pack(0x00_0080), 0b10_0000_0010);
        assert_eq!(rgb10().pack(0x00_0001), 0b00_0000_0100);
        // Each channel only grows with its value.
        let mut last = 0;
        for v in 0..=0xFF {
            let packed = rgb10().pack(v) & 0x3FF;
            assert!(packed >= last, "{v}");
            last = packed;
        }
    }

    #[test]
    fn a_channel_narrower_than_8_bits_drops_the_low_bits() {
        // RGB565-style masks in a 32-bit pixel.
        let f = Format::from_masks((5, 11), (6, 5), (5, 0)).unwrap();
        assert_eq!(f.pack(0xFF_FFFF), 0xFFFF);
        assert_eq!(f.pack(0xF8_0000), 0x1F << 11);
        assert_eq!(f.pack(0x00_00FC), 0x1F, "a blue of 0xFC is 31");
        assert_eq!(f.pack(0x00_0007), 0);
    }

    #[test]
    fn masks_that_make_no_pixel_are_refused() {
        // A channel of no bits, or one that runs off the pixel.
        assert_eq!(Format::from_masks((0, 16), (8, 8), (8, 0)), None);
        assert_eq!(Format::from_masks((8, 28), (8, 8), (8, 0)), None);
        // Wider than a channel's 8-bit source can fill sensibly.
        assert_eq!(Format::from_masks((17, 0), (8, 24), (7, 17)), None);
        // Two channels on the same bits.
        assert_eq!(Format::from_masks((8, 16), (8, 16), (8, 0)), None);
        assert_eq!(Format::from_masks((8, 4), (8, 8), (8, 0)), None);
        // No masks at all (a framebuffer that reports none).
        assert_eq!(Format::from_masks((0, 0), (0, 0), (0, 0)), None);
    }
}
