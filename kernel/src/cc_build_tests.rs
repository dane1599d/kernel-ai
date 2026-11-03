//! P4 task 13's exit case (`test=c-build`): `cc build` in the OS. `pkg` runs
//! `pkg-image`'s step: generation 1 is the image's packages, which run from the
//! image and start no app from a package (#536). The shell has
//! [`kabi::plan::SHELL_BUILD`]'s route to `pkg`.
//!
//! The case writes a C app the image does not ship, `hello`, into
//! `/home/user/c/hello/` as `fs`'s kernel client. Its source lives only here
//! and in that folder, so no image or repository holds it. Then the case types
//! `cc build hello` as the terminal does:
//! - `local-next` gives 0.0.1;
//! - `cc` builds it;
//! - `pkg` publishes it into `local`, signed by the device key it makes, and
//!   installs it as generation 2, then starts it as `p-...`; its `init` logs a
//!   line. The case waits until the app runs that version, which the ledger
//!   says only after `init`, since comphost starts it after `pkg`'s reply,
//!   slowly when QEMU emulates the CPU.
//!
//! `undo` switches back to generation 1, which ends the running app, and the
//! next build is 0.0.2. The ledger still names 0.0.1 active, so its new host
//! swaps to 0.0.2, which `pkg` named (#572), and the case waits until it runs
//! it. The host checks `pkg`'s and the app's lines.

use crate::files_tests::ask;
use crate::pkg_tests::{Step, wait_for};
use crate::process::components;
use crate::shell_tests::start;
use crate::{ipc, log, threads};
use alloc::format;
use alloc::string::String;
use fsproto::{How, Reply, Request};
use kabi::handle::Rights;

/// The built app's name: its component, process and service.
const NAME: &str = "hello";
/// The built app's plan line: started by `pkg`, run by comphost like notes.
pub(crate) const HELLO: &str = "hello program=comphost caps=serve+component pages=8192 stack_pages=512 burst_ms=20000 start=pkg";
/// An app that asks for nothing: what `local` installs. Its `init` logs.
const SOURCE: &str = "#include <app.h>
#include <stdlib.h>
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
// Its state in schema 1, CBOR [1, empty text], which the glue frees after.
// comphost checks the state after init, so an app that saves nothing stops.
bool exports_kai_live_lifecycle_save(uint32_t to_schema, app_list_u8_t *ret) {
    uint8_t *state = malloc(3);
    if (!state) {
        return false;
    }
    state[0] = 0x82;
    state[1] = 0x01;
    state[2] = 0x60;
    ret->ptr = state;
    ret->len = 3;
    return true;
}
uint32_t exports_kai_live_lifecycle_health(void) { return 0; }
uint32_t exports_kai_live_lifecycle_self_test(void) { return 0; }
void exports_kai_app_events_on_event(exports_kai_app_events_event_t *e) {
    exports_kai_app_events_event_free(e);
}
";
/// Time for `pkg` to sync its marker.
pub(crate) const SYNC_MS: u64 = 1_000;
/// A build's line: cc.wasm's translation on the first, the build itself, and
/// `pkg`'s publish, check and install, under TCG too.
pub(crate) const BUILD_MS: u64 = 280_000;
/// Longest wait for the app to run the version `pkg` started, under TCG too.
const UP_MS: u64 = 120_000;
const POLL_MS: u64 = 50;

/// `pkg`'s image step, the model, `cc`, the app's line, and the shell with
/// `updated` just before it, started on demand.
pub fn plan() -> String {
    format!(
        "{}\n{}\n{}\n{HELLO}\n{}",
        crate::pkg_tests::plan(Step::Image),
        crate::llm::plan_line(),
        kabi::plan::CC,
        kabi::plan::with_updated(kabi::plan::SHELL_BUILD, kabi::plan::ON_DEMAND)
    )
}

pub fn check() -> Result<(), &'static str> {
    wait_for(&format!("case/{}.done", Step::Image.name()))
        .map_err(|_| "c-build: pkg did not finish its image step")?;
    // The marker is durable once written, but `pkg` syncs it next, and an
    // action of the kernel's begun first would make that sync Busy.
    threads::sleep_ms(SYNC_MS);
    write_sources()?;
    start()?;
    let first = line("cc build hello")?;
    if !first.starts_with("ok built hello 0.0.1 (") || !first.ends_with("as generation 2. undo") {
        log!("c-build: cc build hello: {first:?}");
        return Err("c-build: the first build was not installed as generation 2");
    }
    log!("c-build: cc build hello built 0.0.1, and pkg installed it from local as generation 2");
    let label = wait_up("0.0.1")?;
    log!("c-build: hello 0.0.1 runs as {label}");
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
    let label = wait_up("0.0.2")?;
    log!("c-build: hello 0.0.2 runs as {label}");
    Ok(())
}

/// Waits until `hello` serves on the package version of `version`, which the
/// ledger names active only after its `init` and a state it saved, so its line
/// is out; that version's label.
pub(crate) fn wait_up(version: &str) -> Result<String, &'static str> {
    let world = format!("app:{NAME}/{NAME}@{version}");
    for _ in 0..UP_MS / POLL_MS {
        let label = components::component(NAME).and_then(|component| {
            let found = component.versions().iter().find(|v| v.world == world);
            found.map(|v| v.label.clone())
        });
        let active = components::ledger(NAME).and_then(|l| l.active().map(String::from));
        if let Some(label) = label
            && active.as_deref() == Some(label.as_str())
            && ipc::server_of(NAME).is_some()
        {
            return Ok(label);
        }
        threads::sleep_ms(POLL_MS);
    }
    log!("c-build: hello {version} did not run within {UP_MS} ms");
    Err("c-build: the built app did not start")
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
pub(crate) fn write_sources() -> Result<(), &'static str> {
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
