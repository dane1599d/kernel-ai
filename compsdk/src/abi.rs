//! The WebAssembly side of the exports: memory for inputs, the one component
//! instance, and packing results as `(ptr << 32) | len`. Built only for wasm32.

use crate::Component;
use alloc::alloc::{Layout, alloc as allocate};
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::UnsafeCell;

#[global_allocator]
static ALLOCATOR: talc::wasm::WasmDynamicTalc = talc::wasm::new_wasm_dynamic_allocator();

/// A panic is a trap: the host rolls back or reloads; nothing here is trusted again.
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

#[link(wasm_import_module = "kai")]
unsafe extern "C" {
    #[link_name = "log"]
    fn kai_log(ptr: *const u8, len: usize);
}

pub fn log(text: &str) {
    // SAFETY: the host only reads `len` bytes at `ptr`, which `text` owns.
    unsafe { kai_log(text.as_ptr(), text.len()) }
}

/// A value that lives as long as the instance. An instance runs one call at a
/// time on one thread, so plain interior mutability is enough.
pub struct Slot<T>(UnsafeCell<Option<T>>);

// SAFETY: wasm32 instances here are single-threaded and calls never overlap.
unsafe impl<T> Sync for Slot<T> {}

impl<T> Slot<T> {
    pub const fn new() -> Self {
        Self(UnsafeCell::new(None))
    }

    fn with<R>(&self, f: impl FnOnce(&mut Option<T>) -> R) -> R {
        // SAFETY: no call is re-entered, so no other reference exists meanwhile.
        unsafe { f(&mut *self.0.get()) }
    }
}

/// The last reply or export, kept until the next call so the host can copy it.
static OUTPUT: Slot<Vec<u8>> = Slot::new();

fn pack(bytes: Vec<u8>) -> i64 {
    OUTPUT.with(|output| {
        let bytes = output.insert(bytes);
        (i64::from(bytes.as_ptr() as u32) << 32) | i64::from(bytes.len() as u32)
    })
}

/// Memory for an input of `len` bytes; 0 when there is none.
pub fn alloc(len: u32) -> u32 {
    if len == 0 {
        // Nothing to hold; any non-zero address will do, and nothing is freed.
        return 1;
    }
    match Layout::array::<u8>(len as usize) {
        // SAFETY: the layout has a non-zero size.
        Ok(layout) => unsafe { allocate(layout) as u32 },
        Err(_) => 0,
    }
}

/// Takes back an input the host wrote into memory from [`alloc`].
fn take(ptr: u32, len: u32) -> Vec<u8> {
    if len == 0 {
        return Vec::new();
    }
    // SAFETY: the host passes back exactly a block from `alloc(len)`, whose layout
    // matches a `Vec<u8>` of capacity `len`, and does not use it again.
    unsafe { Vec::from_raw_parts(ptr as *mut u8, len as usize, len as usize) }
}

pub fn init<C: Component>(slot: &Slot<C>) -> u32 {
    slot.with(|component| *component = Some(C::init()));
    0
}

pub fn import<C: Component>(slot: &Slot<C>, ptr: u32, len: u32, from_schema: u32) -> u32 {
    let state = take(ptr, len);
    match C::import(&state, from_schema) {
        Ok(imported) => {
            slot.with(|component| *component = Some(imported));
            0
        }
        Err(error) => error as u32,
    }
}

pub fn export<C: Component>(slot: &Slot<C>, to_schema: i32) -> i64 {
    let schema = match to_schema {
        0 => C::SCHEMA,
        1.. => to_schema as u32,
        _ => return -1,
    };
    slot.with(|component| component.as_ref().and_then(|c| c.export(schema)))
        .map_or(-1, pack)
}

pub fn handle<C: Component>(slot: &Slot<C>, ptr: u32, len: u32) -> i64 {
    let request = take(ptr, len);
    let reply = match core::str::from_utf8(&request) {
        Ok(text) => slot
            .with(|component| component.as_mut().map(|c| c.handle(text)))
            .unwrap_or_else(|| String::from("err not started")),
        Err(_) => String::from("err the request is not UTF-8"),
    };
    pack(reply.into_bytes())
}

pub fn health<C: Component>(slot: &Slot<C>) -> u32 {
    slot.with(|component| component.as_ref().map_or(1, C::health))
}

pub fn self_test<C: Component>(slot: &Slot<C>) -> u32 {
    slot.with(|component| component.as_ref().map_or(1, C::self_test))
}
