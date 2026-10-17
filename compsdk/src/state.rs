//! State as CBOR `[schema, body]`, the form every version reads and writes, so the
//! host and kernel can check which schema a state is in without decoding it.

use alloc::vec::Vec;
use minicbor::{Decode, Decoder, Encode, Encoder};

/// `body` written in `schema`, as CBOR `[schema, body]`.
pub fn encode<T: Encode<()>>(schema: u32, body: &T) -> Vec<u8> {
    let mut encoder = Encoder::new(Vec::new());
    // Writing into a Vec cannot fail.
    let _ = encoder
        .array(2)
        .and_then(|e| e.u32(schema))
        .and_then(|e| e.encode(body));
    encoder.into_writer()
}

/// The schema a state says it is in.
pub fn schema_of(state: &[u8]) -> Option<u32> {
    let mut decoder = Decoder::new(state);
    if decoder.array().ok()? != Some(2) {
        return None;
    }
    decoder.u32().ok()
}

/// The body of a state written in `schema`; `None` if it is in another schema, is
/// malformed, or has anything after the body.
pub fn decode<'b, T: Decode<'b, ()>>(state: &'b [u8], schema: u32) -> Option<T> {
    let mut decoder = Decoder::new(state);
    if decoder.array().ok()? != Some(2) || decoder.u32().ok()? != schema {
        return None;
    }
    let body = decoder.decode().ok()?;
    (decoder.position() == state.len()).then_some(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;
    use alloc::vec;

    #[derive(Debug, PartialEq, Encode, Decode)]
    struct Body {
        #[n(0)]
        texts: Vec<String>,
        #[n(1)]
        next: u32,
    }

    fn body() -> Body {
        Body {
            texts: vec![String::from("buy milk")],
            next: 7,
        }
    }

    #[test]
    fn a_state_round_trips_and_says_its_schema() {
        let state = encode(2, &body());
        assert_eq!(schema_of(&state), Some(2));
        assert_eq!(decode::<Body>(&state, 2), Some(body()));
        // The header is a two-element array, then the schema as an unsigned integer.
        assert_eq!(&state[..2], &[0x82, 0x02]);
    }

    #[test]
    fn another_schema_trailing_bytes_or_garbage_are_refused() {
        let state = encode(2, &body());
        assert_eq!(decode::<Body>(&state, 1), None);
        let mut longer = state.clone();
        longer.push(0);
        assert_eq!(decode::<Body>(&longer, 2), None);
        assert_eq!(schema_of(b"hello"), None);
        assert_eq!(schema_of(&[]), None);
        assert_eq!(schema_of(&encode(300, &body())), Some(300));
    }
}
