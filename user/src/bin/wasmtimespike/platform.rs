//! What Wasmtime's `custom` platform asks of an embedder (`wasmtime/src/runtime/vm/
//! sys/custom/capi.rs`), for one ring-3 process with one thread. Memory is a
//! reservation committed whole, so it can be made read-execute later with
//! MEM_EXEC (S14-2's throwaway loader call).

use core::ptr;
use core::sync::atomic::{AtomicPtr, AtomicU64, Ordering};
use user::sys;

const PAGE: usize = 4096;
const PROT_EXEC: u32 = 1 << 2;

/// Bytes committed now, and the most there were.
pub static LIVE: AtomicU64 = AtomicU64::new(0);
pub static PEAK: AtomicU64 = AtomicU64::new(0);

#[unsafe(no_mangle)]
pub extern "C" fn wasmtime_page_size() -> usize {
    PAGE
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasmtime_mmap_new(size: usize, _prot: u32, ret: *mut *mut u8) -> i32 {
    let Ok(addr) = sys::mem_reserve(size as u64) else {
        return -1;
    };
    if sys::mem_commit(addr, size as u64).is_err() {
        let _ = sys::mem_release(addr);
        return -1;
    }
    let live = LIVE.fetch_add(size as u64, Ordering::Relaxed) + size as u64;
    PEAK.fetch_max(live, Ordering::Relaxed);
    // SAFETY: Wasmtime passes a valid out pointer.
    unsafe { *ret = addr as *mut u8 };
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasmtime_mmap_remap(addr: *mut u8, size: usize, _prot: u32) -> i32 {
    // A remap hands back zeroed pages.
    // SAFETY: Wasmtime remaps inside a region it got from `wasmtime_mmap_new`.
    unsafe { ptr::write_bytes(addr, 0, size) };
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasmtime_munmap(addr: *mut u8, size: usize) -> i32 {
    LIVE.fetch_sub(size as u64, Ordering::Relaxed);
    match sys::mem_release(addr as u64) {
        Ok(_) => 0,
        Err(_) => -1,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn wasmtime_mprotect(addr: *mut u8, size: usize, prot: u32) -> i32 {
    if prot & PROT_EXEC == 0 {
        // Pages are read-write; Wasmtime asks for read-only only to guard
        // what it has finished writing, which the spike does not enforce.
        return 0;
    }
    match sys::mem_exec(addr as u64, size as u64) {
        Ok(_) => 0,
        Err(_) => -1,
    }
}

static TLS: [AtomicPtr<u8>; 2] = [const { AtomicPtr::new(ptr::null_mut()) }; 2];

#[unsafe(no_mangle)]
pub extern "C" fn wasmtime_tls_get(slot: usize) -> *mut u8 {
    TLS[slot].load(Ordering::Relaxed)
}

#[unsafe(no_mangle)]
pub extern "C" fn wasmtime_tls_set(slot: usize, value: *mut u8) {
    TLS[slot].store(value, Ordering::Relaxed);
}
