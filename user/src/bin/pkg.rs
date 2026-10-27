//! The `pkg` service, its first slice (P4 task 1; ADR 0011 §5, §12): kaipkg's
//! store and generations on kaifs, through the `Dir` handle for `/system/pkg`
//! it gets at startup and `fs` serves (`kaipkg::fsvol`). TUF verification and
//! the image's repository join once `kaituf` lands; until then the one package
//! it installs comes from memory.
//!
//! For boot case `pkg-store` (P2 plan §1), three boots on one data disk, each a
//! `step=`:
//! - `install`: generation 1, empty, if the store has none; then `hello` 1.0.0,
//!   a 300 KiB `data` package fetched in pieces, as generation 2;
//! - `undo` (boot 2): generation 2 is still current, with `hello`'s object
//!   whole; then undo, which switches back to generation 1's packages as
//!   generation 3;
//! - `check` (boot 3): generation 3 is current, without `hello`.
//!
//! After each it checks the store as a whole (`kaipkg::store::check`), leaves
//! `case/<step>` in `/system/pkg` for the kernel's half, and idles: the
//! supervisor would restart an exited service. A failure prints
//! `pkg: FAIL ...`.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use fsproto::{Reply, Request};
use kabi::handle::{Handle, HandleInfo, Kind, MAX_STARTUP, MAX_TRANSFER};
use kaipkg::bundle;
use kaipkg::fsvol::{FsVolume, Transport};
use kaipkg::generation::{Generation, Time};
use kaipkg::resolve::{Entry, Want};
use kaipkg::store::{self, Ctx, Ledger, Source, Store, Volume};
use kaipkg::version::Version;
use user::{println, sys};

user::entry!(main);

const CALL_MS: u64 = 10_000;
/// The OS's `kai:*` packages (ADR 0011 §6).
const PROVIDED: &[(&str, Version)] = &[("kai:app", Version::new(0, 1, 0))];
/// Bytes of `hello`'s payload: more than one of the store's 192 KiB pieces.
const PAYLOAD: usize = 300 << 10;

fn main() -> i32 {
    let mut args = [0u8; 128];
    let len = sys::args(&mut args).unwrap_or(0);
    let args = core::str::from_utf8(&args[..len]).unwrap_or("");
    let step = args
        .split_whitespace()
        .find_map(|w| w.strip_prefix("step="));
    let Some(root) = startup_dir() else {
        println!("pkg: FAIL no Dir handle for /system/pkg at startup");
        idle();
    };
    let mut vol = FsVolume::new(Calls, root);
    let result = match step {
        Some(step @ ("install" | "undo" | "check")) => run(&mut vol, step),
        _ => Err(String::from("no step=install, undo or check")),
    };
    match result {
        Ok(line) => println!("pkg: {line}"),
        Err(why) => println!("pkg: FAIL {why}"),
    }
    idle()
}

fn idle() -> ! {
    loop {
        sys::sleep_ms(60_000);
    }
}

fn startup_dir() -> Option<Handle> {
    let mut handles = [HandleInfo::default(); MAX_STARTUP];
    let count = sys::startup(&mut handles).ok()?;
    handles[..count.min(MAX_STARTUP)]
        .iter()
        .find(|h| h.kind() == Some(Kind::Dir))
        .map(|h| Handle(h.handle))
}

/// CALL_H on the handles `fs` minted.
struct Calls;

impl Transport for Calls {
    type Handle = Handle;

    fn call(&mut self, on: Handle, request: Request) -> Result<(Reply, Option<Handle>), String> {
        let bytes = fsproto::encode_request(&request);
        let mut reply = alloc::vec![0u8; fsproto::MAX_MESSAGE];
        let mut moved = [Handle::NONE; MAX_TRANSFER];
        let (len, came) = sys::call_h(on, &bytes, &mut reply, CALL_MS, &[], &mut moved)
            .map_err(|e| format!("CALL_H: {e:?}"))?;
        let reply = fsproto::decode_reply(&reply[..len]).map_err(|e| format!("a reply: {e:?}"))?;
        Ok((reply, (came > 0).then_some(moved[0])))
    }

    fn close(&mut self, handle: Handle) {
        let _ = sys::handle_close(handle);
    }
}

/// Grants and the `actlog 1` line join the switch's transaction once `pkg`
/// holds `/system/grants` and `/system/log` (P2 task 12's files).
struct NoLedger;

impl Ledger for NoLedger {
    fn switched(&mut self, _: &mut dyn Volume, _: u64, _: &Generation) -> Result<(), String> {
        Ok(())
    }

    fn collected(&mut self, _: &mut dyn Volume, _: &[u64], _: usize) -> Result<(), String> {
        Ok(())
    }
}

