//! `test=c-build`'s trial check (plan "Kai builds apps", piece 3b): `cc`'s
//! `trial` through the handle rights the shell gives it. Only the kernel checks
//! a handle's rights, so no host test sees this (#617's review found the
//! shell's handle had no WRITE while cc mapped every `Shm` read-write).
//!
//! After the first build, the case reads the artifact the shell wrote,
//! `/home/user/c/hello.kai`, into an `Shm` of its own, and moves `cc` a handle
//! to it with the rights the shell's trial moves: MAP, READ and TRANSFER.
//! - `trial hello` passes: cc maps a trial's `Shm` read-only.
//! - `build hello 0.0.9` through the same handle is refused as no fault of the
//!   program's (`internal: `): a build's `Shm` is mapped read-write, which the
//!   handle does not allow.
//!
//! A handle with no rights in cc's own table keeps the object alive between the
//! calls, so no process's exit frees it (`shm::collect`). The case takes it back
//! at the end.

use crate::files_tests::ask;
use crate::{handle, hpet, ipc, log, memory, process, shm};
use alloc::vec;
use alloc::vec::Vec;
use captable::Entry;
use fsproto::{How, Reply, Request};
use kabi::handle::{Kind, Rights};
use kabi::live::MAX_ARTIFACT;
use mm::PAGE_SIZE;

const CC: &str = "cc";
/// The `Shm`'s pages, as `ccserve::build::shm` sizes it: the artifact's length
/// as a u64, then the artifact.
const PAGES: u64 = (MAX_ARTIFACT as u64 + 8).div_ceil(PAGE_SIZE);
/// What the shell's trial moves to cc (`shell/build.rs`): no WRITE.
const TRIAL_RIGHTS: Rights = Rights::MAP.union(Rights::READ).union(Rights::TRANSFER);
/// The shell's wait for a trial (`shellcore::appbuild::CC_TRIAL_WAIT_MS`).
const CALL_MS: u64 = 90_000;

pub fn check() -> Result<(), &'static str> {
    let artifact = read_artifact()?;
    let cc = cc_slot().ok_or("c-build: cc is not running for the trial")?;
    let object = shm::create_object(PAGES).map_err(|_| "c-build: no Shm for the trial")?;
    let keep = Entry::new(Kind::Shm, Rights::empty(), object);
    let Some(kept) = handle::with(cc, |h| h.table.insert(keep))
        .ok()
        .and_then(Result::ok)
    else {
        shm::collect();
        return Err("c-build: the trial's Shm cannot be kept");
    };
    let result = put(object, &artifact).and_then(|()| tries(object));
    let _ = handle::with(cc, |h| h.table.close(kept));
    shm::collect();
    result
}

/// The trial through a read-only handle, then a build through the same one.
fn tries(object: u64) -> Result<(), &'static str> {
    let reply = ask_cc("trial hello", object)?;
    match record(&reply) {
        Some(("text", report)) if report.starts_with("trial ok: init, started; ") => {
            log!("c-build: cc tried hello through a read-only Shm: {report}");
        }
        other => {
            log!("c-build: trial hello: {other:?}");
            return Err("c-build: cc's trial of hello did not pass");
        }
    }
    let reply = ask_cc("build hello 0.0.9", object)?;
    match record(&reply) {
        Some(("err 0", why)) if why.starts_with("internal: cc cannot map the Shm read-write") => {
            log!("c-build: a build through a read-only Shm is refused: {why}");
            Ok(())
        }
        other => {
            log!("c-build: build hello 0.0.9 through a read-only Shm: {other:?}");
            Err("c-build: a build was not refused a read-only Shm")
        }
    }
}

/// `/home/user/c/hello.kai`, as the shell wrote it after the build.
fn read_artifact() -> Result<Vec<u8>, &'static str> {
    let open = Request::Open {
        path: "c/hello.kai".into(),
        rights: Rights::READ.bits(),
        how: How::Existing,
    };
    let Reply::Opened(_, file) = ask(kabi::fs::HOME_USER, open)? else {
        return Err("c-build: hello.kai cannot be opened");
    };
    let mut bytes = Vec::new();
    loop {
        let read = Request::Read {
            off: bytes.len() as u64,
            len: fsproto::MAX_DATA as u32,
        };
        let Reply::Data(data) = ask(file, read)? else {
            return Err("c-build: hello.kai cannot be read");
        };
        bytes.extend_from_slice(&data);
        if bytes.len() > MAX_ARTIFACT {
            return Err("c-build: hello.kai is larger than an artifact");
        }
        if data.len() < fsproto::MAX_DATA {
            return Ok(bytes);
        }
    }
}

/// cc's slot in the process table.
fn cc_slot() -> Option<usize> {
    let pid = ipc::server_of(CC)?;
    process::with_table(|table| table.iter().position(|p| p.live() && p.pid == pid))
}

/// Writes `artifact`'s length, then `artifact`, into the object through the
/// HHDM, as `ccserve::build::shm::put` lays it out.
fn put(object: u64, artifact: &[u8]) -> Result<(), &'static str> {
    let frames = shm::frames_of(object).ok_or("c-build: the trial's Shm is gone")?;
    let mut data = Vec::with_capacity(8 + artifact.len());
    data.extend_from_slice(&(artifact.len() as u64).to_le_bytes());
    data.extend_from_slice(artifact);
    let hhdm = memory::hhdm();
    for (frame, chunk) in frames.iter().zip(data.chunks(PAGE_SIZE as usize)) {
        // SAFETY: the frame is the object's, alive while cc's rightless handle
        // holds it, and mapped by the HHDM; no process maps the object yet.
        let page =
            unsafe { core::slice::from_raw_parts_mut((hhdm + frame) as *mut u8, chunk.len()) };
        page.copy_from_slice(chunk);
    }
    Ok(())
}

/// `request` to cc with a handle to `object` moved as the shell's trial moves
/// one; the reply.
fn ask_cc(request: &str, object: u64) -> Result<Vec<u8>, &'static str> {
    let extra = ipc::Extra {
        badge: 0,
        rights: 0,
        handles: vec![Entry::new(Kind::Shm, TRIAL_RIGHTS, object)],
    };
    let deadline = hpet::nanos() + CALL_MS * 1_000_000;
    match ipc::call_with(CC, request.as_bytes(), 0, deadline, || false, extra) {
        Ok((reply, _, _)) => Ok(reply),
        Err(error) => {
            log!("c-build: cc did not answer {request:?}: {error:?}");
            Err("c-build: cc did not answer the trial's request")
        }
    }
}

/// A one-record `kai-cc 1` reply: its header but the length ("text", "err 0"),
/// and its text.
fn record(reply: &[u8]) -> Option<(&str, &str)> {
    let reply = core::str::from_utf8(reply).ok()?;
    let (head, rest) = reply.split_once('\n')?;
    let (word, len) = head.rsplit_once(' ')?;
    let len = len.parse::<usize>().ok()?;
    (rest.len() == len).then_some((word, rest))
}
