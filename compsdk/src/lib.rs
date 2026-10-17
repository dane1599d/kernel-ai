//! The guest side of `kai-lifecycle-1` (see `kabi::live`): what a live component
//! implements, and [`export!`], which binds it to the WebAssembly exports the
//! component host calls.
//!
//! A component is a plain Rust type. On wasm32 it is built as a `cdylib` with this
//! crate's allocator and panic handler; on the host it is an ordinary library, so
//! its logic is unit-tested like any other crate.

#![cfg_attr(target_family = "wasm", no_std)]

extern crate alloc;

#[cfg(target_family = "wasm")]
#[doc(hidden)]
pub mod abi;
pub mod state;

/// Longest reply a request may get.
pub const MAX_REPLY: usize = kabi::MAX_MESSAGE;
/// Largest state a component may export.
pub const MAX_STATE: usize = kabi::live::MAX_CHECKPOINT;

use alloc::string::String;
use alloc::vec::Vec;

/// Why [`Component::import`] refused a state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportError {
    /// This version cannot read that schema.
    Unsupported = 1,
    /// The state claims the schema but does not hold to it.
    Bad = 2,
}

/// A live component: its state, its requests, and how it moves between versions.
///
/// Every method but `handle` and `init`/`import` only reads. A component has no
/// clock, randomness or other services, so running a request twice (once on each
/// version during probation) cannot be told from running it once.
pub trait Component: Sized {
    /// The schema this version's state is written in.
    const SCHEMA: u32;

    /// An empty component.
    fn init() -> Self;

    /// A component from a state written by any version: the migration.
    fn import(state: &[u8], from_schema: u32) -> Result<Self, ImportError>;

    /// The state in `schema` (this version's own or an older one), as CBOR
    /// `[schema, body]`; `None` when it cannot write that schema.
    fn export(&self, schema: u32) -> Option<Vec<u8>>;

    /// Answers one request with `ok ...` or `err ...`.
    fn handle(&mut self, request: &str) -> String;

    /// 0 when the state holds to its invariants, otherwise a reason above 0.
    fn health(&self) -> u32;

    /// 0 when the version's own checks pass.
    fn self_test(&self) -> u32;
}

/// Writes one line to the host's log (at most 160 bytes are kept).
pub fn log(text: &str) {
    #[cfg(target_family = "wasm")]
    abi::log(text);
    #[cfg(not(target_family = "wasm"))]
    let _ = text;
}

/// Binds a [`Component`] to the `kai-lifecycle-1` exports. Expands to nothing
/// outside WebAssembly, so the component's crate can also be tested on the host.
#[macro_export]
macro_rules! export {
    ($component:ty) => {
        #[cfg(target_family = "wasm")]
        const _: () = {
            use $crate::abi;

            static SLOT: abi::Slot<$component> = abi::Slot::new();

            #[unsafe(no_mangle)]
            pub extern "C" fn kai_alloc(len: u32) -> u32 {
                abi::alloc(len)
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn kai_schema() -> u32 {
                <$component as $crate::Component>::SCHEMA
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn kai_init() -> u32 {
                abi::init(&SLOT)
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn kai_import(ptr: u32, len: u32, from_schema: u32) -> u32 {
                abi::import(&SLOT, ptr, len, from_schema)
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn kai_export(to_schema: i32) -> i64 {
                abi::export(&SLOT, to_schema)
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn kai_handle(ptr: u32, len: u32) -> i64 {
                abi::handle(&SLOT, ptr, len)
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn kai_health() -> u32 {
                abi::health(&SLOT)
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn kai_self_test() -> u32 {
                abi::self_test(&SLOT)
            }
        };
    };
}
