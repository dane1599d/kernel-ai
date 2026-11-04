//! `kai-index 1`, the index file on the `index` volume (ADR 0013 section 4).
//!
//! ```text
//! "kai-index 1\n"                      the magic, which names the format's version
//! frame: header                        which embedder made its vectors, if any
//! frame: file | chunk | remove ...     one batch
//! frame: commit <generation> <frames>  closes it
//! ...                                  more batches, each closed the same way
//! ```
//!
//! A frame is `len: u32`, `crc32: u32` (IEEE, of the payload) and the payload,
//! whose first byte is its kind; numbers are little-endian. A batch is applied
//! only when its commit frame is read and counts its frames, so a crash in the
//! middle of an append loses that batch alone: the reader stops at the first
//! frame that is cut short or fails its checksum, keeps what was committed
//! before it ([`Opened::committed`]), and the writer appends from there. The
//! batch lost is found again by `changes(since)` from the generation the last
//! commit marked. Each batch also commits in one kaifs transaction, as the ADR
//! has it; the framing makes the file safe however it is written.
//!
//! Anything else wrong is a reason to rebuild ([`Refused`]), never to read on:
//! another magic or embedder, or a frame that passed its checksum but does not
//! decode or does not hold. The index is derived data, so a rebuild loses
//! nothing.

use crate::embed::{EmbedderId, MAX_DIMS, MAX_NAME};
use crate::index::{ChunkEntry, Index};
use crate::scope::{self, MAX_PATH};
use crate::terms::{MAX_TERM, MAX_TERMS};
use alloc::string::String;
use alloc::vec::Vec;

/// The file's first bytes: the format and its version.
pub const MAGIC: &[u8] = b"kai-index 1\n";
/// The largest payload one frame carries.
pub const MAX_FRAME: usize = 64 << 10;
/// Most frames one batch holds before its commit.
pub const MAX_BATCH: usize = 8192;

const HEADER: u8 = 1;
const FILE: u8 = 2;
const CHUNK: u8 = 3;
const REMOVE: u8 = 4;
const COMMIT: u8 = 5;

/// What the header says: the embedder whose vectors the index holds, or none
/// for an index searched by keyword alone.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Header {
    pub embedder: Option<EmbedderId>,
}

/// One change to the index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Record {
    /// A file, replacing any entry for its object id and its chunks.
    File {
        object: u64,
        path: String,
        generation: u64,
        modified_ns: u64,
        length: u64,
    },
    /// One chunk of a file a record before it named.
    Chunk { object: u64, chunk: ChunkEntry },
    /// A file no longer indexed: deleted, moved out, excluded or not text.
    Remove { object: u64 },
}

/// Why an index file is not read: each means rebuild.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// It does not start with [`MAGIC`]: another format or version, or empty.
    Magic,
    /// Its header frame is missing or unreadable.
    Header,
    /// Its vectors are another embedder's, or it has none where one is wanted.
    Embedder,
    /// A committed frame does not decode or does not hold.
    Corrupt(&'static str),
}

impl Refused {
    pub fn why(self) -> &'static str {
        match self {
            Self::Magic => "not a kai-index 1 file",
            Self::Header => "its header cannot be read",
            Self::Embedder => "its vectors are another embedder's",
            Self::Corrupt(why) => why,
        }
    }
}

/// An index file read back.
#[derive(Clone, Debug, PartialEq)]
pub struct Opened {
    pub index: Index,
    /// Bytes of the file up to the end of its last commit: the next batch is
    /// appended here, over any torn tail.
    pub committed: usize,
    /// Whether anything followed the last commit: a batch a crash cut short.
    pub torn: bool,
}

/// A new index file: the magic and the header frame. A header the reader would
/// refuse (an embedder name over [`MAX_NAME`] bytes, or no dimensions or too
/// many) is refused here, so a fresh index is never rebuilt again at once.
pub fn fresh(header: &Header) -> Result<Vec<u8>, Refused> {
    let mut out = MAGIC.to_vec();
    let mut p = Out::new(HEADER);
    match &header.embedder {
        Some(e) => {
            p.str8(&e.name);
            p.bytes(&e.sha256);
            p.u16(e.dims);
        }
        None => {
            p.str8("");
            p.bytes(&[0; 32]);
            p.u16(0);
        }
    }
    if decode_header(&p.0).as_ref() != Some(header) {
        return Err(Refused::Header);
    }
    frame(&mut out, &p.0);
    Ok(out)
}

