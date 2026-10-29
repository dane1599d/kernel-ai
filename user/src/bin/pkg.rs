//! The `pkg` service (P4 task 1; ADR 0011 §5, §8, §12): kaipkg's store and
//! generations on kaifs, through the `Dir` handle for `/system/pkg` it gets at
//! startup and `fs` serves (`kaipkg::fsvol`), and the image's signed repository,
//! which it reads with BOOT_READ beneath `pkgrepo/` (its plan line's
//! `bootfiles=pkgrepo`) and checks with `kaituf` (`kaipkg::image`).
//!
//! For boot case `pkg-image`, `step=image`, `image-again` and `image-tampered`
//! each check the image's repository, print each package it refuses by name,
//! and make the rest generation 1 if the store has no generation yet. Each first
//! shows that BOOT_READ refuses every path outside `pkgrepo/`. Then each app of
//! the current generation is added as a package version of its component
//! (COMP_ADD, `p-` and its SHA-256's first 12 hex digits), and the supervisor,
//! through the route `uses=supervisor` gives, starts its `start=pkg` line there.
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
//! `case/<step>` in `/system/pkg`, prints its line, then leaves
//! `case/<step>.done` for the kernel's half, which may end the boot only once
//! the line is out. Then it idles: the supervisor would restart an exited
//! service. A failure prints `pkg: FAIL ...`.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use fsproto::{Reply, Request};
use kabi::Error;
use kabi::handle::{Handle, HandleInfo, Kind, MAX_STARTUP, MAX_TRANSFER};
use kaipkg::bundle;
use kaipkg::fsvol::{FsVolume, Transport};
use kaipkg::generation::{Generation, Time};
use kaipkg::header::{Header, Kind as PackageKind};
use kaipkg::image::Files;
use kaipkg::resolve::{Entry, Want};
use kaipkg::store::{self, Ctx, Ledger, Source, Store, Volume};
use kaipkg::version::Version;
use user::{println, sys};

user::entry!(main);

