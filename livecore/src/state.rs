//! The header of a state, CBOR `[schema, body]`, read without a CBOR library so
//! the kernel and the host can check which schema a state is in.

/// The schema in a state's header: a two-element array (`0x82`), then the schema
/// as an unsigned integer. `None` for anything else.
pub fn schema_of(state: &[u8]) -> Option<u64> {
    let (&array, rest) = state.split_first()?;
    if array != 0x82 {
        return None;
    }
    let (&head, rest) = rest.split_first()?;
    let (major, info) = (head >> 5, head & 0x1f);
    if major != 0 {
        return None;
    }
    let width = match info {
        0..=23 => return Some(u64::from(info)),
        24 => 1,
        25 => 2,
        26 => 4,
        27 => 8,
        _ => return None,
    };
    let bytes = rest.get(..width)?;
    Some(
        bytes
            .iter()
            .fold(0, |value, &b| (value << 8) | u64::from(b)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_unsigned_form_is_read() {
        assert_eq!(schema_of(&[0x82, 0x02, 0x80]), Some(2));
        assert_eq!(schema_of(&[0x82, 0x17]), Some(23));
        assert_eq!(schema_of(&[0x82, 0x18, 24]), Some(24));
        assert_eq!(schema_of(&[0x82, 0x19, 0x01, 0x2c]), Some(300));
        assert_eq!(schema_of(&[0x82, 0x1a, 0, 1, 0, 0]), Some(65536));
        assert_eq!(
            schema_of(&[0x82, 0x1b, 0, 0, 0, 1, 0, 0, 0, 0]),
            Some(1 << 32)
        );
    }

    #[test]
    fn anything_else_is_refused() {
        for bad in [
            &[][..],
            &[0x82],
            &[0x81, 0x02],
            &[0x83, 0x02],
            &[0x82, 0x22],
            &[0x82, 0x19, 0x01],
            &[0x82, 0x1c],
            b"hello",
        ] {
            assert_eq!(schema_of(bad), None, "{bad:?}");
        }
    }
}
