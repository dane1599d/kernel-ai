//! `app:wait/actions@1.0.0`'s binding (`wit/app.wit`, world `bound`): `seen`
//! reaches the one instance through `compsdk::with`.

use crate::Fixture;
use alloc::string::String;

wit_bindgen::generate!({
    world: "app:wait/bound",
    path: "wit",
    generate_all,
});

use exports::app::wait::actions::Guest as Actions;

struct Bound;

impl Actions for Bound {
    fn seen() -> Result<String, String> {
        compsdk::with::<Fixture, String>(|app| app.seen())
            .ok_or_else(|| String::from("not started"))
    }
}

export!(Bound);
