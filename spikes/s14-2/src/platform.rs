//! What Wasmtime's `custom` platform asks of an embedder (`wasmtime/src/runtime/vm/
//! sys/custom/capi.rs`), for one ring-3 process with one thread. Memory is the
//! global allocator's, page-aligned, and never executable: the Pulley
//! interpreter needs no native code, and nothing here maps read-execute.

use alloc::alloc::{Layout, alloc_zeroed, dealloc};
use core::ptr;
use core::sync::atomic::{AtomicPtr, Ordering};

const PAGE: usize = 4096;

fn layout(size: usize) -> Layout {
    Layout::from_size_align(size.max(1), PAGE).expect("a page-aligned layout")
}

#[unsafe(no_mangle)]
pub extern "C" fn wasmtime_page_size() -> usize {
    PAGE
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasmtime_mmap_new(size: usize, _prot: u32, ret: *mut *mut u8) -> i32 {
    // SAFETY: the layout has a non-zero size.
    let memory = unsafe { alloc_zeroed(layout(size)) };
    if memory.is_null() {
        return -1;
    }
    // SAFETY: Wasmtime passes a valid out pointer.
    unsafe { *ret = memory };
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
    // SAFETY: `addr` came from `wasmtime_mmap_new(size)`.
    unsafe { dealloc(addr, layout(size)) };
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn wasmtime_mprotect(_addr: *mut u8, _size: usize, _prot: u32) -> i32 {
    0
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
