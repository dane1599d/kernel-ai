//! Bulk reads and writes (`kai-fs 1`'s ReadShm and WriteShm): a process that
//! calls [`reserve`] gets one `Shm` of up to [`fsproto::MAX_SHM_DATA`],
//! kept for its life, and a duplicate of it moves with each request. One is
//! made once, not per call, since SHM_CREATE counts its pages against the
//! quota until the process ends. Without one, while another thread uses it,
//! once it is poisoned, or with an `fs` that predates the requests,
//! [`super::read_at`] and [`super::write_at`] go a message at a time.
//!
//! Who reserves: the kernel has 64 `Shm` objects for the whole system
//! (`kernel::shm::MAX_OBJECTS`), shared with windows, `cc` and `fetch`, so only
//! a process that moves files in bulk should reserve one: a benchmark, a
//! model or package download. Apps and the shell do not. A reserve that finds
//! none free answers Busy, and that process goes a message at a time.
//!
//! A request whose CALL_H fails after the `Shm` moved (a timeout, `fs` gone)
//! may still be running in `fs`, which could yet read or fill the `Shm`. The
//! `Shm` is then poisoned for the process's life: it is never handed to `fs`
//! again, so no later transfer's bytes can meet that request's.

use crate::sys;
use alloc::format;
use alloc::string::String;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use fsproto::{Error, MAX_SHM_DATA, Reply, Request};
use kabi::handle::{Handle, MAX_TRANSFER, Rights};

/// What the `fs` side maps: read-write, and moved.
const MOVED: Rights = Rights::MAP
    .union(Rights::READ)
    .union(Rights::WRITE)
    .union(Rights::TRANSFER);
const PAGE: u64 = 4096;

/// The process's `Shm`: its handle (0 for none), where it is mapped, and
/// its length; set once by [`reserve`].
static HANDLE: AtomicU32 = AtomicU32::new(0);
static ADDRESS: AtomicU64 = AtomicU64::new(0);
static LEN: AtomicU64 = AtomicU64::new(0);
/// Held while a call uses the `Shm`.
static BUSY: AtomicBool = AtomicBool::new(false);
/// Set once a request's CALL_H failed after the `Shm` moved with it.
static POISONED: AtomicBool = AtomicBool::new(false);

/// Makes the process's bulk `Shm`, `bytes` long (at most
/// [`fsproto::MAX_SHM_DATA`]), counted against its quota for its life.
/// Once is enough; a second call keeps the first.
pub fn reserve(bytes: usize) -> Result<(), kabi::Error> {
    if BUSY.swap(true, Ordering::Acquire) {
        return Err(kabi::Error::Busy);
    }
    let made = (|| {
        if HANDLE.load(Ordering::Relaxed) != 0 {
            return Ok(());
        }
        let len = bytes.min(MAX_SHM_DATA) as u64;
        let handle = sys::shm_create(len.div_ceil(PAGE))?;
        let address = sys::shm_map(handle, true).inspect_err(|_| {
            let _ = sys::handle_close(handle);
        })?;
        ADDRESS.store(address, Ordering::Relaxed);
        LEN.store(len, Ordering::Relaxed);
        HANDLE.store(handle.0, Ordering::Relaxed);
        Ok(())
    })();
    BUSY.store(false, Ordering::Release);
    made
}

/// Whether a call failed after the `Shm` moved: it is then never sent again,
/// and its holders go a message at a time.
pub fn poisoned() -> bool {
    POISONED.load(Ordering::Acquire)
}

/// The `Shm` while a caller holds it: for one call here, or for the life of a
/// client that moves its own requests, as `pkg`'s store does
/// (`kaipkg::fsvol::Bulk`). While one is held, [`read_at`] and [`write_at`]
/// go a message at a time.
pub struct Lease {
    handle: Handle,
    address: u64,
    len: usize,
}

impl Lease {
    /// The `Shm`, if [`reserve`] made one, it is not poisoned and no one else
    /// holds it.
    pub fn take() -> Option<Self> {
        let unusable = HANDLE.load(Ordering::Relaxed) == 0 || POISONED.load(Ordering::Acquire);
        if unusable || BUSY.swap(true, Ordering::Acquire) {
            return None;
        }
        Some(Self {
            handle: Handle(HANDLE.load(Ordering::Relaxed)),
            address: ADDRESS.load(Ordering::Relaxed),
            len: LEN.load(Ordering::Relaxed) as usize,
        })
    }

