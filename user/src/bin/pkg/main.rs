//! The `pkg` service (P4 task 1; ADR 0011 §5, §8, §12): kaipkg's store and
//! generations on kaifs, through the `Dir` handle for `/system/pkg` it gets at
//! startup and `fs` serves (`kaipkg::fsvol`), and the image's signed repository,
//! which it reads with BOOT_READ beneath `pkgrepo/` (its plan line's
//! `bootfiles=pkgrepo`) and checks with `kaituf` (`kaipkg::image`).
//!
//! With no `step=`, as the v0.1 system plans it (`kabi::plan::system`), it does
//! what `step=image` does, the image's repository as generation 1 if the store
//! has none and its apps started, and leaves no case file; then it serves.
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
use kabi::handle::{Handle, HandleInfo, Kind, MAX_STARTUP, MAX_TRANSFER, RecvInfo};
use kaipkg::bundle;
use kaipkg::catalog;
use kaipkg::fsvol::{FsVolume, Transport};
use kaipkg::generation::{Generation, Time};
use kaipkg::header::{Header, Kind as PackageKind};
use kaipkg::image::Files;
use kaipkg::index::Index;
use kaipkg::mirror::{self, Exchange, Mirror, TufCache};
use kaipkg::resolve::{Entry, Want};
use kaipkg::store::{self, Ctx, Ledger, Source, Store, Volume};
use kaipkg::version::Version;
use kaituf::updater::Cache;
use user::{println, sys};

mod local;

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
    if step == Some("not-holder") {
        not_holder();
    }
    let Some(root) = startup_dir() else {
        println!("pkg: FAIL no Dir handle for /system/pkg at startup");
        idle();
    };
    let mut vol = FsVolume::new(Calls, root);
    let result = match step {
        Some(step @ ("install" | "undo" | "check")) => run(&mut vol, step),
        Some(step @ ("image" | "image-again" | "image-tampered")) => image(&mut vol, Some(step)),
        Some(step @ ("mirror" | "mirror-again" | "mirror-tampered" | "mirror-rollback")) => {
            mirror(&mut vol, step)
        }
        // The system's boot: the image's repository, no case file.
        None => image(&mut vol, None),
        Some(other) => Err(format!(
            "step={other} is not install, undo, check, image or mirror"
        )),
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
    serve(|text, moved, caller| {
        local::answer(&mut vol, text, moved, caller).unwrap_or_else(|| answer(&mut vol, text))
    })
}

/// `step=not-holder`: `pkg` from a plan line without `dir=pkg`, so it holds no
/// `/system/pkg`. Each call of lane P's block must be Permission, each with
/// arguments the holder's would pass, so a dropped gate would let it through.
/// It answers `ok` or what went wrong on its endpoint: the kernel's half asks,
/// rather than racing its line to the end of the boot.
fn not_holder() -> ! {
    let mut buf = [0u8; 16];
    let request = kabi::pkgrepo::request("/metadata/timestamp.json", 0, 99).unwrap_or_default();
    let artifact = b"not a package";
    let calls = [
        (
            "BOOT_READ",
            sys::boot_read("pkgrepo/metadata/1.root.json", &mut buf).map(|_| ()),
        ),
        (
            "COMP_ADD",
            sys::comp_add("notes", artifact, &kaipkg::sha256(artifact)),
        ),
        (
            "PKG_FETCH",
            pkg_fetch(request.as_bytes(), &mut buf).map(|_| ()),
        ),
        ("PKG_TIME", sys::pkg_time().map(|_| ())),
    ];
    let wrong: Vec<String> = calls
        .iter()
        .filter(|(_, got)| !matches!(got, Err(Error::Permission)))
        .map(|(name, got)| format!("{name} gave {got:?}, not Permission"))
        .collect();
    let line = match wrong.is_empty() {
        true => String::from("ok"),
        false => wrong.join("; "),
    };
    println!("pkg: without /system/pkg: {line}");
    serve(|_, _, _| line.clone())
}

/// What the image's repository and the mirror listed at this boot, as index
/// entries, for the catalogue.
static KNOWN: spin::Mutex<(Vec<Index>, Vec<Index>)> = spin::Mutex::new((Vec::new(), Vec::new()));

