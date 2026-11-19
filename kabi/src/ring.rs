//! KRNG v1, the one ring format for bulk data (ADR 0007 section 7), laid over
//! one `Shm` object (ADR 0001). Its users: ring-3 drivers, ADR 0005's
//! `kai-surface 1` transport (kind [`MSG`]), ADR 0006's `vmm` exit ring (kind
//! [`VMEXIT`]) and ADR 0004's file service if S04-2 moves bulk data off copied
//! IPC (kind [`BLOCK`]).
//!
//! - **Layout.** A 64-byte [`Header`] at offset 0; then `entries` 64-byte
//!   [`Request`]s; then `entries` 16-byte [`Response`]s; then a data area.
//! - **Indices.** `req_head`, `req_tail`, `rsp_head` and `rsp_tail` run free
//!   (they wrap at `u32::MAX`, never at `entries`); a slot is the index masked
//!   by `entries - 1`.
//! - **One producer and one consumer per direction.** A producer writes an
//!   entry, then publishes its head with release ordering; the consumer reads
//!   the head with acquire ordering, and publishes its tail the same way.
//! - **The other side is not trusted.** Each end keeps its own index in its
//!   own memory and reads only the peer's from the ring, and takes `entries`
//!   once, when it attaches. A peer's index that is out of range or goes
//!   backwards is [`Corrupt`], never a panic or a read outside the ring.
//! - A peer refuses any other version ([`Error::Version`]).

use core::marker::PhantomData;
use core::mem::size_of;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicU32, Ordering};

/// The header's magic.
pub const MAGIC: [u8; 4] = *b"KRNG";
/// The one version.
pub const VERSION: u16 = 1;
/// Fewest and most entries per direction; a power of two between them.
pub const MIN_ENTRIES: u32 = 8;
pub const MAX_ENTRIES: u32 = 4096;
/// Bytes of the header, of one request and of one response.
pub const HEADER_BYTES: usize = 64;
pub const REQUEST_BYTES: usize = 64;
pub const RESPONSE_BYTES: usize = 16;

/// Ring kinds, claimed in order (ADR 0007): block I/O (ADR 0004).
pub const BLOCK: u16 = 1;
/// Network frames.
pub const FRAME: u16 = 2;
/// ADR 0005's messages, at most [`MSG_MAX`] bytes each in the data area.
pub const MSG: u16 = 3;
/// Audio.
pub const PCM: u16 = 4;
/// ADR 0006's exits and doorbells; read data comes back in the data area.
pub const VMEXIT: u16 = 5;

/// Longest message a [`MSG`] ring carries (Wayland's `WL_MAX_MESSAGE_SIZE`).
pub const MSG_MAX: u32 = 4096;

/// [`BLOCK`]'s ops (ADR 0004's semantics, with no FUA).
pub mod block {
    pub const READ: u16 = 1;
    pub const WRITE: u16 = 2;
    pub const FLUSH: u16 = 3;
}

/// [`VMEXIT`]'s ops, the exit kinds (ADR 0006), and its flag.
pub mod vmexit {
    pub const MMIO: u16 = 1;
    pub const PIO: u16 = 2;
    pub const DOORBELL: u16 = 3;
    pub const SHUTDOWN: u16 = 4;
    pub const TRIPLE_FAULT: u16 = 5;
    /// `flags` bit 0: the access is a write.
    pub const WRITE: u16 = 1;
}

/// The 64-byte header at offset 0.
#[repr(C)]
pub struct Header {
    pub magic: [u8; 4],
    pub version: u16,
    pub kind: u16,
    /// A power of two, [`MIN_ENTRIES`] to [`MAX_ENTRIES`].
    pub entries: u32,
    pub req_head: AtomicU32,
    pub req_tail: AtomicU32,
    pub rsp_head: AtomicU32,
    pub rsp_tail: AtomicU32,
    /// Set by a side to ask for doorbells, as virtio's event suppression does.
    pub want_req: AtomicU32,
    pub want_rsp: AtomicU32,
    /// Zero.
    pub rest: [u8; 28],
}

