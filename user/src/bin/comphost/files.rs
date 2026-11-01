//! The editor app's way to the person's files (P4): what `kai:files/files` queued
//! in the shown version is answered here. The import names a whole file, whatever
//! way the OS finds to move it; this system has no such way yet, so every request
//! is answered with that reason and the editor says it in its status row. The
//! real door (the file's bytes through `desktop` to the shell's `fs_*` rows, so
//! tiers, the log and undo stay the OS's) replaces [`pump`]'s answer and nothing
//! else: the app is not changed.
//!
//! A standby version, rehearsed beside the active one during probation, has its
//! queue emptied and dropped, as for the edit service's.

use alloc::string::String;
use alloc::vec::Vec;
use livecore::host::{Env, Host};
use user::println;
use wasmhost::{FileOp, FilesRequest, LiveInstance};

/// Why a request is answered with an error.
pub const NO_DOOR: &str = "this system has no file door yet";

/// Answers what the shown version queued.
pub fn pump<E: Env<Instance = LiveInstance>>(host: &mut Host<E>) {
    let mut queued: Vec<FilesRequest> = Vec::new();
    host.each_instance(|instance, _, shown| {
        if let LiveInstance::V2(core) = instance {
            let taken = core.take_files();
            if shown {
                queued = taken;
            }
        }
    });
    for request in queued {
        let (verb, path) = match &request.op {
            FileOp::Read { path } => ("read", path),
            FileOp::Write { path, .. } => ("write", path),
        };
        println!(
            "comphost: files: {verb} {path} (person {}): {NO_DOOR}",
            request.user
        );
        let ticket = request.ticket;
        // An answer is a call into the app like a window's input: a transaction of
        // its own, so a fault in `on-done` is the version's, reported and reloaded.
        if crate::ui::serve_shown(host, |core| {
            core.files_reply(ticket, Err(String::from(NO_DOOR)))
        })
        .is_none()
        {
            println!("comphost: files: the answer for ticket {ticket} did not reach the app");
            return;
        }
    }
}