/// Records to append, and the commit that closes them.
#[derive(Clone, Debug, Default)]
pub struct Batch {
    bytes: Vec<u8>,
    frames: u32,
}

impl Batch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `record`, or refuses one the reader would not read back as it is
    /// (a path that is not clean, a term out of shape, too many of anything),
    /// so a batch written is a batch read.
    pub fn push(&mut self, record: &Record) -> Result<(), Refused> {
        let payload = encode(record);
        if payload.len() > MAX_FRAME || decode(&payload).as_ref() != Some(record) {
            return Err(Refused::Corrupt("a record the index cannot keep"));
        }
        if self.frames as usize >= MAX_BATCH {
            return Err(Refused::Corrupt("a batch of too many frames"));
        }
        frame(&mut self.bytes, &payload);
        self.frames += 1;
        Ok(())
    }

    /// Frames in the batch so far.
    pub fn len(&self) -> usize {
        self.frames as usize
    }

    pub fn is_empty(&self) -> bool {
        self.frames == 0
    }

    /// The bytes to append at [`Opened::committed`], closed by a commit that
    /// marks `generation`.
    pub fn commit(mut self, generation: u64) -> Vec<u8> {
        let mut p = Out::new(COMMIT);
        p.u64(generation);
        p.u32(self.frames);
        frame(&mut self.bytes, &p.0);
        self.bytes
    }
}

/// Reads `bytes`, an index file, for an index whose header is `want`: its
/// committed batches applied in order, or why it must be rebuilt.
pub fn open(bytes: &[u8], want: &Header) -> Result<Opened, Refused> {
    let rest = bytes.strip_prefix(MAGIC).ok_or(Refused::Magic)?;
    let mut at = MAGIC.len();
    let (payload, next) = next_frame(rest).ok_or(Refused::Header)?;
    let header = decode_header(payload).ok_or(Refused::Header)?;
    if header != *want {
        return Err(Refused::Embedder);
    }
    at += next;
    let mut index = Index::new(header);
    let mut committed = at;
    let mut pending: Vec<Record> = Vec::new();
    while let Some((payload, len)) = next_frame(&bytes[at..]) {
        at += len;
        match payload.first() {
            Some(&COMMIT) => {
                let (generation, frames) = decode_commit(payload)
                    .ok_or(Refused::Corrupt("a commit that does not decode"))?;
                if frames as usize != pending.len() {
                    return Err(Refused::Corrupt("a commit that miscounts its batch"));
                }
                if generation < index.generation {
                    return Err(Refused::Corrupt("a commit that goes back in generation"));
                }
                for record in pending.drain(..) {
                    index.apply(record).map_err(Refused::Corrupt)?;
                }
                index.generation = generation;
                committed = at;
            }
            _ => {
                let record =
                    decode(payload).ok_or(Refused::Corrupt("a frame that does not decode"))?;
                if pending.len() >= MAX_BATCH {
                    return Err(Refused::Corrupt("a batch of too many frames"));
                }
                pending.push(record);
            }
        }
    }
    Ok(Opened {
        index,
        committed,
        torn: committed < bytes.len(),
    })
}

/// The first frame of `bytes` and its whole length, or `None` if it is cut
/// short, too large, empty or fails its checksum.
fn next_frame(bytes: &[u8]) -> Option<(&[u8], usize)> {
    let len = u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?) as usize;
    let crc = u32::from_le_bytes(bytes.get(4..8)?.try_into().ok()?);
    if len == 0 || len > MAX_FRAME {
        return None;
    }
    let payload = bytes.get(8..8 + len)?;
    (diskmap::crc32(payload) == crc).then_some((payload, 8 + len))
}

fn frame(out: &mut Vec<u8>, payload: &[u8]) {
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&diskmap::crc32(payload).to_le_bytes());
    out.extend_from_slice(payload);
}

/// A payload being written.
struct Out(Vec<u8>);

impl Out {
    fn new(kind: u8) -> Self {
        Self(alloc::vec![kind])
    }
    fn bytes(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
    fn u16(&mut self, n: u16) {
        self.bytes(&n.to_le_bytes());
    }
    fn u32(&mut self, n: u32) {
        self.bytes(&n.to_le_bytes());
    }
    fn u64(&mut self, n: u64) {
        self.bytes(&n.to_le_bytes());
    }
    fn str8(&mut self, s: &str) {
        self.0.push(s.len() as u8);
        self.bytes(s.as_bytes());
    }
    fn str16(&mut self, s: &str) {
        self.u16(s.len() as u16);
        self.bytes(s.as_bytes());
    }
}

/// A payload being read: each getter is `None` past its end.
struct In<'a>(&'a [u8]);

