//! Notes on `kai:app/component` through wit-bindgen: the same `Notes` type the
//! hand-bound artifact exports, so the difference in size is the binding's.

#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use comp_notes::Notes;
use compsdk::Component;
use core::cell::UnsafeCell;

wit_bindgen::generate!({ world: "notes", path: "../wit-notes", generate_all });

use exports::kai::live::lifecycle::{Guest as Lifecycle, StartError};
use exports::kai::probe_notes::requests::Guest as Requests;

/// The one instance; calls never overlap on a wasm32 instance.
struct Slot(UnsafeCell<Option<Notes>>);

// SAFETY: wasm32 instances here are single-threaded and calls never overlap.
unsafe impl Sync for Slot {}

static SLOT: Slot = Slot(UnsafeCell::new(None));

fn with<R>(f: impl FnOnce(&mut Option<Notes>) -> R) -> R {
    // SAFETY: no call is re-entered, so no other reference exists meanwhile.
    unsafe { f(&mut *SLOT.0.get()) }
}

struct Bound;

impl Lifecycle for Bound {
    fn schema() -> u32 {
        Notes::SCHEMA
    }

    fn init() -> Result<(), StartError> {
        with(|slot| *slot = Some(Notes::init()));
        Ok(())
    }

    fn migrate(state: Vec<u8>, from_schema: u32) -> Result<(), StartError> {
        let notes = Notes::import(&state, from_schema).map_err(|e| match e {
            compsdk::ImportError::Unsupported => StartError::UnsupportedSchema,
            compsdk::ImportError::Bad => StartError::BadState,
        })?;
        with(|slot| *slot = Some(notes));
        Ok(())
    }

    fn save(to_schema: u32) -> Option<Vec<u8>> {
        let schema = if to_schema == 0 { Notes::SCHEMA } else { to_schema };
        with(|slot| slot.as_ref()?.export(schema))
    }

    fn health() -> u32 {
        with(|slot| slot.as_ref().map_or(1, |n| n.health()))
    }

    fn self_test() -> u32 {
        with(|slot| slot.as_ref().map_or(1, |n| n.self_test()))
    }
}

impl Requests for Bound {
    fn handle(request: String) -> String {
        with(|slot| match slot {
            Some(notes) => notes.handle(&request),
            None => String::from("err not started"),
        })
    }
}

export!(Bound);
