//! `test=files [reread]` and `test=files-restart` (P2 task 8, ADR 0004 §6-7): the
//! `fs` service on kaifs, with `filecheck` as its ring-3 client through the `Dir`
//! handle FILES gives.
//! - `fs-reboot`: boot 1's `filecheck step=write` makes files; boot 2, on the
//!   same data disk, reads them back byte for byte (`step=read`).
//! - `fs-restart`: with `filecheck` holding a `File` handle, the kernel opens an
//!   action of its own, writes, and kills `fs` before it commits. The restarted
//!   `fs` must show nothing of that write, and the handle opened before the kill
//!   must work again.
//!
//! The kernel is `fs`'s client here as well, naming objects by their badges: it
//! takes no handles.

use crate::process;
use crate::{ipc, log, threads};
use alloc::format;
use alloc::vec::Vec;
use fsproto::{How, Reply, Request};
use kabi::ExitReason;
use kabi::handle::Rights;

const SERVICE: &str = "fs";
const CLIENT: &str = "filecheck";
/// Plan lines: `fs`, then `filecheck` doing `step`.
pub fn plan(step: &str) -> alloc::string::String {
    format!("{}\n{CLIENT} caps=files step={step}", kabi::plan::FS)
}
const CALL_MS: u64 = 10_000;
const WAIT_MS: u64 = 120_000;
const RIGHTS: Rights = Rights::READ
    .union(Rights::WRITE)
    .union(Rights::ENUMERATE)
    .union(Rights::CREATE)
    .union(Rights::DELETE);

/// `fs-reboot`'s boots: `filecheck` leaves `write.ok` or `read.ok`.
pub fn check(cmdline: &str) -> Result<(), &'static str> {
    let reread = cmdline.split_whitespace().any(|w| w == "reread");
    let (marker, boot) = if reread {
        ("read.ok", 2)
    } else {
        ("write.ok", 1)
    };
    wait_for(marker).map_err(|_| "files: filecheck did not finish its step")?;
    log!("files: boot {boot}: filecheck's step through its Dir handle passed");
    Ok(())
}

/// `fs-restart`.
pub fn restart() -> Result<(), &'static str> {
    wait_for("ready").map_err(|_| "files: filecheck did not get ready")?;
    let file = match ask(
        kabi::fs::HOME_USER,
        Request::Open {
            path: "restart.txt".into(),
            rights: (Rights::READ | Rights::WRITE).bits(),
            how: How::Existing,
        },
    )? {
        Reply::Opened(stat) => kabi::fs::badge(kabi::fs::HOME, true, stat.object),
        _ => return Err("files: restart.txt cannot be opened"),
    };
    // An action left open: written, never committed.
    ok(ask(kabi::fs::HOME_USER, Request::Begin { action: 77 })?)?;
    ok(ask(
        file,
        Request::Write {
            off: 0,
            data: b"half-done".to_vec(),
        },
    )?)?;
    let before: Vec<u64> = process::named(SERVICE)
        .into_iter()
        .map(|(pid, _)| pid)
        .collect();
    let &[pid] = before.as_slice() else {
        return Err("files: not one fs to kill");
    };
    process::kill(pid, ExitReason::Killed).map_err(|_| "files: fs could not be killed")?;
    wait_within(WAIT_MS, || {
        ipc::server_of(SERVICE).is_some_and(|served| served != pid)
    })
    .map_err(|_| "files: fs did not come back")?;
    match ask(file, Request::Read { off: 0, len: 64 })? {
        Reply::Data(data) if data == b"v1" => {}
        Reply::Data(_) => return Err("files: the killed action's write is on disk"),
        _ => return Err("files: restart.txt cannot be read after the restart"),
    }
    match ask(
        kabi::fs::HOME_USER,
        Request::Open {
            path: "restarted".into(),
            rights: Rights::READ.bits(),
            how: How::CreateFile,
        },
    )? {
        Reply::Opened(_) => {}
        _ => return Err("files: the restarted marker cannot be made"),
    }
    wait_for("restart.ok").map_err(|_| "files: filecheck's handle did not work again")?;
    log!(
        "files: fs restarted mid-action: nothing half-done is on disk, and an open File handle works again"
    );
    Ok(())
}

/// One request to `fs` on `badge`, as its kernel client.
fn ask(badge: u64, request: Request) -> Result<Reply, &'static str> {
    let bytes = fsproto::encode_request(&request);
    let deadline = crate::hpet::nanos() + CALL_MS * 1_000_000;
    let extra = ipc::Extra {
        badge,
        rights: RIGHTS.bits(),
        handles: Vec::new(),
    };
    let (reply, _, _) = ipc::call_with(SERVICE, &bytes, 0, deadline, || false, extra)
        .map_err(|_| "files: a call to fs failed")?;
    fsproto::decode_reply(&reply).map_err(|_| "files: fs's reply does not decode")
}

fn ok(reply: Reply) -> Result<(), &'static str> {
    match reply {
        Reply::Done => Ok(()),
        _ => Err("files: fs refused a request"),
    }
}

/// Waits until `/home/user/<name>` exists.
fn wait_for(name: &str) -> Result<(), ()> {
    wait_within(WAIT_MS, || {
        ipc::server_of(SERVICE).is_some()
            && matches!(
                ask(
                    kabi::fs::HOME_USER,
                    Request::Open {
                        path: name.into(),
                        rights: 0,
                        how: How::Existing,
                    }
                ),
                Ok(Reply::Opened(_))
            )
    })
}

fn wait_within(ms: u64, mut done: impl FnMut() -> bool) -> Result<(), ()> {
    for _ in 0..ms / 50 {
        if done() {
            return Ok(());
        }
        threads::sleep_ms(50);
    }
    Err(())
}
