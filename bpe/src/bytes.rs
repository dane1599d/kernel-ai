//! GPT-2's byte-level alphabet: every byte stands for one printable char, so token
//! strings never hold a raw space or control byte. Printable Latin-1 bytes stand
//! for themselves; the other 68 take the chars from U+0100 up, in byte order.

const fn printable(b: u8) -> bool {
    matches!(b, b'!'..=b'~' | 0xa1..=0xac | 0xae..=0xff)
}

/// How many bytes do not stand for themselves.
const SHIFTED: usize = 68;

const TO_CHAR: [u32; 256] = {
    let mut table = [0; 256];
    let (mut b, mut next) = (0, 0x100);
    while b < 256 {
        table[b] = if printable(b as u8) {
            b as u32
        } else {
            next += 1;
            next - 1
        };
        b += 1;
    }
    table
};

/// The byte behind each char from U+0100 on.
const FROM_SHIFTED: [u8; SHIFTED] = {
    let mut table = [0; SHIFTED];
    let mut b = 0;
    while b < 256 {
        if !printable(b as u8) {
            table[(TO_CHAR[b] - 0x100) as usize] = b as u8;
        }
        b += 1;
    }
    table
};

/// The char that stands for byte `b`.
pub fn to_char(b: u8) -> char {
    char::from_u32(TO_CHAR[b as usize]).unwrap_or(char::REPLACEMENT_CHARACTER)
}

/// The byte that `c` stands for, if any.
pub fn from_char(c: char) -> Option<u8> {
    let code = c as u32;
    match u8::try_from(code) {
        Ok(b) => printable(b).then_some(b),
        Err(_) => FROM_SHIFTED.get(code.checked_sub(0x100)? as usize).copied(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_map_is_gpt2s() {
        assert_eq!(to_char(b' '), '\u{120}');
        assert_eq!(to_char(b'\n'), '\u{10a}');
        assert_eq!(to_char(0), '\u{100}');
        assert_eq!(to_char(0x7f), '\u{121}');
        assert_eq!(to_char(0xa0), '\u{142}');
        assert_eq!(to_char(0xad), '\u{143}');
        assert_eq!(to_char(b'a'), 'a');
        assert_eq!(to_char(0xe9), '\u{e9}');
        for b in 0..=255u8 {
            assert_eq!(from_char(to_char(b)), Some(b));
        }
        assert_eq!(from_char(' '), None);
        assert_eq!(from_char('\u{ad}'), None);
        assert_eq!(from_char('\u{144}'), None);
        assert_eq!(from_char('\u{4e2d}'), None);
    }
}