/// One request, 64 bytes. `buffer` 0 is the ring's own data area; 1 to 15
/// name further `Shm` objects passed with the ring.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Request {
    pub tag: u64,
    pub op: u16,
    pub flags: u16,
    pub buffer: u32,
    pub offset: u64,
    pub len: u32,
    pub _r: u32,
    pub args: [u64; 4],
}

/// One response, 16 bytes: `status` 0 or a negative `kabi::Error`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Response {
    pub tag: u64,
    pub status: i32,
    pub len: u32,
}

const _: () = assert!(size_of::<Header>() == HEADER_BYTES);
const _: () = assert!(size_of::<Request>() == REQUEST_BYTES);
const _: () = assert!(size_of::<Response>() == RESPONSE_BYTES);

/// Why a ring cannot be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Not "KRNG".
    Magic,
    /// Another version: a peer refuses it.
    Version,
    /// `entries` is no power of two from 8 to 4,096.
    Entries,
    /// The region is too small, or not 8-byte aligned, for its layout.
    Size,
}

/// The peer's index is out of range or went backwards: the ring cannot be
/// trusted, and nothing was read or written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Corrupt;

/// The ring is full: the consumer has not taken enough yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Full;

/// Bytes a ring of `entries` with `data` bytes of data area takes.
pub const fn size(entries: u32, data: usize) -> usize {
    HEADER_BYTES + entries as usize * (REQUEST_BYTES + RESPONSE_BYTES) + data
}

fn entries_ok(entries: u32) -> bool {
    entries.is_power_of_two() && (MIN_ENTRIES..=MAX_ENTRIES).contains(&entries)
}

/// A KRNG v1 ring over a region of shared memory, attached: its `entries`
/// read once, here.
pub struct Ring<'a> {
    base: NonNull<u8>,
    len: usize,
    entries: u32,
    kind: u16,
    _region: PhantomData<&'a mut [u8]>,
}

// SAFETY: the ring's shared fields are atomics, and its entries are plain
// integers copied in and out whole by one producer and one consumer.
unsafe impl Send for Ring<'_> {}
unsafe impl Sync for Ring<'_> {}

impl<'a> Ring<'a> {
    /// Lays out a fresh ring of `kind` with `entries` in `region`: the header
    /// written, every index 0.
    ///
    /// # Safety
    /// `region` must be valid for reads and writes of `len` bytes for `'a`,
    /// 8-byte aligned, and touched by nothing else while it is set up.
    pub unsafe fn init(
        region: NonNull<u8>,
        len: usize,
        kind: u16,
        entries: u32,
    ) -> Result<Self, Error> {
        if !entries_ok(entries) {
            return Err(Error::Entries);
        }
        if len < size(entries, 0) || !(region.as_ptr() as usize).is_multiple_of(8) {
            return Err(Error::Size);
        }
        // SAFETY: the caller gives the region; the header fits (checked).
        unsafe {
            region.as_ptr().write_bytes(0, HEADER_BYTES);
            let header = region.as_ptr().cast::<Header>();
            (*header).magic = MAGIC;
            (*header).version = VERSION;
            (*header).kind = kind;
            (*header).entries = entries;
            Self::attach(region, len)
        }
    }

    /// Attaches to a ring the other side laid out: its magic, version and
    /// `entries` checked, and `entries` kept.
    ///
    /// # Safety
    /// `region` must be valid for reads and writes of `len` bytes for `'a`
    /// and 8-byte aligned; the other side may write any bytes into it at any
    /// time, which this type survives.
    pub unsafe fn attach(region: NonNull<u8>, len: usize) -> Result<Self, Error> {
        if len < HEADER_BYTES || !(region.as_ptr() as usize).is_multiple_of(8) {
            return Err(Error::Size);
        }
        let header = region.as_ptr().cast::<Header>();
        // SAFETY: the header lies in the region (checked); its plain fields
        // are read once, volatile, and judged before use.
        let (magic, version, kind, entries) = unsafe {
            (
                core::ptr::addr_of!((*header).magic).read_volatile(),
                core::ptr::addr_of!((*header).version).read_volatile(),
                core::ptr::addr_of!((*header).kind).read_volatile(),
                core::ptr::addr_of!((*header).entries).read_volatile(),
            )
        };
        if magic != MAGIC {
            return Err(Error::Magic);
        }
        if version != VERSION {
            return Err(Error::Version);
        }
        if !entries_ok(entries) {
            return Err(Error::Entries);
        }
        if len < size(entries, 0) {
            return Err(Error::Size);
        }
        Ok(Self {
            base: region,
            len,
            entries,
            kind,
            _region: PhantomData,
        })
    }