/// `hello` 1.0.0: its header, then a payload no two runs differ in.
fn hello() -> Vec<u8> {
    let header = "kai-package 1\nname hello\nversion 1.0.0\nkind data\n\
        summary A file for the pkg-store case\nlicence MIT\n";
    let payload: Vec<u8> = (0..PAYLOAD).map(|i| (i * 31 % 251) as u8).collect();
    bundle::build(&[(bundle::HEADER, header.as_bytes()), ("hello.txt", &payload)])
}

/// Its index entry, as the repository's `custom.kai` will give it.
fn entry(bytes: &[u8]) -> Entry {
    Entry {
        name: "hello".into(),
        version: Version::new(1, 0, 0),
        sha: kaipkg::sha256(bytes),
        size: bytes.len() as u64,
        depends: Vec::new(),
        grants: Vec::new(),
        yanked: false,
    }
}

/// Serves the package from memory, a piece at a time, counting the pieces.
struct Memory<'a> {
    bytes: &'a [u8],
    pieces: usize,
}

impl Source for Memory<'_> {
    fn fetch(&mut self, _: &Entry, offset: u64, len: usize) -> Result<Vec<u8>, String> {
        let start = (offset as usize).min(self.bytes.len());
        let end = (start + len).min(self.bytes.len());
        self.pieces += 1;
        Ok(self.bytes[start..end].to_vec())
    }
}

/// kaifs's way with no wall clock: never before the newest generation.
fn now(vol: &mut dyn Volume) -> Time {
    let newest = store::current(vol)
        .and_then(|n| store::generation(vol, n))
        .map_or(0, |g| g.time.0);
    Time(newest + 1 + sys::time_ns() / 1_000_000_000)
}

/// The packages generation `n` names, as `name version`.
fn names(vol: &mut dyn Volume, n: u64) -> Result<Vec<String>, String> {
    let generation = store::generation(vol, n)?;
    Ok(generation
        .packages
        .iter()
        .map(|p| format!("{} {}", p.name, p.version))
        .collect())
}

fn run(vol: &mut FsVolume<Calls>, step: &str) -> Result<String, String> {
    let store = Store::default();
    let mut ledger = NoLedger;
    let time = now(vol);
    let mut cx = Ctx {
        vol,
        ledger: &mut ledger,
        now: time,
        action: 1,
    };
    let fail = |e: store::Error| format!("{e:?}");
    let bytes = hello();
    let line = match step {
        "install" => {
            store.setup(&mut cx, Vec::new()).map_err(fail)?;
            let mut source = Memory {
                bytes: &bytes,
                pieces: 0,
            };
            let want = [Want {
                name: "hello",
                range: None,
            }];
            let n = store
                .install(
                    &mut cx,
                    &mut source,
                    &[entry(&bytes)],
                    PROVIDED,
                    &want,
                    false,
                )
                .map_err(fail)?;
            if n != 2 || names(cx.vol, n)? != ["hello 1.0.0"] {
                return Err(format!("the install made generation {n}, not 2 with hello"));
            }
            format!(
                "boot 1: hello 1.0.0 installed as generation 2, fetched in {} pieces",
                source.pieces
            )
        }
        "undo" => {
            let current = store::current(cx.vol)?;
            if current != 2 || names(cx.vol, 2)? != ["hello 1.0.0"] {
                return Err(format!(
                    "boot 2 found generation {current} current, not 2 with hello"
                ));
            }
            if !store::present(cx.vol, &kaipkg::sha256(&bytes))? {
                return Err("hello's object is not whole after the reboot".into());
            }
            let parent = store::generation(cx.vol, current)?.parent;
            let n = store.rollback(&mut cx, parent).map_err(fail)?;
            if n != 3 || !names(cx.vol, n)?.is_empty() {
                return Err(format!("undo made generation {n}, not 3 without hello"));
            }
            "boot 2: generation 2 was current with hello's object whole; undo switched to generation 3, generation 1's packages".to_string()
        }
        _ => {
            let current = store::current(cx.vol)?;
            if current != 3 || !names(cx.vol, 3)?.is_empty() {
                return Err(format!(
                    "boot 3 found generation {current} current, not 3 without hello"
                ));
            }
            "boot 3: generation 3 is current, without hello: the undo survived the reboot"
                .to_string()
        }
    };
    let summary = store::check(cx.vol, &store, time)?;
    cx.vol.write(&format!("pkg/case/{step}"), line.as_bytes())?;
    Ok(format!(
        "{line} (store checks: current {}, good {}, generations {:?}, {} object(s))",
        summary.current,
        summary.good,
        summary.generations,
        summary.objects.len()
    ))
}
