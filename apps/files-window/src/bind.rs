//! `app:files/actions@1.0.0`'s bindings (`apps/files/wit/files.wit`, world
//! `actions-only`): each function reaches the one instance through
//! `compsdk::with`, moves the window's view and redraws it.

use crate::FilesWindow;
use alloc::string::String;

wit_bindgen::generate!({
    world: "app:files/actions-only",
    path: ["../../wit/kai-live", "../../wit/kai-app", "../files/wit"],
});

use exports::app::files::actions::Guest;

struct Bound;

fn started<R>(f: impl FnOnce(&mut FilesWindow) -> R) -> R {
    match compsdk::with::<FilesWindow, R>(f) {
        Some(result) => result,
        None => panic!("files was called before it started"),
    }
}

impl Guest for Bound {
    fn open(path: String) -> Result<String, String> {
        started(|app| app.act(|files, os| files.act_open(os, &path)))
    }

    fn reveal(path: String) -> Result<String, String> {
        started(|app| app.act(|files, os| files.act_reveal(os, &path)))
    }

    fn select(name: String) -> Result<String, String> {
        started(|app| app.act(|files, os| files.act_select(os, &name)))
    }
}

export!(Bound);
