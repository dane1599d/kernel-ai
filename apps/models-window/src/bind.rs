//! `kai:models@0.1.1`'s bindings (`wit/kai-models/models.wit`, world
//! `models-read`): the read the window asks for and the list the host gives
//! back, and the switch a person's key or click asks for and what the shell
//! answered, each handed to the window.

use crate::ModelsWindow;
use alloc::string::String;

wit_bindgen::generate!({
    world: "kai:models/models-read",
    path: "../../wit/kai-models",
    generate_all,
});

use exports::kai::models::listing::Guest as Listing;

struct Bound;

fn started<R>(f: impl FnOnce(&mut ModelsWindow) -> R) -> R {
    match compsdk::with::<ModelsWindow, R>(f) {
        Some(result) => result,
        None => panic!("the model manager was called before it started"),
    }
}

/// Asks the host for the model list; its ticket.
pub fn read() -> u32 {
    kai::models::models::read()
}

/// Asks for the stored model with `sha256` (64 lowercase hex digits) from the
/// next restart; its ticket.
pub fn use_model(sha256: &str) -> u32 {
    kai::models::models::use_model(sha256)
}

impl Listing for Bound {
    fn on_listing(_ticket: u32, outcome: Result<String, String>) {
        started(|app| app.listed(outcome));
    }

    fn on_used(_ticket: u32, outcome: Result<String, String>) {
        started(|app| app.used(outcome));
    }
}

export!(Bound);
