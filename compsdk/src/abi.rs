//! The WebAssembly runtime of a component: its allocator, which wit-bindgen's
//! `cabi_realloc` uses, and its panic handler. Built only for wasm32.

#[global_allocator]
static ALLOCATOR: talc::wasm::WasmDynamicTalc = talc::wasm::new_wasm_dynamic_allocator();

/// A panic is a trap: the host rolls back or reloads; nothing here is trusted again.
/// Where it happened goes to the host first (`kai:app/log.panic`), so the fault
/// names the line.
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    if let Some(at) = info.location() {
        crate::bindings::kai::app::log::panic(at.file(), at.line(), at.column());
    }
    core::arch::wasm32::unreachable()
}
