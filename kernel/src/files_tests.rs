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

use crate::handle;
use crate::process;
use crate::{ipc, log, threads};
use alloc::format;
use alloc::vec::Vec;
use captable::Entry;
use fsproto::{How, Reply, Request};
use kabi::handle::Handle;
use kabi::handle::{Kind, Rights};
use kabi::{Error, ExitReason, cap};

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
    if !reread {
        state_holder()?;
        minting()?;
        log!("files: ENDPOINT_MINT, the FILES ceiling and SYSTEM_STATE refuse what they must");
    }
    Ok(())
}

/// `/system/state` goes only to the shell program running as `shell`, and the
/// shell holds it; `filecheck`, with FILES, gets only its home folder.
fn state_holder() -> Result<(), &'static str> {
    use crate::process::supervision::storage_caps_allowed;
    let state = cap::SYSTEM_STATE;
    let refused = [
        ("filecheck", "filecheck"),
        ("shell", "filecheck"),
        ("fs", "fs"),
        ("x", "shell"),
    ];
    if refused
        .iter()
        .any(|(name, program)| storage_caps_allowed(name, program, state))
        || !storage_caps_allowed("shell", "shell", state)
    {
        return Err("files: SYSTEM_STATE is not the shell's alone");
    }
    // With both bits, STARTUP lists the home Dir first, then the state Dir.
    let both = cap::FILES | cap::SYSTEM_STATE;
    let order: Vec<u64> = handle::fs_dirs(both, 1).map(|e| e.badge).collect();
    if order != [kabi::fs::HOME_USER, kabi::fs::SYSTEM_STATE] {
        return Err("files: the startup Dirs are not home first, then state");
    }
    let pid = process::named(CLIENT).first().map(|&(pid, _)| pid);
    let slot = pid
        .and_then(|pid| process::with_table(|t| t.iter().position(|p| p.live() && p.pid == pid)))
        .ok_or("files: filecheck has no slot")?;
    let badges: Vec<u64> = handle::with(slot, |h| {
        h.startup()
            .filter_map(|info| h.table.get(Handle(info.handle)).ok().map(|e| e.badge))
            .collect()
    })
    .map_err(|_| "files: filecheck has no table")?;
    if badges.contains(&kabi::fs::SYSTEM_STATE) || !badges.contains(&kabi::fs::HOME_USER) {
        return Err("files: filecheck's startup Dirs are not just its home folder");
    }
    Ok(())
}

/// ENDPOINT_MINT and REPLY_H's ceiling, on `filecheck`'s table (it idles once
/// its step is done): what ring 3 cannot reach to try.
fn minting() -> Result<(), &'static str> {
    use crate::process::syscall::fs::mint;
    let pid = process::named(CLIENT).first().map(|&(pid, _)| pid);
    let fs = process::named(SERVICE).first().map(|&(pid, _)| pid);
    let (Some(pid), Some(fs)) = (pid, fs) else {
        return Err("files: filecheck or fs is gone");
    };
    let slot = process::with_table(|t| t.iter().position(|p| p.live() && p.pid == pid))
        .ok_or("files: filecheck has no slot")?;
    let put = |kind, rights, object| {
        handle::with(slot, |h| h.table.insert(Entry::new(kind, rights, object)))
            .ok()
            .and_then(Result::ok)
            .map(|h| u64::from(h.0))
            .ok_or("files: a test handle cannot be put in filecheck's table")
    };
    let (dir, file) = (Kind::Dir as u64, Kind::File as u64);
    let read = Rights::READ.bits() as u64;
    let others = put(Kind::Endpoint, Rights::MINT, fs)?;
    let own = put(Kind::Endpoint, Rights::MINT, pid)?;
    let not_endpoint = put(Kind::Dir, Rights::READ, 1)?;
    for (what, got, want) in [
        (
            "no endpoint",
            mint(slot, 0, file, read, 1),
            Error::BadHandle,
        ),
        (
            "a Dir as endpoint",
            mint(slot, not_endpoint, file, read, 1),
            Error::BadHandle,
        ),
        (
            "another's endpoint",
            mint(slot, others, file, read, 1),
            Error::Permission,
        ),
        (
            "a Partition",
            mint(slot, own, Kind::Partition as u64, read, 1),
            Error::Invalid,
        ),
        ("no rights", mint(slot, own, file, 0, 1), Error::Invalid),
        (
            "only MINT and CALL",
            mint(
                slot,
                own,
                dir,
                (Rights::MINT | Rights::CALL).bits() as u64,
                1,
            ),
            Error::Invalid,
        ),
    ] {
        if got != Err(want) {
            log!("files: ENDPOINT_MINT with {what} gave {got:?}, wanted {want:?}");
            return Err("files: ENDPOINT_MINT allowed what it must refuse");
        }
    }
    // A good one: masked rights, the badge as given, routed to this service.
    let wide = (Rights::READ | Rights::MINT | Rights::CREATE).bits() as u64;
    let minted = mint(slot, own, file, wide, 0xb0b).map_err(|_| "files: a good mint failed")?;
    let route = handle::route_id(CLIENT).map_err(|_| "files: no route id")?;
    let entry = handle::with(slot, |h| h.table.get(Handle(minted as u32)).copied())
        .ok()
        .and_then(Result::ok)
        .ok_or("files: the minted handle is not in the table")?;
    if (
        entry.kind,
        entry.rights,
        entry.badge,
        entry.object,
        entry.grant,
    ) != (Kind::File, Rights::READ, 0xb0b, route, None)
    {
        return Err("files: a minted handle is not what was asked");
    }
    // REPLY_H moves a Dir only to a process with FILES.
    let moving = put(Kind::Dir, Rights::READ | Rights::TRANSFER, 1)?;
    let moving = Handle(moving as u32);
    if handle::take_for(slot, &[moving], cap::SERVE | cap::CALL) != Err(Error::Permission) {
        return Err("files: a Dir would move to a process without FILES");
    }
    handle::take_for(slot, &[moving], cap::FILES)
        .map_err(|_| "files: a Dir did not move to FILES")?;
    for h in [others, own, not_endpoint, minted] {
        let _ = handle::with(slot, |t| t.table.close(Handle(h as u32)));
    }
    Ok(())
}

