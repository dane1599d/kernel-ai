//! One look at the machine, as `kai:system` carries it (P4 task 9): the CPUs,
//! memory and processes the kernel's `METRICS` reports, and the model's state
//! from llm's `health`. comphost writes a [`Sample`] with [`Sample::encode`] and
//! the System monitor reads it with [`Sample::parse`]; this crate is the one
//! place that knows the bytes.
//!
//! The format is little-endian numbers and length-prefixed names, with no text to
//! parse. Parsing is strict about shape (the magic, every length, no byte left
//! over) and takes no view of the numbers, which only change what the window
//! shows:
//!
//! ```text
//! "KSY1"
//! u64 uptime_ns   u32 cpus   u64 free_pages   u64 total_pages   u64 heap_bytes
//! u32 omitted     u8 model state   u32 tps10   u16 process count
//! per process:    u32 pid   u64 pages   u64 cpu_ns   u8 name length   name
//! ```

#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

/// What starts a sample; the digit is the format's version.
pub const MAGIC: [u8; 4] = *b"KSY1";
/// Most bytes a sample has; a larger one is refused.
pub const MAX_BYTES: usize = 32 * 1024;
/// Most processes a sample lists.
pub const MAX_PROCS: usize = 256;
/// Longest process name, in bytes.
pub const MAX_NAME: usize = 64;
/// Bytes in a page.
pub const PAGE: u64 = 4096;

/// What the model is doing, as llm's `health` reports it
/// (`llmproto::health::State`); `Unknown` when the host could not ask.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelState {
    Unknown,
    Waiting,
    Warming,
    Ready,
    Remote,
    Failed,
    NoModel,
    /// `health` did not answer in time: loading, or answering a request.
    Busy,
}

impl ModelState {
    /// The byte a state is written as.
    pub const fn code(self) -> u8 {
        match self {
            ModelState::Unknown => 0,
            ModelState::Waiting => 1,
            ModelState::Warming => 2,
            ModelState::Ready => 3,
            ModelState::Remote => 4,
            ModelState::Failed => 5,
            ModelState::NoModel => 6,
            ModelState::Busy => 7,
        }
    }

    pub const fn from_code(code: u8) -> Option<ModelState> {
        Some(match code {
            0 => ModelState::Unknown,
            1 => ModelState::Waiting,
            2 => ModelState::Warming,
            3 => ModelState::Ready,
            4 => ModelState::Remote,
            5 => ModelState::Failed,
            6 => ModelState::NoModel,
            7 => ModelState::Busy,
            _ => return None,
        })
    }
}

/// The model's state and the last answer's decode speed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelStatus {
    pub state: ModelState,
    /// Tokens a second times ten; 0 before the first answer, and for a server.
    pub tps10: u32,
}

impl ModelStatus {
    pub const UNKNOWN: Self = Self {
        state: ModelState::Unknown,
        tps10: 0,
    };
}

/// One process in a sample (`kabi::metrics::ProcessInfo`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcSample {
    pub pid: u32,
    pub name: String,
    /// Pages counted against its quota.
    pub pages: u64,
    /// CPU time its threads have used since it started, in nanoseconds.
    pub cpu_ns: u64,
}

/// One sample (`kabi::metrics::Metrics`, and the model's status).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sample {
    /// Nanoseconds since boot when the kernel took it.
    pub uptime_ns: u64,
    pub cpus: u32,
    pub free_pages: u64,
    pub total_pages: u64,
    pub heap_bytes: u64,
    pub procs: Vec<ProcSample>,
    /// Processes the kernel's list had no room for.
    pub omitted: u32,
    pub model: ModelStatus,
}

/// Why bytes are not a sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    TooLarge,
    BadMagic,
    Truncated,
    /// Bytes remain after the last process.
    Trailing,
    BadModelState(u8),
    TooManyProcs(u16),
    /// A name longer than [`MAX_NAME`].
    NameLength(u8),
    NameNotUtf8,
}

