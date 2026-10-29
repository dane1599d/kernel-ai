//! `test=app-on-demand` (P3 task 12b): an app starts when it is first needed,
//! not at boot. `notes` (version 2) is planned with `start=demand`: once the
//! supervisor has started everything else, Notes is not running. The first
//! request to it waits for it, which makes the kernel send the supervisor
//! `demand notes`; the supervisor starts it, and the request is answered.

use crate::{ipc, log, process, threads};
use alloc::string::String;

const NAME: &str = "notes";
/// Long enough for the supervisor to have started every boot service.
const SETTLE_MS: u64 = 3_000;
/// The first call waits for Notes to start and load: comphost's start and the
/// component's compile, under TCG too.
const CALL_MS: u64 = 60_000;
const WAIT_MS: u64 = 30_000;

/// The plan line: the live `notes` component, version 2, on demand.
pub fn plan() -> String {
    alloc::format!("{} start=demand", kabi::plan::notes("2", None))
}

pub fn check() -> Result<(), &'static str> {
    // The supervisor is up and serving.
    for _ in 0..WAIT_MS / 50 {
        if ipc::server_of("supervisor").is_some() {
            break;
        }
        threads::sleep_ms(50);
    }
    if ipc::server_of("supervisor").is_none() {
        return Err("app-on-demand: the supervisor never came up");
    }
    threads::sleep_ms(SETTLE_MS);
    if !process::named(NAME).is_empty() {
        return Err("app-on-demand: notes ran at boot");
    }
    log!("app-on-demand: notes is not running after boot");
    let add = "kai-call 1\nfunc app:notes/actions@1.0.0#add\nargs {text: \"buy milk\"}\n";
    let reply = ipc::call(NAME, add.as_bytes(), CALL_MS)
        .map(|reply| String::from_utf8_lossy(&reply).into_owned())
        .map_err(|_| "app-on-demand: the first request was not answered")?;
    if !reply.contains("ok(1)") {
        log!("app-on-demand: notes answered {reply}");
        return Err("app-on-demand: notes did not add the note");
    }
    if process::named(NAME).is_empty() {
        return Err("app-on-demand: notes answered but is not running");
    }
    log!("app-on-demand: the first request started notes, and it answered");
    Ok(())
}
