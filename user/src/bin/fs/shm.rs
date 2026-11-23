//! The `Shm` a bulk request moves in (`kai-fs 1`'s ReadShm and WriteShm,
//! `fsserve`'s `bulk.rs`): mapped read-write, whole, for that one request, and
//! unmapped before the reply. `fs` touches only the bytes the request names,
//! which the kernel checked the object holds (SHM_MAP's fewest pages).

use core::sync::atomic::{AtomicU64, Ordering};
use fsproto::{MAX_SHM_DATA, Request};
use kabi::handle::{Handle, Kind};
use user::{println, sys};

/// An `Shm`'s pages are 4 KiB.
const PAGE: usize = 4096;
/// Failures to map that `fs` logs; the rest are only answered Invalid.
const LOGGED_FAILURES: u64 = 8;
static FAILURES: AtomicU64 = AtomicU64::new(0);

/// One mapped `Shm`, `len` bytes from `address`.
pub struct Mapped {
    address: u64,
    len: usize,
}

impl Mapped {
    /// Maps `handle` when `request` is a bulk request and it is an `Shm` of at
    /// least the bytes the request names: the kernel refuses a shorter one
    /// (SHM_MAP's fewest pages), so `fs` never faults past its end. `None`
    /// otherwise, and the request is then answered Invalid.
    pub fn of(request: &[u8], handle: Handle) -> Option<Self> {
        let len = match fsproto::decode_request(request).ok()? {
            Request::ReadShm { len, .. } | Request::WriteShm { len, .. } => {
                (len as usize).min(MAX_SHM_DATA)
            }
            _ => return None,
        };
        let is_shm = sys::handle_info(handle).is_ok_and(|i| i.kind() == Some(Kind::Shm));
        if !is_shm {
            return None;
        }
        let pages = len.div_ceil(PAGE) as u64;
        match sys::shm_map_least(handle, true, pages) {
            Ok(address) => Some(Self { address, len }),
            Err(error) => {
                // A client can send these at will: the first few only.
                if FAILURES.fetch_add(1, Ordering::Relaxed) < LOGGED_FAILURES {
                    println!("fs: a bulk request's Shm does not map: {error:?}");
                }
                None
            }
        }
    }

    /// The mapped bytes.
    pub fn bytes(&mut self) -> &mut [u8] {
        // SAFETY: the kernel mapped the `Shm` read-write at `address` with at
        // least `len` bytes (SHM_MAP's fewest pages), in a slot of its own
        // (`kabi::layout::SHM_SLOT`), and it stays mapped until `drop`. The
        // client's other threads may write it meanwhile: `fsserve` only copies
        // in or out of it once, so the worst they get is their own torn bytes.
        unsafe { core::slice::from_raw_parts_mut(self.address as *mut u8, self.len) }
    }
}

impl Drop for Mapped {
    fn drop(&mut self) {
        if let Err(error) = sys::shm_unmap(self.address) {
            println!("fs: shm_unmap failed: {error:?}");
        }
    }
}