impl Sample {
    /// The sample's bytes. Total: a name past [`MAX_NAME`] is cut at a character
    /// boundary, and processes past [`MAX_PROCS`] are left out and counted in
    /// `omitted`, so what is written always parses.
    pub fn encode(&self) -> Vec<u8> {
        let kept = self.procs.len().min(MAX_PROCS);
        let left_out = self.procs.len() - kept;
        let omitted = u32::try_from(left_out)
            .unwrap_or(u32::MAX)
            .saturating_add(self.omitted);
        let mut out = Vec::with_capacity(64 + kept * 40);
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&self.uptime_ns.to_le_bytes());
        out.extend_from_slice(&self.cpus.to_le_bytes());
        out.extend_from_slice(&self.free_pages.to_le_bytes());
        out.extend_from_slice(&self.total_pages.to_le_bytes());
        out.extend_from_slice(&self.heap_bytes.to_le_bytes());
        out.extend_from_slice(&omitted.to_le_bytes());
        out.push(self.model.state.code());
        out.extend_from_slice(&self.model.tps10.to_le_bytes());
        out.extend_from_slice(&(kept as u16).to_le_bytes());
        for proc in &self.procs[..kept] {
            let name = cut(&proc.name, MAX_NAME);
            out.extend_from_slice(&proc.pid.to_le_bytes());
            out.extend_from_slice(&proc.pages.to_le_bytes());
            out.extend_from_slice(&proc.cpu_ns.to_le_bytes());
            out.push(name.len() as u8);
            out.extend_from_slice(name.as_bytes());
        }
        out
    }

    /// The sample `bytes` hold, or why they are not one.
    pub fn parse(bytes: &[u8]) -> Result<Sample, Error> {
        if bytes.len() > MAX_BYTES {
            return Err(Error::TooLarge);
        }
        let mut r = Reader(bytes);
        if r.take(MAGIC.len())? != MAGIC {
            return Err(Error::BadMagic);
        }
        let uptime_ns = r.u64()?;
        let cpus = r.u32()?;
        let free_pages = r.u64()?;
        let total_pages = r.u64()?;
        let heap_bytes = r.u64()?;
        let omitted = r.u32()?;
        let code = r.u8()?;
        let state = ModelState::from_code(code).ok_or(Error::BadModelState(code))?;
        let tps10 = r.u32()?;
        let count = r.u16()?;
        if usize::from(count) > MAX_PROCS {
            return Err(Error::TooManyProcs(count));
        }
        let mut procs = Vec::with_capacity(usize::from(count));
        for _ in 0..count {
            let pid = r.u32()?;
            let pages = r.u64()?;
            let cpu_ns = r.u64()?;
            let len = r.u8()?;
            if usize::from(len) > MAX_NAME {
                return Err(Error::NameLength(len));
            }
            let name =
                core::str::from_utf8(r.take(usize::from(len))?).map_err(|_| Error::NameNotUtf8)?;
            procs.push(ProcSample {
                pid,
                name: String::from(name),
                pages,
                cpu_ns,
            });
        }
        if !r.0.is_empty() {
            return Err(Error::Trailing);
        }
        Ok(Sample {
            uptime_ns,
            cpus,
            free_pages,
            total_pages,
            heap_bytes,
            procs,
            omitted,
            model: ModelStatus { state, tps10 },
        })
    }
}

/// `text` cut to at most `max` bytes at a character boundary.
fn cut(text: &str, max: usize) -> &str {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// The bytes not yet read.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let (head, rest) = self.0.split_at_checked(n).ok_or(Error::Truncated)?;
        self.0 = rest;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        self.take(N)?.try_into().map_err(|_| Error::Truncated)
    }

    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.array::<1>()?[0])
    }

    fn u16(&mut self) -> Result<u16, Error> {
        self.array().map(u16::from_le_bytes)
    }

    fn u32(&mut self) -> Result<u32, Error> {
        self.array().map(u32::from_le_bytes)
    }

    fn u64(&mut self) -> Result<u64, Error> {
        self.array().map(u64::from_le_bytes)
    }
}
