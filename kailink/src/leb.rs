//! The LEB128 forms relocations patch: 5-byte padded unsigned and signed values,
//! as Linking.md requires, and the reads to split a code section.

use crate::Error;

/// Writes `value` as a 5-byte padded unsigned LEB128.
pub fn put_u32_padded(out: &mut [u8], value: u32) {
    let mut v = value;
    for byte in out.iter_mut().take(4) {
        *byte = (v as u8 & 0x7f) | 0x80;
        v >>= 7;
    }
    out[4] = v as u8 & 0x0f;
}

/// Writes `value` as a 5-byte padded signed LEB128.
pub fn put_i32_padded(out: &mut [u8], value: i32) {
    let mut v = value;
    for byte in out.iter_mut().take(4) {
        *byte = (v as u8 & 0x7f) | 0x80;
        v >>= 7;
    }
    out[4] = v as u8 & 0x7f;
}

/// Reads an unsigned LEB128 `u32` at `*at`, moving past it.
pub fn read_u32(bytes: &[u8], at: &mut usize) -> Result<u32, Error> {
    let mut value = 0u32;
    for shift in (0..35).step_by(7) {
        let byte = *bytes
            .get(*at)
            .ok_or_else(|| Error::Parse("truncated LEB128".into()))?;
        *at += 1;
        if shift == 28 && byte & 0x70 != 0 {
            return Err(Error::Parse("LEB128 overflows u32".into()));
        }
        value |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(Error::Parse("LEB128 too long".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padded_round_trips() {
        for value in [0u32, 1, 127, 128, 0x3fff, 0x4000, u32::MAX] {
            let mut buf = [0u8; 5];
            put_u32_padded(&mut buf, value);
            let mut at = 0;
            assert_eq!(read_u32(&buf, &mut at), Ok(value));
            assert_eq!(at, 5);
        }
        let mut buf = [0u8; 5];
        put_i32_padded(&mut buf, -1);
        assert_eq!(buf, [0xff, 0xff, 0xff, 0xff, 0x7f]);
        put_i32_padded(&mut buf, 64);
        assert_eq!(buf, [0xc0, 0x80, 0x80, 0x80, 0x00]);
    }

    #[test]
    fn bad_leb_is_refused() {
        let mut at = 0;
        assert!(read_u32(&[0x80, 0x80], &mut at).is_err());
        let mut at = 0;
        assert!(read_u32(&[0xff, 0xff, 0xff, 0xff, 0x7f], &mut at).is_err());
    }
}