/// `fs-actions`: `filecheck` makes 200 actions; xtask replays the log.
pub fn actions() -> Result<(), &'static str> {
    wait_for("actions.ok").map_err(|_| "files: filecheck's 200 actions did not finish")?;
    log!("files: 200 file actions are on the disk; the host replays the log");
    Ok(())
}

/// One boot of `fs-powercut`: `filecheck` checks every file after the last cut
/// and leaves `verified-<boot>`, then makes actions until the power goes. Only
/// the last boot ends the case; the others wait for xtask to cut them. With no
/// `boot=N` (`xtask vbox --powercut`) filecheck numbers the boot and xtask reads
/// its lines, so this only waits for the power to go.
pub fn churn(boot: u64, last: bool) -> Result<(), &'static str> {
    if boot == 0 {
        loop {
            threads::sleep_ms(1_000);
        }
    }
    wait_for(&format!("verified-{boot}"))
        .map_err(|_| "files: filecheck did not get through the files after the cut")?;
    log!("files: boot {boot}: every file is whole after the cut");
    if last {
        return Ok(());
    }
    loop {
        threads::sleep_ms(1_000);
    }
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
        Reply::Opened(_, badge) => badge,
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
        Reply::Opened(..) => {}
        _ => return Err("files: the restarted marker cannot be made"),
    }
    wait_for("restart.ok").map_err(|_| "files: filecheck's handle did not work again")?;
    log!(
        "files: fs restarted mid-action: nothing half-done is on disk, and an open File handle works again"
    );
    Ok(())
}

/// `fs-dead-client`: `filecheck` holds an action with `half.txt` in it. While
/// it runs, the kernel's Begin waits (Busy); once it is killed, `fs` aborts its
/// action, the kernel's Begin goes through, and `half.txt` is gone.
pub fn dead_client() -> Result<(), &'static str> {
    let home = kabi::fs::HOME_USER;
    wait_for("half.txt").map_err(|_| "files: filecheck did not open its action")?;
    let begin = || ask(home, Request::Begin { action: 7 });
    if begin()? != Reply::Failed(fsproto::Error::Busy) {
        return Err("files: a live client's action did not hold the next Begin");
    }
    let held: Vec<u64> = process::named(CLIENT)
        .into_iter()
        .map(|(pid, _)| pid)
        .collect();
    let &[pid] = held.as_slice() else {
        return Err("files: not one filecheck to kill");
    };
    process::kill(pid, ExitReason::Killed).map_err(|_| "files: filecheck could not be killed")?;
    wait_within(WAIT_MS, || {
        !process::named(CLIENT).iter().any(|(p, _)| *p == pid)
    })
    .map_err(|_| "files: filecheck did not die")?;
    ok(begin()?).map_err(|_| "files: the dead client's action still holds every Begin")?;
    let half = ask(
        home,
        Request::Open {
            path: "half.txt".into(),
            rights: Rights::READ.bits(),
            how: How::Existing,
        },
    )?;
    if half != Reply::Failed(fsproto::Error::NotFound) {
        return Err("files: the dead client's half-done change is still there");
    }
    ok(ask(home, Request::Abort)?)?;
    log!("files: a client killed mid-action leaves nothing, and the next Begin goes through");
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
                Ok(Reply::Opened(..))
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