impl<'a> In<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let (head, rest) = (self.0.get(..n)?, self.0.get(n..)?);
        self.0 = rest;
        Some(head)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn text(&mut self, len: usize) -> Option<String> {
        core::str::from_utf8(self.take(len)?).ok().map(String::from)
    }
    /// The payload had nothing more.
    fn done(&self) -> Option<()> {
        self.0.is_empty().then_some(())
    }
}

fn encode(record: &Record) -> Vec<u8> {
    match record {
        Record::File {
            object,
            path,
            generation,
            modified_ns,
            length,
        } => {
            let mut p = Out::new(FILE);
            p.u64(*object);
            p.u64(*generation);
            p.u64(*modified_ns);
            p.u64(*length);
            p.str16(path);
            p.0
        }
        Record::Chunk { object, chunk } => {
            let mut p = Out::new(CHUNK);
            p.u64(*object);
            p.u64(chunk.offset);
            p.u32(chunk.len);
            p.u16(chunk.terms.len() as u16);
            for (term, count) in &chunk.terms {
                p.str8(term);
                p.u16(*count);
            }
            let vector = chunk.vector.as_deref().unwrap_or(&[]);
            p.u16(vector.len() as u16);
            for h in vector {
                p.u16(*h);
            }
            p.0
        }
        Record::Remove { object } => {
            let mut p = Out::new(REMOVE);
            p.u64(*object);
            p.0
        }
    }
}

fn decode_header(payload: &[u8]) -> Option<Header> {
    let mut p = In(payload);
    (p.u8()? == HEADER).then_some(())?;
    let name_len = usize::from(p.u8()?);
    let name = p.text(name_len)?;
    let sha256: [u8; 32] = p.take(32)?.try_into().ok()?;
    let dims = p.u16()?;
    p.done()?;
    if name.is_empty() {
        return (sha256 == [0; 32] && dims == 0).then_some(Header { embedder: None });
    }
    let fits = name.len() <= MAX_NAME && dims > 0 && usize::from(dims) <= MAX_DIMS;
    fits.then_some(Header {
        embedder: Some(EmbedderId { name, sha256, dims }),
    })
}

fn decode_commit(payload: &[u8]) -> Option<(u64, u32)> {
    let mut p = In(payload);
    (p.u8()? == COMMIT).then_some(())?;
    let generation = p.u64()?;
    let frames = p.u32()?;
    p.done()?;
    Some((generation, frames))
}

/// A record's payload read back, or `None` if any field is out of shape.
fn decode(payload: &[u8]) -> Option<Record> {
    let mut p = In(payload);
    let record = match p.u8()? {
        FILE => {
            let object = p.u64()?;
            let generation = p.u64()?;
            let modified_ns = p.u64()?;
            let length = p.u64()?;
            let len = usize::from(p.u16()?);
            let path = p.text(len)?;
            (len <= MAX_PATH && scope::clean(&path)).then_some(())?;
            Record::File {
                object,
                path,
                generation,
                modified_ns,
                length,
            }
        }
        CHUNK => {
            let object = p.u64()?;
            let offset = p.u64()?;
            let len = p.u32()?;
            let n = usize::from(p.u16()?);
            (n <= MAX_TERMS).then_some(())?;
            let mut terms: Vec<(String, u16)> = Vec::with_capacity(n);
            for _ in 0..n {
                let tlen = usize::from(p.u8()?);
                let term = p.text(tlen)?;
                let count = p.u16()?;
                // Terms are sorted and distinct, each 2 to MAX_TERM bytes and
                // counted at least once, as `terms::counted` makes them.
                let ordered = terms.last().is_none_or(|(last, _)| *last < term);
                ((2..=MAX_TERM).contains(&tlen) && count > 0 && ordered).then_some(())?;
                terms.push((term, count));
            }
            let dims = usize::from(p.u16()?);
            (dims <= MAX_DIMS).then_some(())?;
            let mut vector = Vec::with_capacity(dims);
            for _ in 0..dims {
                vector.push(p.u16()?);
            }
            Record::Chunk {
                object,
                chunk: ChunkEntry {
                    offset,
                    len,
                    terms,
                    vector: (dims > 0).then_some(vector),
                },
            }
        }
        REMOVE => Record::Remove { object: p.u64()? },
        _ => return None,
    };
    p.done()?;
    Some(record)
}
