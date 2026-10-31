//! P4 task 13's exit case (`test=c-build`): `cc build` in the OS. `pkg` runs
//! `pkg-image`'s step (generation 1, the image's notes), and the shell has
//! [`kabi::plan::SHELL_BUILD`]'s route to it.
//!
//! The case writes a C app into `/home/user/c/hello/` as `fs`'s kernel
//! client, then types `cc build hello` as the terminal does:
//! - `local-next` gives 0.0.1;
//! - `cc` builds it;
//! - `pkg` publishes it into `local`, signed by the device key it makes, and
//!   installs it as generation 2, then starts it; its `init` logs a line.
//!
//! `undo` switches back to generation 1, and the next build is 0.0.2. The host
//! checks `pkg`'s and the app's lines.

use crate::files_tests::ask;
use crate::pkg_tests::{Step, wait_for};
use crate::shell_tests::start;
use crate::{ipc, log};
use alloc::format;
use alloc::string::String;
use fsproto::{How, Reply, Request};
use kabi::handle::Rights;

/// The built app's plan line: started by `pkg`, run by comphost like notes.
const HELLO: &str = "hello program=comphost caps=serve+component pages=8192 stack_pages=512 burst_ms=20000 start=pkg";
/// An app that asks for nothing: what `local` installs. Its `init` logs.
const SOURCE: &str = "#include <app.h>
// SPDX-License-Identifier: MIT
uint32_t exports_kai_live_lifecycle_schema(void) { return 1; }
bool exports_kai_live_lifecycle_init(exports_kai_live_lifecycle_start_error_t *err) {
    app_string_t text;
    app_string_set(&text, \"hello from C, built in the OS\");
    kai_app_log_write(&text);
    return true;
}
bool exports_kai_live_lifecycle_migrate(app_list_u8_t *state, uint32_t from_schema,
                                        exports_kai_live_lifecycle_start_error_t *err) { return true; }
bool exports_kai_live_lifecycle_save(uint32_t to_schema, app_list_u8_t *ret) {
    ret->ptr = 0;
    ret->len = 0;
    return true;
}
uint32_t exports_kai_live_lifecycle_health(void) { return 0; }
uint32_t exports_kai_live_lifecycle_self_test(void) { return 0; }
void exports_kai_app_events_on_event(exports_kai_app_events_event_t *e) {
    exports_kai_app_events_event_free(e);
}
";
/// A build's line: cc.wasm's translation on the first, the build itself, and
/// `pkg`'s publish, check and install, under TCG too.
const BUILD_MS: u64 = 280_000;

/// `pkg`'s image step, the model, `cc`, the app's line and the shell.
pub fn plan() -> String {
    format!(
        "{}\n{}\n{}\n{HELLO}\n{}",
        crate::pkg_tests::plan(Step::Image),
        crate::llm::plan_line(),
        kabi::plan::CC,
        kabi::plan::SHELL_BUILD
    )
}

pub fn check() -> Result<(), &'static str> {
    wait_for(&format!("case/{}.done", Step::Image.name()))
        .map_err(|_| "c-build: pkg did not finish its image step")?;
    write_sources()?;
    start()?;
    let first = line("cc build hello")?;
    if !first.starts_with("ok built hello 0.0.1 (") || !first.ends_with("as generation 2. undo") {
        log!("c-build: cc build hello: {first:?}");
        return Err("c-build: the first build was not installed as generation 2");
    }
    log!("c-build: cc build hello built 0.0.1, and pkg installed it from local as generation 2");
    let undone = line("undo")?;
    if !undone.contains("packages back to generation 1") {
        log!("c-build: undo: {undone:?}");
        return Err("c-build: undo did not switch back to generation 1");
    }
    let second = line("cc build hello")?;
    if !second.starts_with("ok built hello 0.0.2 (") {
        log!("c-build: the second cc build hello: {second:?}");
        return Err("c-build: the second build was not 0.0.2");
    }
    log!("c-build: undo switched back to generation 1, and the next build is 0.0.2");
    Ok(())
}

/// `text` sent to the shell as the terminal sends it: the reply.
fn line(text: &str) -> Result<String, &'static str> {
    let reply = ipc::call("shell", text.as_bytes(), BUILD_MS).map_err(|error| {
        log!("c-build: no reply to {text:?}: {error:?}");
        "c-build: a line got no reply"
    })?;
    Ok(String::from_utf8_lossy(&reply).into_owned())
}

/// `/home/user/c/hello/hello.c`, in one action.
fn write_sources() -> Result<(), &'static str> {
    let home = kabi::fs::HOME_USER;
    let done = |reply: Reply| match reply {
        Reply::Done => Ok(()),
        _ => Err("c-build: fs refused a request"),
    };
    done(ask(home, Request::Begin { action: 13 })?)?;
    for folder in ["c", "c/hello"] {
        let open = Request::Open {
            path: folder.into(),
            rights: (Rights::READ | Rights::ENUMERATE).bits(),
            how: How::Mkdir,
        };
        if !matches!(ask(home, open)?, Reply::Opened(..)) {
            return Err("c-build: a folder cannot be made");
        }
    }
    let open = Request::Open {
        path: "c/hello/hello.c".into(),
        rights: (Rights::READ | Rights::WRITE).bits(),
        how: How::CreateFile,
    };
    let Reply::Opened(_, file) = ask(home, open)? else {
        return Err("c-build: hello.c cannot be made");
    };
    let write = Request::Write {
        off: 0,
        data: SOURCE.as_bytes().to_vec(),
    };
    done(ask(file, write)?)?;
    match ask(home, Request::Commit)? {
        Reply::Committed { .. } => Ok(()),
        _ => Err("c-build: the sources did not commit"),
    }
}