    pub fn kind(&self) -> u16 {
        self.kind
    }

    pub fn entries(&self) -> u32 {
        self.entries
    }

    /// Where the data area starts in the region, and how long it is.
    pub fn data_area(&self) -> (usize, usize) {
        let start = size(self.entries, 0);
        (start, self.len - start)
    }

    fn header(&self) -> &Header {
        // SAFETY: the header lies in the region for 'a (checked at attach);
        // only its atomics are read through this reference.
        unsafe { self.base.cast::<Header>().as_ref() }
    }

    /// Asks for (or stops asking for) doorbells on requests.
    pub fn want_requests(&self, on: bool) {
        self.header()
            .want_req
            .store(u32::from(on), Ordering::SeqCst);
    }

    /// Whether the other side asked for doorbells on requests.
    pub fn wants_requests(&self) -> bool {
        self.header().want_req.load(Ordering::SeqCst) != 0
    }

    pub fn want_responses(&self, on: bool) {
        self.header()
            .want_rsp
            .store(u32::from(on), Ordering::SeqCst);
    }

    pub fn wants_responses(&self) -> bool {
        self.header().want_rsp.load(Ordering::SeqCst) != 0
    }

    /// The request side's producer (the client).
    pub fn request_producer(&self) -> Producer<'_, 'a, Request> {
        Producer::new(self, Dir::Requests)
    }

    /// The request side's consumer (the server).
    pub fn request_consumer(&self) -> Consumer<'_, 'a, Request> {
        Consumer::new(self, Dir::Requests)
    }

    /// The response side's producer (the server).
    pub fn response_producer(&self) -> Producer<'_, 'a, Response> {
        Producer::new(self, Dir::Responses)
    }

    /// The response side's consumer (the client).
    pub fn response_consumer(&self) -> Consumer<'_, 'a, Response> {
        Consumer::new(self, Dir::Responses)
    }

    /// The (head, tail) atomics of `dir`.
    fn indices(&self, dir: Dir) -> (&AtomicU32, &AtomicU32) {
        let header = self.header();
        match dir {
            Dir::Requests => (&header.req_head, &header.req_tail),
            Dir::Responses => (&header.rsp_head, &header.rsp_tail),
        }
    }