    /// The `Shm`'s bytes: what a WriteShm sends and a ReadShm fills.
    pub fn bytes(&mut self) -> &mut [u8] {
        // SAFETY: `reserve` made at least `len` bytes, mapped read-write
        // at `address` for the process's life; this lease alone uses them
        // (BUSY), and `fs` touches them only while this thread waits in
        // CALL_H.
        unsafe { core::slice::from_raw_parts_mut(self.address as *mut u8, self.len) }
    }

    /// `request` on `file` with a duplicate of the `Shm` moved in.
    /// Refused once the `Shm` is poisoned ([`poisoned`]), so a caller that
    /// holds a lease can never send it again.
    pub fn ask(&self, file: Handle, request: Request) -> Result<Reply, String> {
        if poisoned() {
            return Err(String::from("the bulk Shm is poisoned"));
        }
        let copy = sys::handle_dup(self.handle, MOVED).map_err(|e| format!("HANDLE_DUP: {e:?}"))?;
        let bytes = fsproto::encode_request(&request);
        let mut reply = alloc::vec![0u8; fsproto::MAX_MESSAGE];
        let mut moved = [Handle::NONE; MAX_TRANSFER];
        let call = sys::call_h(
            file,
            &bytes,
            &mut reply,
            super::CALL_MS,
            &[copy],
            &mut moved,
        );
        let (len, _) = call.map_err(|e| {
            match e {
                // Refused before anything moved: the copy is still here.
                kabi::Error::Permission | kabi::Error::Invalid => {
                    let _ = sys::handle_close(copy);
                }
                // It moved, and the request may still be running in `fs`.
                _ => POISONED.store(true, Ordering::Release),
            }
            format!("CALL_H: {e:?}")
        })?;
        fsproto::decode_reply(&reply[..len]).map_err(|e| format!("a reply: {e:?}"))
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::Release);
    }
}

/// Fills `buf` from `file` at `off`: how many bytes, short at the file's
/// end; `None` when it could not go in bulk and read nothing.
pub fn read_at(file: Handle, off: u64, buf: &mut [u8]) -> Result<Option<usize>, String> {
    let Some(mut shm) = Lease::take() else {
        return Ok(None);
    };
    let mut at = 0;
    while at < buf.len() {
        let len = (buf.len() - at).min(shm.len) as u32;
        let request = Request::ReadShm {
            off: off + at as u64,
            len,
        };
        match shm.ask(file, request)? {
            Reply::Failed(Error::BadRequest) if at == 0 => return Ok(None),
            Reply::Copied { len: 0 } => break,
            Reply::Copied { len } if len as usize <= buf.len() - at => {
                let len = len as usize;
                buf[at..at + len].copy_from_slice(&shm.bytes()[..len]);
                at += len;
            }
            other => return Err(format!("a bulk read at {}: {other:?}", off + at as u64)),
        }
    }
    Ok(Some(at))
}

/// Writes `bytes` to `file` from `off`; `false` when it could not go in
/// bulk and wrote nothing.
pub fn write_at(file: Handle, off: u64, bytes: &[u8]) -> Result<bool, String> {
    let Some(mut shm) = Lease::take() else {
        return Ok(false);
    };
    let size = shm.len;
    for (i, piece) in bytes.chunks(size).enumerate() {
        shm.bytes()[..piece.len()].copy_from_slice(piece);
        let at = off + (i * size) as u64;
        let request = Request::WriteShm {
            off: at,
            len: piece.len() as u32,
        };
        match shm.ask(file, request)? {
            Reply::Done => {}
            Reply::Failed(Error::BadRequest) if i == 0 => return Ok(false),
            other => return Err(format!("a bulk write at {at}: {other:?}")),
        }
    }
    Ok(true)
}

/// Pushes `bytes` into the sealed blob open on the model store's `dir`
/// (SealedPushShm), the `Shm`'s length a round trip; `false` when it could not
/// go in bulk and pushed nothing, and the caller pushes in messages.
pub fn sealed_push(dir: Handle, bytes: &[u8]) -> Result<bool, String> {
    let Some(mut shm) = Lease::take() else {
        return Ok(false);
    };
    let size = shm.len;
    for (i, piece) in bytes.chunks(size).enumerate() {
        shm.bytes()[..piece.len()].copy_from_slice(piece);
        let request = Request::SealedPushShm {
            len: piece.len() as u32,
        };
        match shm.ask(dir, request)? {
            Reply::Done => {}
            Reply::Failed(Error::BadRequest) if i == 0 => return Ok(false),
            other => return Err(format!("a bulk sealed push: {other:?}")),
        }
    }
    Ok(true)
}