const CALL_MS: u64 = 10_000;
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
        Some(step @ ("image" | "image-again" | "image-tampered")) => image(&mut vol, step),
        _ => Err(String::from("no step=install, undo, check or image")),
    };
    match result {
        Ok(line) => println!("pkg: {line}"),
        Err(why) => println!("pkg: FAIL {why}"),
    }
    // The line is out: the kernel's half may end the boot now, not while the
    // console still prints it.
    if let Some(step) = step {
        let done = vol
            .write(&format!("pkg/case/{step}.done"), b"")
            .and_then(|()| vol.commit());
        if let Err(why) = done {
            println!("pkg: FAIL cannot leave case/{step}.done: {why}");
        }
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

/// Where the image's repository lies in the boot bundle.
const REPO: &str = "pkgrepo/";

/// The image's repository, read with BOOT_READ.
struct BootRepo;

impl Files for BootRepo {
    fn file(&mut self, path: &str, max: u64) -> Result<Option<Vec<u8>>, String> {
        let path = format!("{REPO}{path}");
        let len = match sys::boot_read(&path, &mut []) {
            Ok(len) => len,
            Err(Error::NotFound) => return Ok(None),
            Err(e) => return Err(format!("BOOT_READ {path}: {e:?}")),
        };
        if len as u64 > max {
            return Err(format!("{path} is {len} bytes, over {max}"));
        }
        let mut bytes = alloc::vec![0u8; len];
        match sys::boot_read(&path, &mut bytes) {
            Ok(got) if got == len => Ok(Some(bytes)),
            Ok(_) => Err(format!("{path} changed while it was read")),
            Err(e) => Err(format!("BOOT_READ {path}: {e:?}")),
        }
    }
}

/// BOOT_READ answers Permission outside `pkgrepo/`: the plan line grants that
/// folder and nothing else of the boot bundle.
fn outside_refused() -> Result<(), String> {
    for path in [
        "kernel.lines",
        "user/pkg.elf",
        "pkgrepo",
        "pkgrepo/../kernel.lines",
        "/pkgrepo/metadata/1.root.json",
    ] {
        match sys::boot_read(path, &mut []) {
            Err(Error::Permission) => {}
            other => return Err(format!("BOOT_READ {path:?} gave {other:?}, not Permission")),
        }
    }
    println!("pkg: BOOT_READ refuses every path outside pkgrepo/");
    Ok(())
}

/// `step=image*`: the image's repository checked, its refusals printed, and
/// the rest made generation 1 if the store has no generation.
fn image(vol: &mut FsVolume<Calls>, step: &str) -> Result<String, String> {
    outside_refused()?;
    let store = Store::default();
    let mut ledger = NoLedger;
    let time = now(vol);
    let checked = kaipkg::image::check(&mut BootRepo, time.0, kaipkg::PROVIDED)
        .map_err(|why| format!("the image's repository is refused: {why}"))?;
    for (path, why) in &checked.refused {
        println!("pkg: the image's {path} is refused: {why}");
    }
    let mut cx = Ctx {
        vol,
        ledger: &mut ledger,
        now: time,
        action: 1,
    };
    let seeded = store
        .seed(&mut cx, &checked.packages)
        .map_err(|e| format!("{e:?}"))?;
    let current = store::current(cx.vol)?;
    let listed = names(cx.vol, current)?;
    let listed = match listed.is_empty() {
        true => "no packages".to_string(),
        false => listed.join(", "),
    };
    let verified = format!(
        "the image's repository verified, {} kept and {} refused",
        checked.packages.len(),
        checked.refused.len()
    );
    let line = match seeded {
        true => format!("{verified}; generation 1 is its packages: {listed}"),
        false => format!("{verified}; generation {current} stays current: {listed}"),
    };
    let summary = store::check(cx.vol, &store, time)?;
    println!(
        "pkg: {line} (store checks: current {}, {} object(s))",
        summary.current,
        summary.objects.len()
    );
    let started = start_apps(cx.vol, current)?;
    let line = match started.is_empty() {
        true => format!("generation {current} holds no app to start"),
        false => format!("started from generation {current}: {}", started.join(", ")),
    };
    cx.vol.write(&format!("pkg/case/{step}"), line.as_bytes())?;
    Ok(line)
}

/// The supervisor's route, from the plan line's `uses=supervisor`.
fn supervisor() -> Handle {
    user::route(0).unwrap_or(Handle::NONE)
}

/// Each app of generation `n`, its artifact added as a package version of its
/// component (COMP_ADD, which only adds: Busy when it is there already) and
/// started on it by the supervisor (`run`); `name version as label` for each.
fn start_apps(vol: &mut dyn Volume, n: u64) -> Result<Vec<String>, String> {
    let mut started = Vec::new();
    for package in store::generation(vol, n)?.packages {
        let name = &package.name;
        let object = vol
            .read(&store::object_path(&package.sha))?
            .ok_or_else(|| format!("{name}'s object is not in the store"))?;
        let files = bundle::files(&object).map_err(|why| format!("{name}: {why}"))?;
        let header = files
            .first()
            .and_then(|(_, text)| core::str::from_utf8(text).ok())
            .ok_or_else(|| format!("{name}: no header"))?;
        if Header::parse(header)?.kind != PackageKind::App {
            continue;
        }
        let artifact = files
            .iter()
            .find(|(path, _)| *path == bundle::ARTIFACT)
            .map(|(_, bytes)| *bytes)
            .ok_or_else(|| format!("{name}: an app without {}", bundle::ARTIFACT))?;
        let sha = kaipkg::sha256(artifact);
        match sys::comp_add(name, artifact, &sha) {
            Ok(()) | Err(Error::Busy) => {}
            Err(e) => return Err(format!("COMP_ADD {name}: {e:?}")),
        }
        let label = kabi::live::package_label(&sha);
        let run = supervise::Run {
            name,
            version: &label,
        };
        let mut reply = [0u8; 256];
        let len = sys::call_route(supervisor(), run.encode().as_bytes(), &mut reply, CALL_MS)
            .map_err(|e| format!("the supervisor did not answer {name}'s run: {e:?}"))?;
        let reply = core::str::from_utf8(&reply[..len.min(reply.len())]).unwrap_or("");
        let version = package.version;
        // Another version runs it: a local override (its plan line is not
        // `start=pkg`) or one already serving. Shown, never hidden.
        let shadowed = match reply {
            "ok" => None,
            supervise::NOT_FROM_PKG => Some("its plan line runs a local version"),
            supervise::RUNS_ANOTHER => Some("it already runs another version"),
            _ => return Err(format!("the supervisor refused {name}'s run: {reply}")),
        };
        match shadowed {
            Some(why) => println!(
                "pkg: {name} {version} ({label}) is installed but not started from generation {n}: {why}"
            ),
            None => started.push(format!("{name} {version} as {label}")),
        }
    }
    Ok(started)
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
                    kaipkg::PROVIDED,
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
