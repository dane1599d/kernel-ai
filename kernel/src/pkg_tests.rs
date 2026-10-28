//! `test=pkg-store [undo|check]` (P2 task 13, the first slice of P4 task 1;
//! ADR 0011 §5, §12): the `pkg` service runs kaipkg's store on kaifs through
//! `fs`, holding only the `Dir` for `/system/pkg` its plan line's `dir=pkg`
//! grants. Three boots on one data disk:
//! - boot 1, `step=install`: generation 1, then `hello` installed as 2;
//! - boot 2, `undo`: 2 is still current; undo switches to 3, 1's packages;
//! - boot 3, `check`: 3 is current.
//!
//! The kernel's half waits for `pkg`'s `case/<step>` marker, then reads
//! `current` through `fs` itself, by the `SYSTEM_PKG` badge, and checks it names
//! the generation that step leaves. On boot 1 it also checks that `pkg` holds
//! no capability bits and starts with that one `Dir`, and no home folder.

use crate::process;
use crate::{handle, ipc, log, threads};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use fsproto::{How, Reply, Request};
use kabi::handle::{Handle, Rights};

const SERVICE: &str = "fs";
const CLIENT: &str = "pkg";
const CALL_MS: u64 = 10_000;
const WAIT_MS: u64 = 180_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Install,
    Undo,
    Check,
}

impl Step {
    /// The word after `test=pkg-store`: none for boot 1. `selftest::from_cmdline`
    /// runs at the top of `kmain`, before the heap exists, so this allocates
    /// nothing: a `Vec` here faulted before the banner and QEMU reset.
    pub fn from_cmdline(cmdline: &str) -> Self {
        let has = |word| cmdline.split_whitespace().any(|w| w == word);
        if has("check") {
            Step::Check
        } else if has("undo") {
            Step::Undo
        } else {
            Step::Install
        }
    }

    fn name(self) -> &'static str {
        match self {
            Step::Install => "install",
            Step::Undo => "undo",
            Step::Check => "check",
        }
    }

    /// The boot it is, and the generation `current` names after it.
    fn boot_and_generation(self) -> (u32, u64) {
        match self {
            Step::Install => (1, 2),
            Step::Undo => (2, 3),
            Step::Check => (3, 3),
        }
    }
}

/// Plan lines: `fs`, then `pkg` with `/system/pkg` and nothing else. `pkg` needs
/// a deeper stack than the 16-page default: fsproto's minicbor decoder
/// recurses, and overflowed it in `Decoder::array`.
pub fn plan(step: Step) -> String {
    format!(
        "{}\n{CLIENT} dir=pkg pages=2048 stack_pages=128 step={}",
        kabi::plan::FS,
        step.name()
    )
}

pub fn check(step: Step) -> Result<(), &'static str> {
    let (boot, generation) = step.boot_and_generation();
    wait_for(&format!("case/{}", step.name()))
        .map_err(|_| "pkg-store: pkg did not finish its step")?;
    let current = read("current")?;
    let current = core::str::from_utf8(&current).map_err(|_| "pkg-store: current is not text")?;
    if current.trim_end().parse::<u64>() != Ok(generation) {
        log!("pkg-store: boot {boot}: current holds {current:?}, wanted {generation}");
        return Err("pkg-store: current names another generation");
    }
    if step == Step::Install {
        only_its_folder()?;
    }
    log!("pkg-store: boot {boot}: fs shows generation {generation} current, as pkg left it");
    Ok(())
}

/// `pkg` holds no capability bits, and its startup table is the one `Dir` its
/// plan line grants: `/system/pkg`, not the home folder.
fn only_its_folder() -> Result<(), &'static str> {
    let pid = process::named(CLIENT).first().map(|&(pid, _)| pid);
    let slot = pid
        .and_then(|pid| process::with_table(|t| t.iter().position(|p| p.live() && p.pid == pid)))
        .ok_or("pkg-store: pkg has no slot")?;
    let (caps, folders) = process::with_table(|t| (t[slot].limits.caps, t[slot].folders));
    if caps != 0 || !folders {
        return Err("pkg-store: pkg holds capability bits, or no plan folder");
    }
    let badges: Vec<u64> = handle::with(slot, |h| {
        h.startup()
            .filter_map(|info| h.table.get(Handle(info.handle)).ok().map(|e| e.badge))
            .collect()
    })
    .map_err(|_| "pkg-store: pkg has no table")?;
    if badges != [kabi::fs::SYSTEM_PKG] {
        log!("pkg-store: pkg's startup badges are {badges:x?}");
        return Err("pkg-store: pkg's startup Dirs are not just /system/pkg");
    }
    log!("pkg-store: pkg holds no capability bits and starts with only /system/pkg's Dir");
    Ok(())
}

/// One request to `fs` on `badge`, as its kernel client.
fn ask(badge: u64, request: Request) -> Result<Reply, &'static str> {
    let bytes = fsproto::encode_request(&request);
    let deadline = crate::hpet::nanos() + CALL_MS * 1_000_000;
    let extra = ipc::Extra {
        badge,
        rights: (Rights::READ | Rights::ENUMERATE).bits(),
        handles: Vec::new(),
    };
    let (reply, _, _) = ipc::call_with(SERVICE, &bytes, 0, deadline, || false, extra)
        .map_err(|_| "pkg-store: a call to fs failed")?;
    fsproto::decode_reply(&reply).map_err(|_| "pkg-store: fs's reply does not decode")
}

/// `/system/pkg/<path>`, whole.
fn read(path: &str) -> Result<Vec<u8>, &'static str> {
    let opened = ask(
        kabi::fs::SYSTEM_PKG,
        Request::Open {
            path: path.into(),
            rights: Rights::READ.bits(),
            how: How::Existing,
        },
    )?;
    let Reply::Opened(stat, file) = opened else {
        return Err("pkg-store: a file in /system/pkg cannot be opened");
    };
    let mut out = Vec::new();
    while (out.len() as u64) < stat.size {
        let request = Request::Read {
            off: out.len() as u64,
            len: fsproto::MAX_DATA as u32,
        };
        match ask(file, request)? {
            Reply::Data(data) if !data.is_empty() => out.extend_from_slice(&data),
            _ => return Err("pkg-store: a read in /system/pkg failed"),
        }
    }
    Ok(out)
}

/// Waits until `/system/pkg/<path>` exists.
fn wait_for(path: &str) -> Result<(), ()> {
    for _ in 0..WAIT_MS / 50 {
        let open = Request::Open {
            path: path.into(),
            rights: 0,
            how: How::Existing,
        };
        if ipc::server_of(SERVICE).is_some()
            && matches!(ask(kabi::fs::SYSTEM_PKG, open), Ok(Reply::Opened(..)))
        {
            return Ok(());
        }
        threads::sleep_ms(50);
    }
    Err(())
}
