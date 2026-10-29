//! The guest side of `kai:app/component` (ADR 0003 sections 1-3): what a live
//! component implements, and [`component!`], which binds it to `kai:live/lifecycle`
//! through wit-bindgen 0.63.0. An app's own `actions` interface is bound by the
//! app's crate with its own `wit_bindgen::generate!`; its functions reach the one
//! instance through [`with`].
//!
//! A component is a plain Rust type. On wasm32 it is built as a `cdylib` with this
//! crate's allocator and panic handler; on the host it is an ordinary library, so
//! its logic is unit-tested like any other crate.

#![cfg_attr(target_family = "wasm", no_std)]

extern crate alloc;

#[cfg(target_family = "wasm")]
#[doc(hidden)]
pub mod abi;
pub mod app;
pub mod state;

/// The `kai:app/component` world's bindings: the `log` import and the
/// `kai:live/lifecycle` export.
#[cfg(target_family = "wasm")]
#[doc(hidden)]
pub mod bindings {
    wit_bindgen::generate!({
        world: "kai:app/component",
        path: ["../wit/kai-live", "../wit/kai-app"],
        pub_export_macro: true,
        export_macro_name: "export_component",
        default_bindings_module: "compsdk::bindings",
    });
}

/// Largest state a component may export.
pub const MAX_STATE: usize = kabi::live::MAX_CHECKPOINT;

use alloc::vec::Vec;
use core::cell::UnsafeCell;

#[doc(hidden)]
pub mod __private {
    pub use alloc::vec::Vec;
}

/// Why [`Component::import`] refused a state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportError {
    /// This version cannot read that schema.
    Unsupported = 1,
    /// The state claims the schema but does not hold to it.
    Bad = 2,
}

/// A live component: its state, and how it moves between versions. Its actions
/// are the app's own `actions` interface.
///
/// Every method but `init` and `import` only reads. A component has no services
/// beyond the log yet, so running a call twice (once on each version during
/// probation) cannot be told from running it once.
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

    /// 0 when the state holds to its invariants, otherwise a reason above 0.
    fn health(&self) -> u32;

    /// 0 when the version's own checks pass.
    fn self_test(&self) -> u32;
}

/// A value that lives as long as the instance. An instance runs one call at a
/// time on one thread, so plain interior mutability is enough.
pub struct Slot<T>(UnsafeCell<Option<T>>);

// SAFETY: wasm32 instances are single-threaded and calls never overlap; on the
// host a slot is only used by single-threaded tests.
unsafe impl<T> Sync for Slot<T> {}

impl<T> Default for Slot<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Slot<T> {
    pub const fn new() -> Self {
        Self(UnsafeCell::new(None))
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut Option<T>) -> R) -> R {
        // SAFETY: no call is re-entered, so no other reference exists meanwhile.
        unsafe { f(&mut *self.0.get()) }
    }
}

/// A component with its one instance, which [`component!`] gives it.
pub trait Live: Component + 'static {
    fn slot() -> &'static Slot<Self>;
}

/// Runs `f` on the started instance; `None` before `init` or a migration.
pub fn with<C: Live, R>(f: impl FnOnce(&mut C) -> R) -> Option<R> {
    C::slot().with(|slot| slot.as_mut().map(f))
}

/// Writes one line to the host's log (at most 160 bytes are kept).
pub fn log(text: &str) {
    #[cfg(target_family = "wasm")]
    bindings::kai::app::log::write(text);
    #[cfg(not(target_family = "wasm"))]
    let _ = text;
}

/// The lifecycle on a [`Live`] component, for [`component!`].
#[doc(hidden)]
pub mod live {
    use super::{ImportError, Live};
    use alloc::vec::Vec;

    /// 0, or the `start-error` case plus one.
    pub fn init<C: Live>() -> u32 {
        C::slot().with(|slot| *slot = Some(C::init()));
        0
    }

    pub fn migrate<C: Live>(state: &[u8], from_schema: u32) -> u32 {
        match C::import(state, from_schema) {
            Ok(imported) => {
                C::slot().with(|slot| *slot = Some(imported));
                0
            }
            Err(ImportError::Unsupported) => 1,
            Err(ImportError::Bad) => 2,
        }
    }

    pub fn save<C: Live>(to_schema: u32) -> Option<Vec<u8>> {
        let schema = if to_schema == 0 { C::SCHEMA } else { to_schema };
        C::slot().with(|slot| slot.as_ref()?.export(schema))
    }

    pub fn health<C: Live>() -> u32 {
        C::slot().with(|slot| slot.as_ref().map_or(1, C::health))
    }

    pub fn self_test<C: Live>() -> u32 {
        C::slot().with(|slot| slot.as_ref().map_or(1, C::self_test))
    }
}

/// Gives a [`Component`] its one instance and binds it to `kai:live/lifecycle`.
/// The `kai:live` part expands to nothing outside WebAssembly, so the component's
/// crate is also tested on the host.
#[macro_export]
macro_rules! component {
    ($component:ty) => {
        impl $crate::Live for $component {
            fn slot() -> &'static $crate::Slot<Self> {
                static SLOT: $crate::Slot<$component> = $crate::Slot::new();
                &SLOT
            }
        }

        #[cfg(target_family = "wasm")]
        const _: () = {
            use $crate::bindings::exports::kai::live::lifecycle::{Guest, StartError};

            struct Lifecycle;

            fn start(code: u32) -> Result<(), StartError> {
                match code {
                    0 => Ok(()),
                    1 => Err(StartError::UnsupportedSchema),
                    _ => Err(StartError::BadState),
                }
            }

            impl Guest for Lifecycle {
                fn schema() -> u32 {
                    <$component as $crate::Component>::SCHEMA
                }
                fn init() -> Result<(), StartError> {
                    start($crate::live::init::<$component>())
                }
                fn migrate(
                    state: $crate::__private::Vec<u8>,
                    from_schema: u32,
                ) -> Result<(), StartError> {
                    start($crate::live::migrate::<$component>(&state, from_schema))
                }
                fn save(to_schema: u32) -> Option<$crate::__private::Vec<u8>> {
                    $crate::live::save::<$component>(to_schema)
                }
                fn health() -> u32 {
                    $crate::live::health::<$component>()
                }
                fn self_test() -> u32 {
                    $crate::live::self_test::<$component>()
                }
            }

    $crate::bindings::export_component!(Lifecycle with_types_in $crate::bindings);
        };
    };
}
