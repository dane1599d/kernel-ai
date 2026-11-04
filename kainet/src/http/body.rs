//! Chunked transfer coding (RFC 9112 §7.1), decoded as bytes arrive: a size
//! line in hex (with any extension ignored), the data, CRLF; a zero size,
//! then trailer lines up to a blank one. Every line has a cap, and a size
//! past 16 hex digits is refused, so a hostile server cannot make it hold or
//! count without end.

use super::HttpError;
use alloc::vec::Vec;

/// The longest size line, extensions included.
const MAX_SIZE_LINE: usize = 1024;
/// The most trailer bytes, all lines together.
const MAX_TRAILERS: usize = 8 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Reading a size line.
    Size,
    /// This many data bytes left in the chunk.
    Data(u64),
    /// The CRLF after a chunk's data.
    DataEnd,
    /// Trailer lines after the last chunk.
    Trailers,
    Done,
}

pub struct Chunked {
    state: State,
    line: Vec<u8>,
    trailers: usize,
}

impl Chunked {
    pub fn new() -> Self {
        Self {
            state: State::Size,
            line: Vec::new(),
            trailers: 0,
        }
    }

    /// Whether the last chunk and its trailers have come.
    pub fn done(&self) -> bool {
        self.state == State::Done
    }

    /// Decodes what it can of `input`, passing data to `out`; how many of
    /// `input`'s bytes it used. Bytes after the end are left unused.
    pub fn feed(
        &mut self,
        input: &[u8],
        out: &mut dyn FnMut(&[u8]) -> Result<(), HttpError>,
    ) -> Result<usize, HttpError> {
        let mut at = 0;
        while at < input.len() && self.state != State::Done {
            match self.state {
                State::Data(left) => {
                    let n = (input.len() - at).min(usize::try_from(left).unwrap_or(usize::MAX));
                    out(&input[at..at + n])?;
                    at += n;
                    let left = left - n as u64;
                    self.state = if left == 0 {
                        State::DataEnd
                    } else {
                        State::Data(left)
                    };
                }
                State::Size | State::DataEnd | State::Trailers => {
                    let byte = input[at];
                    at += 1;
                    self.line.push(byte);
                    let cap = match self.state {
                        State::Size => MAX_SIZE_LINE,
                        State::DataEnd => 2,
                        _ => MAX_TRAILERS - self.trailers,
                    };
                    if self.line.len() > cap {
                        return Err(HttpError::Body("a chunk line is too long"));
                    }
                    if byte == b'\n' {
                        self.line_done()?;
                    }
                }
                State::Done => {}
            }
        }
        Ok(at)
    }

    fn line_done(&mut self) -> Result<(), HttpError> {
        let line = core::mem::take(&mut self.line);
        let text = line
            .strip_suffix(b"\r\n")
            .ok_or(HttpError::Body("a chunk line does not end in CRLF"))?;
        self.state = match self.state {
            State::Size => match size(text)? {
                0 => State::Trailers,
                n => State::Data(n),
            },
            State::DataEnd if text.is_empty() => State::Size,
            State::DataEnd => return Err(HttpError::Body("a chunk's data runs past its size")),
            State::Trailers if text.is_empty() => State::Done,
            State::Trailers => {
                self.trailers += line.len();
                State::Trailers
            }
            state => state,
        };
        Ok(())
    }
}

/// A size line's size: 1 to 16 hex digits, then nothing or `;` and an
/// extension.
fn size(line: &[u8]) -> Result<u64, HttpError> {
    let digits = line.split(|&b| b == b';').next().unwrap_or_default();
    let digits = digits.strip_suffix(b" ").unwrap_or(digits);
    if digits.is_empty() || digits.len() > 16 || !digits.iter().all(u8::is_ascii_hexdigit) {
        return Err(HttpError::Body("a chunk size is not hex"));
    }
    let text =
        core::str::from_utf8(digits).map_err(|_| HttpError::Body("a chunk size is not hex"))?;
    u64::from_str_radix(text, 16).map_err(|_| HttpError::Body("a chunk size is not hex"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `input` decoded in pieces of `step` bytes: the data, and what was left.
    fn decode(input: &[u8], step: usize) -> Result<(Vec<u8>, usize), HttpError> {
        let mut chunked = Chunked::new();
        let mut data = Vec::new();
        let mut used = 0;
        for piece in input.chunks(step) {
            used += chunked.feed(piece, &mut |d| {
                data.extend_from_slice(d);
                Ok(())
            })?;
            if chunked.done() {
                break;
            }
        }
        assert!(chunked.done(), "not done");
        Ok((data, input.len() - used))
    }

    #[test]
    fn chunks_decode_in_any_pieces() {
        let input =
            b"5\r\nhello\r\n1;ext=\"x\"\r\n \r\nA\r\n0123456789\r\n0\r\nX-Sum: 1\r\n\r\nNEXT";
        for step in [1, 2, 3, 7, input.len()] {
            let (data, left) = decode(input, step).unwrap();
            assert_eq!(data, b"hello 0123456789", "step {step}");
            assert_eq!(left, 4, "step {step}");
        }
    }

    #[test]
    fn hostile_chunks_are_refused() {
        for input in [
            &b"g\r\nx\r\n0\r\n\r\n"[..],
            b"\r\n0\r\n\r\n",
            b"5\nhello\r\n0\r\n\r\n",
            b"5\r\nhelloX\r\n0\r\n\r\n",
            b"5\r\nhello\n0\r\n\r\n",
            b"0x5\r\nhello\r\n0\r\n\r\n",
            b"-5\r\nhello\r\n0\r\n\r\n",
            b"10000000000000000\r\n",
            b"0\r\nX: y\n\r\n",
        ] {
            let mut chunked = Chunked::new();
            let got = chunked.feed(input, &mut |_| Ok(()));
            assert!(got.is_err(), "{:?}", core::str::from_utf8(input));
        }
        let mut long = alloc::vec![b'1'; MAX_SIZE_LINE + 1];
        long.extend_from_slice(b"\r\n");
        assert!(Chunked::new().feed(&long, &mut |_| Ok(())).is_err());
        let mut trailers = b"0\r\n".to_vec();
        for _ in 0..1000 {
            trailers.extend_from_slice(b"X-Pad: aaaaaaaaaa\r\n");
        }
        assert!(Chunked::new().feed(&trailers, &mut |_| Ok(())).is_err());
    }

    #[test]
    fn the_largest_size_is_taken_but_not_one_digit_more() {
        let mut chunked = Chunked::new();
        let used = chunked
            .feed(b"ffffffffffffffff\r\nab", &mut |_| Ok(()))
            .unwrap();
        assert_eq!((used, chunked.state), (20, State::Data(u64::MAX - 2)));
    }
}