/// After its step, `pkg` answers on its endpoint with `answer`, given each
/// request, the handles it moved and the pid of the process that sent it: the
/// Apps sheet's `catalog <offset>` (P4 task 3) and `cc build`'s `local`
/// requests (P4 task 13, [`local`]). Every handle a request moved is closed
/// once it is answered, and an answer is at most one message. With no `serve`
/// in its plan line it idles.
fn serve(mut answer: impl FnMut(&str, &[Handle], u64) -> String) -> ! {
    let mut request = [0u8; 64];
    loop {
        let mut info = RecvInfo::default();
        match sys::recv_h(&mut request, 60_000, &mut info) {
            Ok(len) => {
                let text = core::str::from_utf8(&request[..len.min(request.len())]).unwrap_or("");
                let count = (info.handle_count as usize).min(MAX_TRANSFER);
                let moved: Vec<Handle> = info.handles[..count].iter().map(|h| Handle(*h)).collect();
                let mut reply = answer(text, &moved, info.caller_pid);
                for handle in moved {
                    let _ = sys::handle_close(handle);
                }
                if reply.len() > kabi::MAX_MESSAGE {
                    let mut end = kabi::MAX_MESSAGE;
                    while !reply.is_char_boundary(end) {
                        end -= 1;
                    }
                    reply.truncate(end);
                }
                if let Err(why) = sys::svc_reply(reply.as_bytes()) {
                    println!("pkg: cannot answer {text:?}: {why:?}");
                }
            }
            Err(Error::TimedOut) => {}
            Err(_) => idle(),
        }
    }
}

/// `catalog` or `catalog <offset>`: one page of `kai-catalog 1`.
fn answer(vol: &mut dyn Volume, text: &str) -> String {
    let offset = match text.split_once(' ') {
        None if text == "catalog" => Some(0),
        Some(("catalog", n)) => n.parse::<usize>().ok(),
        _ => None,
    };
    let Some(offset) = offset else {
        return String::from("err unknown request");
    };
    match catalog_rows(vol) {
        Ok(rows) => catalog::page(&rows, offset, kabi::MAX_MESSAGE),
        Err(why) => format!("err {why}"),
    }
}

