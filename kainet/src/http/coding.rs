//! A response body's content coding undone as its bytes arrive (RFC 9110
//! §8.4): gzip (RFC 1952: its header, raw deflate through miniz_oxide, then
//! its CRC-32 and length checked) and brotli (RFC 7932, through
//! brotli-decompressor, never its large-window form). Each decoder holds a
//! fixed window (32 KiB for deflate, at most 16 MiB for brotli) and writes
//! out through a fixed buffer, so a small hostile body that inflates without
//! end costs time bounded by the caller's limit on decoded bytes, never
//! memory.

use super::HttpError;
use super::head::Coding;
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use brotli_decompressor::{
    Allocator, BrotliDecompressStream, BrotliResult, BrotliState, SliceWrapper, SliceWrapperMut,
};
use miniz_oxide::inflate::stream::{InflateState, inflate};
use miniz_oxide::{DataFormat, MZError, MZFlush, MZStatus};

/// Decoded bytes written out at a time.
const OUT: usize = 32 * 1024;
/// The longest gzip header taken: its optional name, comment and extra field
/// together.
pub const MAX_GZIP_HEADER: usize = 4096;

/// Undoes one coding, fed the body's bytes in any pieces.
pub struct Decoder {
    kind: Kind,
    out: Vec<u8>,
    /// Decoded bytes passed on.
    decoded: u64,
    /// The most decoded bytes passed on.
    max: u64,
}

enum Kind {
    Identity,
    Gzip(Box<Gzip>),
    Brotli(Box<Brotli>),
}

impl Decoder {
    /// A decoder for `coding` that passes on at most `max` decoded bytes.
    pub fn new(coding: Coding, max: u64) -> Self {
        let kind = match coding {
            Coding::Identity => Kind::Identity,
            Coding::Gzip => Kind::Gzip(Box::new(Gzip::new())),
            Coding::Brotli => Kind::Brotli(Box::new(Brotli::new())),
        };
        let out = match kind {
            Kind::Identity => Vec::new(),
            _ => vec![0; OUT],
        };
        Self {
            kind,
            out,
            decoded: 0,
            max,
        }
    }

    /// Decoded bytes passed on so far.
    pub fn decoded(&self) -> u64 {
        self.decoded
    }

    /// Decodes `input`, passing what it gives to `sink`.
    pub fn feed(
        &mut self,
        input: &[u8],
        sink: &mut dyn FnMut(&[u8]) -> Result<(), HttpError>,
    ) -> Result<(), HttpError> {
        let (decoded, max) = (&mut self.decoded, self.max);
        let mut pass = |bytes: &[u8]| -> Result<(), HttpError> {
            if bytes.is_empty() {
                return Ok(());
            }
            *decoded += bytes.len() as u64;
            if *decoded > max {
                return Err(HttpError::TooLarge);
            }
            sink(bytes)
        };
        match &mut self.kind {
            Kind::Identity => pass(input),
            Kind::Gzip(gzip) => gzip.feed(input, &mut self.out, &mut pass),
            Kind::Brotli(brotli) => brotli.feed(input, &mut self.out, &mut pass),
        }
    }