    /// Where slot `index` of `dir` is: always inside the region, since the
    /// slot is masked by the `entries` kept at attach.
    fn slot(&self, dir: Dir, index: u32) -> *mut u8 {
        let at = (index & (self.entries - 1)) as usize;
        let offset = match dir {
            Dir::Requests => HEADER_BYTES + at * REQUEST_BYTES,
            Dir::Responses => {
                HEADER_BYTES + self.entries as usize * REQUEST_BYTES + at * RESPONSE_BYTES
            }
        };
        // SAFETY: offset + entry size <= size(entries, 0) <= len.
        unsafe { self.base.as_ptr().add(offset) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dir {
    Requests,
    Responses,
}

/// An entry the ring carries: copied whole, any bit pattern valid.
pub trait Entry: Copy {}
impl Entry for Request {}
impl Entry for Response {}

/// One direction's producer: keeps its head in its own memory and reads only
/// the consumer's tail from the ring.
pub struct Producer<'r, 'a, T: Entry> {
    ring: &'r Ring<'a>,
    dir: Dir,
    head: u32,
    /// The consumer's tail as last seen: it never goes back.
    tail: u32,
    _entry: PhantomData<T>,
}

impl<'r, 'a, T: Entry> Producer<'r, 'a, T> {
    fn new(ring: &'r Ring<'a>, dir: Dir) -> Self {
        let (head, tail) = ring.indices(dir);
        Self {
            ring,
            dir,
            head: head.load(Ordering::Acquire),
            tail: tail.load(Ordering::Acquire),
            _entry: PhantomData,
        }
    }

    /// The consumer's tail, judged: it may not pass this producer's head nor
    /// go back.
    fn tail(&mut self) -> Result<u32, Corrupt> {
        let tail = self.ring.indices(self.dir).1.load(Ordering::Acquire);
        let taken = tail.wrapping_sub(self.tail);
        let used = self.head.wrapping_sub(tail);
        if taken > self.head.wrapping_sub(self.tail) || used > self.ring.entries {
            return Err(Corrupt);
        }
        self.tail = tail;
        Ok(tail)
    }

    /// Free slots now.
    pub fn space(&mut self) -> Result<u32, Corrupt> {
        let tail = self.tail()?;
        Ok(self.ring.entries - self.head.wrapping_sub(tail))
    }

    /// Writes `entry` at the head, then publishes the head (release).
    pub fn push(&mut self, entry: &T) -> Result<Result<(), Full>, Corrupt> {
        if self.space()? == 0 {
            return Ok(Err(Full));
        }
        let slot = self.ring.slot(self.dir, self.head).cast::<T>();
        // SAFETY: the slot lies in the region; the consumer reads it only
        // after the head below says it is there.
        unsafe { slot.write_volatile(*entry) };
        self.head = self.head.wrapping_add(1);
        self.ring
            .indices(self.dir)
            .0
            .store(self.head, Ordering::Release);
        Ok(Ok(()))
    }
}

/// One direction's consumer: keeps its tail in its own memory and reads only
/// the producer's head from the ring.
pub struct Consumer<'r, 'a, T: Entry> {
    ring: &'r Ring<'a>,
    dir: Dir,
    tail: u32,
    /// The producer's head as last seen: it never goes back.
    head: u32,
    _entry: PhantomData<T>,
}

impl<'r, 'a, T: Entry> Consumer<'r, 'a, T> {
    fn new(ring: &'r Ring<'a>, dir: Dir) -> Self {
        let (head, tail) = ring.indices(dir);
        let tail = tail.load(Ordering::Acquire);
        Self {
            ring,
            dir,
            tail,
            head: head.load(Ordering::Acquire),
            _entry: PhantomData,
        }
    }

    /// The producer's head, judged (acquire): at most `entries` ahead of this
    /// tail, and never behind where it was.
    fn head(&mut self) -> Result<u32, Corrupt> {
        let head = self.ring.indices(self.dir).0.load(Ordering::Acquire);
        // Both measured from this tail, both at most `entries`, so no wrap
        // makes a head that went back look ahead.
        let ready = head.wrapping_sub(self.tail);
        let seen = self.head.wrapping_sub(self.tail);
        if ready > self.ring.entries || ready < seen {
            return Err(Corrupt);
        }
        self.head = head;
        Ok(head)
    }

    /// Entries ready now.
    pub fn ready(&mut self) -> Result<u32, Corrupt> {
        let head = self.head()?;
        Ok(head.wrapping_sub(self.tail))
    }

    /// Takes the entry at the tail, if one is ready, then publishes the tail
    /// (release).
    pub fn pop(&mut self) -> Result<Option<T>, Corrupt> {
        if self.ready()? == 0 {
            return Ok(None);
        }
        let slot = self.ring.slot(self.dir, self.tail).cast::<T>();
        // SAFETY: the slot lies in the region, and the head (acquire) says
        // the producer finished writing it.
        let entry = unsafe { slot.read_volatile() };
        self.tail = self.tail.wrapping_add(1);
        self.ring
            .indices(self.dir)
            .1
            .store(self.tail, Ordering::Release);
        Ok(Some(entry))
    }
}

#[cfg(test)]
mod tests;