/// The catalogue's rows: the current generation's packages, each with the index
/// entry this boot saw for its object, or else its own header; then the image's
/// and the mirror's offers.
fn catalog_rows(vol: &mut dyn Volume) -> Result<Vec<catalog::Row>, String> {
    let (image, mirror) = KNOWN.lock().clone();
    let current = store::current(vol)?;
    let mut installed = Vec::new();
    for package in store::generation(vol, current)?.packages {
        let seen = image.iter().chain(&mirror).find(|i| i.sha == package.sha);
        let index = match seen {
            Some(index) => Index {
                grants: package.grants.clone(),
                ..index.clone()
            },
            None => {
                let object = vol
                    .read(&store::object_path(&package.sha))?
                    .ok_or_else(|| format!("{}'s object is not in the store", package.name))?;
                let files = bundle::files(&object).map_err(String::from)?;
                let text = files
                    .first()
                    .and_then(|(_, text)| core::str::from_utf8(text).ok())
                    .ok_or_else(|| format!("{}: no header", package.name))?;
                let header = Header::parse(text)?;
                let effects = kaipkg::effects::declared(&header, &object)?;
                let size = object.len() as u64;
                let grants = package.grants.clone();
                Index::of_header(&header, text, (package.sha, size), grants, effects)
            }
        };
        installed.push(index);
    }
    Ok(catalog::rows(&installed, &image, &mirror))
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
/// The image's repository as generation 1 if the store has none, and its apps
/// started; `step` names the case file it leaves, none at a system boot.
fn image(vol: &mut FsVolume<Calls>, step: Option<&str>) -> Result<String, String> {
    outside_refused()?;
    let store = Store::default();
    let mut ledger = NoLedger;
    let time = now(vol);
    let checked = kaipkg::image::check(&mut BootRepo, kaipkg::PROVIDED)
        .map_err(|why| format!("the image's repository is refused: {why}"))?;
    KNOWN.lock().0 = checked.indexes.clone();
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
    if let Some(step) = step {
        cx.vol.write(&format!("pkg/case/{step}"), line.as_bytes())?;
    }
    Ok(line)
}

/// The package the mirror cases install: a `data` package of two pieces.
const FROM_MIRROR: &str = "big";

/// PKG_FETCH: the fixed `pkgrepo=` mirror, one request at a time.
struct Fetch;

impl Exchange for Fetch {
    fn exchange(&mut self, request: &[u8]) -> Result<Vec<u8>, String> {
        let mut response = alloc::vec![0u8; llmproto::http::MAX_RESPONSE];
        let len = pkg_fetch(request, &mut response).map_err(|e| format!("PKG_FETCH: {e:?}"))?;
        response.truncate(len);
        Ok(response)
    }
}

/// PKG_FETCH of `request`, its answer into `response`.
fn pkg_fetch(request: &[u8], response: &mut [u8]) -> Result<usize, Error> {
    let call = kabi::HttpRequest {
        request_ptr: request.as_ptr() as u64,
        request_len: request.len() as u64,
        response_ptr: response.as_mut_ptr() as u64,
        response_cap: response.len() as u64,
        timeout_ms: CALL_MS,
    };
    sys::pkg_fetch(&call)
}

/// PKG_FETCH sends only what `kabi::pkgrepo::request` builds, the holder's
/// requests too: one byte off it (HTTP/1.0 for HTTP/1.1) is Invalid, refused
/// before the kernel sends anything.
fn bent_request_refused() -> Result<(), String> {
    let request = kabi::pkgrepo::request("/metadata/timestamp.json", 0, 99)?;
    let bent = request.replacen("HTTP/1.1", "HTTP/1.0", 1);
    match pkg_fetch(bent.as_bytes(), &mut [0u8; 16]) {
        Err(Error::Invalid) => {
            println!("pkg: PKG_FETCH refuses a request one byte off kabi::pkgrepo's");
            Ok(())
        }
        other => Err(format!(
            "a request one byte off gave {other:?}, not Invalid"
        )),
    }
}

/// `step=mirror*` (P4 task 2): generation 1 from the image, as `step=image`,
/// then an install from the `pkgrepo=` mirror, checked with `kaituf` against
/// the root the image carries:
/// - `mirror`: [`FROM_MIRROR`] installs, fetched in pieces, after a request one
///   byte off is refused ([`bent_request_refused`]);
/// - `mirror-again` (boot 2): undo, then the same install again, from the store;
/// - `mirror-tampered`, `mirror-rollback`: the mirror is tampered with, and the
///   install is refused by name.
fn mirror(vol: &mut FsVolume<Calls>, step: &str) -> Result<String, String> {
    image(vol, Some(step))?;
    if step == "mirror" {
        bent_request_refused()?;
    }
    let mut cache = TufCache { vol };
    if cache.load("root").map_err(|e| e.to_string())?.is_none() {
        let image_root = BootRepo
            .file(kaipkg::image::ROOT, kaituf::updater::ROOT_MAX)?
            .ok_or("the image has no root for the mirror")?;
        cache.save("root", &image_root).map_err(|e| e.to_string())?;
    }
    let time = match dated(vol) {
        Ok(time) => time,
        Err(why) => return Ok(format!("the mirror is refused: {why}")),
    };
    let mut source = Mirror::new(Fetch);
    let listing = match mirror::index(&mut source, &mut TufCache { vol }, time.0) {
        Ok(listing) => listing,
        Err(why) => return Ok(format!("the mirror's index is refused: {why}")),
    };
    for (path, why) in &listing.refused {
        println!("pkg: the mirror's {path} is refused: {why}");
    }
    KNOWN.lock().1 = listing.indexes.clone();
    let store = Store::default();
    let mut ledger = NoLedger;
    let mut cx = Ctx {
        vol,
        ledger: &mut ledger,
        now: time,
        action: 2,
    };
    let mut undone = String::new();
    if step == "mirror-again" {
        let current = store::current(cx.vol)?;
        let parent = store::generation(cx.vol, current)?.parent;
        let n = store
            .rollback(&mut cx, parent)
            .map_err(|e| format!("{e:?}"))?;
        undone = format!("undo switched to generation {n}; ");
    }
    let want = [Want {
        name: FROM_MIRROR,
        range: None,
    }];
    let before = source.requests;
    let result = store.install(
        &mut cx,
        &mut source,
        &listing.entries,
        kaipkg::PROVIDED,
        &want,
        false,
    );
    let pieces = source.requests - before;
    let line = match result {
        Ok(n) => format!(
            "{undone}{FROM_MIRROR} installed from the mirror as generation {n}, fetched in {pieces} pieces"
        ),
        Err(store::Error::Refused(why)) => {
            format!("the mirror's {FROM_MIRROR} is refused: {why:?}")
        }
        Err(why) => return Err(format!("{why:?}")),
    };
    let summary = store::check(cx.vol, &store, time)?;
    Ok(format!(
        "{line} (store checks: current {})",
        summary.current
    ))
}

/// The supervisor's route, from the plan line's `uses=supervisor`.
fn supervisor() -> Handle {
    user::route(0).unwrap_or(Handle::NONE)
}

/// Each app of generation `n`, its artifact added as a package version of its
/// component (COMP_ADD, which only adds: Busy when it is there already) and
/// started on it by the supervisor (`run`) as its service, its own name or its
/// window's (`kaipkg::app`); `name version as label` for each.
fn start_apps(vol: &mut dyn Volume, n: u64) -> Result<Vec<String>, String> {
    let mut started = Vec::new();
    for package in store::generation(vol, n)?.packages {
        let name = &package.name;
        let Some((service, artifact)) = app_artifact(vol, name, &package.sha)? else {
            continue;
        };
        let sha = kaipkg::sha256(&artifact);
        match sys::comp_add(&service, &artifact, &sha) {
            Ok(()) | Err(Error::Busy) => {}
            Err(e) => return Err(format!("COMP_ADD {service}: {e:?}")),
        }
        let label = kabi::live::package_label(&sha);
        let run = supervise::Run {
            name: &service,
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

/// The service package `name`, whose object is `sha`, runs as and its artifact,
/// if it is an app: an artifact of another app's component is refused
/// (`kaipkg::app::service`).
fn app_artifact(
    vol: &mut dyn Volume,
    name: &str,
    sha: &kaipkg::Sha,
) -> Result<Option<(String, Vec<u8>)>, String> {
    let object = vol
        .read(&store::object_path(sha))?
        .ok_or_else(|| format!("{name}'s object is not in the store"))?;
    let files = bundle::files(&object).map_err(|why| format!("{name}: {why}"))?;
    let header = files
        .first()
        .and_then(|(_, text)| core::str::from_utf8(text).ok())
        .ok_or_else(|| format!("{name}: no header"))?;
    let header = Header::parse(header)?;
    if header.kind != PackageKind::App {
        return Ok(None);
    }
    let service = kaipkg::app::service(&header, &object)
        .map_err(|why| format!("{name}: {why}"))?
        .ok_or_else(|| format!("{name}: an app without {}", bundle::ARTIFACT))?;
    let artifact = files
        .iter()
        .find(|(path, _)| *path == bundle::ARTIFACT)
        .map(|(_, bytes)| bytes.to_vec())
        .ok_or_else(|| format!("{name}: an app without {}", bundle::ARTIFACT))?;
    Ok(Some((service, artifact)))
}

/// Each app generation `from` held that generation `to` does not, withdrawn
/// from the supervisor (`withdraw`), so an undo ends the build it took out;
/// `name as label` for each one that was running.
fn withdraw_apps(vol: &mut dyn Volume, from: u64, to: u64) -> Result<Vec<String>, String> {
    let kept = store::generation(vol, to)?.packages;
    let mut ended = Vec::new();
    for package in store::generation(vol, from)?.packages {
        if kept.iter().any(|k| k.sha == package.sha) {
            continue;
        }
        let name = &package.name;
        let Some((service, artifact)) = app_artifact(vol, name, &package.sha)? else {
            continue;
        };
        let label = kabi::live::package_label(&kaipkg::sha256(&artifact));
        let withdraw = supervise::Run {
            name: &service,
            version: &label,
        };
        let mut reply = [0u8; 256];
        let len = sys::call_route(
            supervisor(),
            withdraw.encode_withdraw().as_bytes(),
            &mut reply,
            CALL_MS,
        )
        .map_err(|e| format!("the supervisor did not answer {name}'s withdraw: {e:?}"))?;
        match core::str::from_utf8(&reply[..len.min(reply.len())]).unwrap_or("") {
            "ok" => ended.push(format!("{name} as {label}")),
            other => println!("pkg: {name} ({label}) is not withdrawn: {other}"),
        }
    }
    Ok(ended)
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
        effects: Vec::new(),
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

/// The time generations are stamped with: the date (PKG_TIME), never before
/// the newest generation ([`Time::at`]).
fn now(vol: &mut dyn Volume) -> Time {
    let newest = store::current(vol)
        .and_then(|n| store::generation(vol, n))
        .map_or(Time(0), |g| g.time);
    Time::at(sys::pkg_time().ok(), newest)
}

/// [`now`] for kaituf's expiry checks on the mirror, which needs the date:
/// without one the time is near 1970, and metadata that expired long ago would
/// pass (a freeze attack), so the mirror is refused, as `updated` stops without
/// TRUST_TIME.
fn dated(vol: &mut dyn Volume) -> Result<Time, String> {
    match sys::pkg_time() {
        Ok(_) => Ok(now(vol)),
        Err(e) => Err(format!(
            "there is no date to check its expiry against (PKG_TIME: {e:?})"
        )),
    }
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