    /// Whether the coded stream ended where the body did. A body of no bytes
    /// at all is taken as empty whatever its coding says, as browsers take it.
    pub fn finish(&self, body_bytes: u64) -> Result<(), HttpError> {
        let done = match &self.kind {
            Kind::Identity => true,
            Kind::Gzip(gzip) => gzip.state == GzipState::Done,
            Kind::Brotli(brotli) => brotli.done,
        };
        match done || body_bytes == 0 {
            true => Ok(()),
            false => Err(HttpError::Body("a compressed body ends inside its stream")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GzipState {
    /// The fixed ten bytes, then the optional fields `flags` names.
    Header,
    Deflate,
    /// CRC-32 and ISIZE, eight bytes.
    Trailer,
    Done,
}

/// RFC 1952's flag bits.
const FHCRC: u8 = 0x02;
const FEXTRA: u8 = 0x04;
const FNAME: u8 = 0x08;
const FCOMMENT: u8 = 0x10;
const RESERVED: u8 = 0xE0;

struct Gzip {
    state: GzipState,
    /// The header or trailer bytes so far.
    held: Vec<u8>,
    inflate: Box<InflateState>,
    crc: crc32fast::Hasher,
    /// Decoded bytes, mod 2^32, for ISIZE.
    size: u32,
}

impl Gzip {
    fn new() -> Self {
        Self {
            state: GzipState::Header,
            held: Vec::new(),
            inflate: InflateState::new_boxed(DataFormat::Raw),
            crc: crc32fast::Hasher::new(),
            size: 0,
        }
    }

    fn feed(
        &mut self,
        mut input: &[u8],
        out: &mut [u8],
        pass: &mut dyn FnMut(&[u8]) -> Result<(), HttpError>,
    ) -> Result<(), HttpError> {
        while !input.is_empty() {
            match self.state {
                GzipState::Header => {
                    let byte = input[0];
                    input = &input[1..];
                    self.held.push(byte);
                    if self.held.len() > MAX_GZIP_HEADER {
                        return Err(HttpError::Body("a gzip header is too long"));
                    }
                    if gzip_header_ends(&self.held)? {
                        self.held.clear();
                        self.state = GzipState::Deflate;
                    }
                }
                GzipState::Deflate => {
                    let used = self.inflate(input, out, pass)?;
                    input = &input[used..];
                }
                GzipState::Trailer => {
                    let take = (8 - self.held.len()).min(input.len());
                    self.held.extend_from_slice(&input[..take]);
                    input = &input[take..];
                    if self.held.len() == 8 {
                        let word = |at: usize| {
                            u32::from_le_bytes([
                                self.held[at],
                                self.held[at + 1],
                                self.held[at + 2],
                                self.held[at + 3],
                            ])
                        };
                        if word(0) != self.crc.clone().finalize() {
                            return Err(HttpError::Body("a gzip body fails its CRC"));
                        }
                        if word(4) != self.size {
                            return Err(HttpError::Body("a gzip body is not its stated size"));
                        }
                        self.state = GzipState::Done;
                    }
                }
                GzipState::Done => {
                    return Err(HttpError::Body("bytes after the gzip stream"));
                }
            }
        }
        Ok(())
    }

    /// Inflates what it can of `input`; how many bytes it used.
    fn inflate(
        &mut self,
        input: &[u8],
        out: &mut [u8],
        pass: &mut dyn FnMut(&[u8]) -> Result<(), HttpError>,
    ) -> Result<usize, HttpError> {
        let mut used = 0;
        loop {
            let result = inflate(&mut self.inflate, &input[used..], out, MZFlush::None);
            used += result.bytes_consumed;
            let written = &out[..result.bytes_written];
            self.crc.update(written);
            self.size = self.size.wrapping_add(written.len() as u32);
            pass(written)?;
            match result.status {
                Ok(MZStatus::StreamEnd) => {
                    self.state = GzipState::Trailer;
                    return Ok(used);
                }
                Ok(_) if used == input.len() && result.bytes_written < out.len() => {
                    return Ok(used);
                }
                Ok(_) if result.bytes_consumed == 0 && result.bytes_written == 0 => {
                    return Err(HttpError::Body("a deflate stream makes no progress"));
                }
                Ok(_) => {}
                // Waiting for input, with all it had written out.
                Err(MZError::Buf) if used == input.len() => return Ok(used),
                Err(_) => return Err(HttpError::Body("a gzip body does not inflate")),
            }
        }
    }
}

/// Whether `held`, read from a gzip stream's first byte, is its whole header;
/// an error if it cannot be one.
fn gzip_header_ends(held: &[u8]) -> Result<bool, HttpError> {
    const FIXED: usize = 10;
    match held.len() {
        1 if held[0] != 0x1F => return Err(HttpError::Body("not a gzip stream")),
        2 if held[1] != 0x8B => return Err(HttpError::Body("not a gzip stream")),
        3 if held[2] != 8 => return Err(HttpError::Body("a gzip method other than deflate")),
        4 if held[3] & RESERVED != 0 => return Err(HttpError::Body("a gzip flag is reserved")),
        n if n < FIXED => return Ok(false),
        _ => {}
    }
    let flags = held[3];
    let mut at = FIXED;
    if flags & FEXTRA != 0 {
        let Some(len) = held.get(at..at + 2) else {
            return Ok(false);
        };
        at += 2 + usize::from(u16::from_le_bytes([len[0], len[1]]));
        if held.len() < at {
            return Ok(false);
        }
    }
    for flag in [FNAME, FCOMMENT] {
        if flags & flag != 0 {
            match held[at..].iter().position(|&b| b == 0) {
                Some(end) => at += end + 1,
                None => return Ok(false),
            }
        }
    }
    if flags & FHCRC != 0 {
        at += 2;
        if held.len() < at {
            return Ok(false);
        }
        let crc = crc32fast::hash(&held[..at - 2]) as u16;
        if crc.to_le_bytes() != held[at - 2..at] {
            return Err(HttpError::Body("a gzip header fails its CRC"));
        }
    }
    Ok(held.len() == at)
}

/// The brotli decoder's memory, from the heap: brotli-decompressor asks for
/// every table and its window through these.
#[derive(Default)]
struct Heap;

/// One allocation of the brotli decoder.
#[derive(Default)]
struct Block<T>(Box<[T]>);

impl<T> SliceWrapper<T> for Block<T> {
    fn slice(&self) -> &[T] {
        &self.0
    }
}

impl<T> SliceWrapperMut<T> for Block<T> {
    fn slice_mut(&mut self) -> &mut [T] {
        &mut self.0
    }
}

impl<T: Clone + Default> Allocator<T> for Heap {
    type AllocatedMemory = Block<T>;

    fn alloc_cell(&mut self, len: usize) -> Block<T> {
        Block(vec![T::default(); len].into_boxed_slice())
    }

    fn free_cell(&mut self, _block: Block<T>) {}
}

struct Brotli {
    state: BrotliState<Heap, Heap, Heap>,
    done: bool,
}

impl Brotli {
    fn new() -> Self {
        Self {
            // Strict: RFC 7932's windows only, at most 16 MiB, never the
            // large-window form a server could ask 1 GiB with.
            state: BrotliState::new_strict(Heap, Heap, Heap),
            done: false,
        }
    }

    fn feed(
        &mut self,
        input: &[u8],
        out: &mut [u8],
        pass: &mut dyn FnMut(&[u8]) -> Result<(), HttpError>,
    ) -> Result<(), HttpError> {
        if self.done {
            return match input.is_empty() {
                true => Ok(()),
                false => Err(HttpError::Body("bytes after the brotli stream")),
            };
        }
        let (mut in_left, mut in_at) = (input.len(), 0);
        let mut total = 0;
        loop {
            let (mut out_left, mut out_at) = (out.len(), 0);
            let result = BrotliDecompressStream(
                &mut in_left,
                &mut in_at,
                input,
                &mut out_left,
                &mut out_at,
                out,
                &mut total,
                &mut self.state,
            );
            pass(&out[..out_at])?;
            match result {
                BrotliResult::ResultSuccess => {
                    self.done = true;
                    return match in_left {
                        0 => Ok(()),
                        _ => Err(HttpError::Body("bytes after the brotli stream")),
                    };
                }
                BrotliResult::NeedsMoreInput => return Ok(()),
                BrotliResult::NeedsMoreOutput => {}
                BrotliResult::ResultFailure => {
                    return Err(HttpError::Body("a brotli body does not decode"));
                }
            }
        }
    }
}
