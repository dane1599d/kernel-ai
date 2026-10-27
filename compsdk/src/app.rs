//! The rest of `kai:app/app` (ADR 0003 sections 3-4) for apps: their own
//! storage, the call's clock, effects queued for the agent core, and the events
//! the OS sends back. In WebAssembly these are the host's imports; on the host
//! they are a per-thread fake, so an app's logic is unit-tested with the same
//! calls. A component that uses none of them imports none of them.
//!
//! The host answers storage, clock and effects only during an action call: a
//! lifecycle call (`init`, `migrate`, `save`) that reaches them traps.

use alloc::string::String;
use alloc::vec::Vec;

#[cfg(target_family = "wasm")]
#[doc(hidden)]
pub mod bindings {
    wit_bindgen::generate!({
        world: "kai:compsdk/app-extras",
        path: ["../wit/kai-live", "../wit/kai-app", "wit"],
        generate_all,
        pub_export_macro: true,
        export_macro_name: "export_events",
        default_bindings_module: "compsdk::app::bindings",
    });
}

/// What the OS tells an app (`kai:app/events`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The app started.
    Started,
    /// The effect with this ticket finished: its result or why not.
    EffectDone {
        ticket: u32,
        outcome: Result<String, String>,
    },
}

/// What an app does with an [`Event`]; bound by [`events!`].
pub trait Events {
    fn on_event(&mut self, event: Event);
}

/// Something outside the app, run by the agent core under its tier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// A registry action by name, with its arguments as a WAVE record.
    Call { name: String, args: String },
    /// A line for the user.
    Notify(String),
}

#[cfg(not(target_family = "wasm"))]
pub mod fake {
    //! The host's stand-in: one storage, clock and effect queue per thread.
    use super::Effect;
    use alloc::collections::BTreeMap;
    use alloc::string::String;
    use alloc::vec::Vec;
    use core::cell::RefCell;

    extern crate std;

    #[derive(Default)]
    pub struct Os {
        pub storage: BTreeMap<String, Vec<u8>>,
        pub now_ms: u64,
        pub seed: u64,
        pub effects: Vec<Effect>,
    }

    std::thread_local! {
        static OS: RefCell<Os> = RefCell::new(Os::default());
    }

    /// Runs `f` on this thread's fake OS.
    pub fn with<R>(f: impl FnOnce(&mut Os) -> R) -> R {
        OS.with(|os| f(&mut os.borrow_mut()))
    }

    /// A fresh fake OS for the next test on this thread.
    pub fn reset() {
        with(|os| *os = Os::default());
    }
}

/// The value under `key` in the app's own scope, as it was when the call began.
pub fn get(key: &str) -> Option<Vec<u8>> {
    #[cfg(target_family = "wasm")]
    return bindings::kai::app::storage::get(key);
    #[cfg(not(target_family = "wasm"))]
    fake::with(|os| os.storage.get(key).cloned())
}

/// Sets `key`; the write commits with the call's checkpoint.
pub fn put(key: &str, value: &[u8]) {
    #[cfg(target_family = "wasm")]
    bindings::kai::app::storage::put(key, value);
    #[cfg(not(target_family = "wasm"))]
    fake::with(|os| os.storage.insert(String::from(key), value.to_vec()));
}

/// Removes `key`; the removal commits with the call's checkpoint.
pub fn delete(key: &str) {
    #[cfg(target_family = "wasm")]
    bindings::kai::app::storage::delete(key);
    #[cfg(not(target_family = "wasm"))]
    fake::with(|os| os.storage.remove(key));
}

/// Milliseconds since the Unix epoch, fixed for the whole call.
pub fn now_ms() -> u64 {
    #[cfg(target_family = "wasm")]
    return bindings::kai::app::clock::now_ms();
    #[cfg(not(target_family = "wasm"))]
    fake::with(|os| os.now_ms)
}

/// A random seed, fixed for the whole call.
pub fn seed() -> u64 {
    #[cfg(target_family = "wasm")]
    return bindings::kai::app::clock::seed();
    #[cfg(not(target_family = "wasm"))]
    fake::with(|os| os.seed)
}

/// Queues `effect`; its ticket, which an [`Event::EffectDone`] reports back.
pub fn request(effect: Effect) -> u32 {
    #[cfg(target_family = "wasm")]
    {
        use bindings::kai::app::effects::{ActionCall, Effect as E, request};
        let e = match effect {
            Effect::Call { name, args } => E::Call(ActionCall { name, args }),
            Effect::Notify(text) => E::Notify(text),
        };
        request(&e)
    }
    #[cfg(not(target_family = "wasm"))]
    fake::with(|os| {
        os.effects.push(effect);
        os.effects.len() as u32
    })
}

/// Binds [`Events`] on a [`crate::Live`] component to `kai:app/events`; events
/// before it starts are dropped.
#[macro_export]
macro_rules! events {
    ($app:ty) => {
        #[cfg(target_family = "wasm")]
        const _: () = {
            use $crate::app::bindings::exports::kai::app::events::{
                EffectResult, Event as E, Guest,
            };
            use $crate::app::{Event, Events};

            struct Bound;

            impl Guest for Bound {
                fn on_event(e: E) {
                    let event = match e {
                        E::Started => Event::Started,
                        E::EffectDone(EffectResult { ticket, outcome }) => {
                            Event::EffectDone { ticket, outcome }
                        }
                    };
                    $crate::with::<$app, ()>(|app| app.on_event(event));
                }
            }

    $crate::app::bindings::export_events!(Bound with_types_in $crate::app::bindings);
        };
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_fake_keeps_storage_and_queues_effects() {
        fake::reset();
        assert_eq!(get("a"), None);
        put("a", b"1");
        assert_eq!(get("a").as_deref(), Some(&b"1"[..]));
        delete("a");
        assert_eq!(get("a"), None);
        let ticket = request(Effect::Notify("hi".into()));
        assert_eq!(ticket, 1);
        fake::with(|os| assert_eq!(os.effects, [Effect::Notify("hi".into())]));
    }
}
