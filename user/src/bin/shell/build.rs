//! `cc build`'s host half (P4 task 13; `shellcore::ccbuild` is the rest):
//!
//! - the component's `.c` and `.h` files from `/home/user/c/<name>/`, read
//!   whole, at most `MAX_SOURCE` in all;
//! - sent to `cc` in an `Shm` (`ccserve::build::shm`), which cc writes the
//!   artifact over. One `Shm` serves every build: its pages count against
//!   the shell's quota until it ends;
//! - the artifact checked (its name, version and source hash) and written to
//!   `/home/user/c/<name>.kai` in an action of its own;
//! - handed to `pkg`'s `local` as a read-only copy of the File handle it
//!   was written through, not opened again by its path.

use crate::files::{self, Files};
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use ccserve::build::shm;
use fsproto::{How, Reply, Request};
use kabi::handle::{Handle, Rights};
use kabi::live::MAX_ARTIFACT;
use livecore::artifact::MAX_SOURCE;
use promptcore::wire::{Record, parse_reply};
use shellcore::ccbuild::{CcBuilt, DIR, NO_ANSWER};
use spin::Mutex;
use user::sys;

/// cc's wait for a build: its deadline is ten line budgets (20 s, or 120 s
/// when QEMU emulates the CPU), and its first request translates cc.wasm.
const BUILD_CALL_MS: u64 = 150_000;
/// `pkg`'s wait for `local`: it signs, checks, installs and starts the app.
const PKG_CALL_MS: u64 = 60_000;
/// `pkg` reads requests into 64 bytes, and answers in a few hundred.
const PKG_REPLY: usize = 512;

/// The `Shm` every build uses, made by the first.
static SHM: Mutex<Option<Handle>> = Mutex::new(None);
/// The last artifact written, and the File handle it was written through.
static WRITTEN: Mutex<Option<(String, Handle)>> = Mutex::new(None);

/// [`DIR`] beneath `/home/user`, where the shell's `Dir` is.
fn folder() -> &'static str {
    DIR.strip_prefix("/home/user/").unwrap_or("c")
}

/// Builds component `name` at `version` through `cc`, and writes its artifact.
pub fn build(files: &Files, cc: Handle, name: &str, version: &str) -> Result<CcBuilt, String> {
    let home = files.home_dir()?;
    let sources = sources(home, &format!("{}/{name}", folder()))?;
    let named: Vec<(&str, &[u8])> = sources
        .iter()
        .map(|(file, bytes)| (file.as_str(), bytes.as_slice()))
        .collect();
    let bundle = livecore::artifact::bundle(&named);
    if bundle.len() > MAX_SOURCE {
        return Err(format!("the sources pass {} KiB", MAX_SOURCE >> 10));
    }
    let (text, artifact) = ask_cc(cc, name, version, &bundle)?;
    let built = CcBuilt::from_reply(&text, format!("{DIR}/{name}.kai"))?;
    check(&built, &artifact, name, version)?;
    let path = format!("{}/{name}.kai", folder());
    let file = files
        .put_whole(&path, &artifact)
        .map_err(|why| format!("{DIR}/{name}.kai cannot be written: {why}"))?;
    if let Some((_, old)) = WRITTEN.lock().replace((built.path.clone(), file)) {
        files::close(old);
    }
    Ok(built)
}

/// What cc wrote is what it said, for `name` at `version`: its length, its
/// manifest's name and world, and the hash of the sources it carries.
fn check(built: &CcBuilt, artifact: &[u8], name: &str, version: &str) -> Result<(), String> {
    if built.bytes != artifact.len() {
        return Err(format!(
            "cc said {} bytes but wrote {}",
            built.bytes,
            artifact.len()
        ));
    }
    let parsed = livecore::artifact::parse(artifact)
        .map_err(|why| format!("cc's artifact does not load: {why}"))?;
    let manifest = &parsed.manifest;
    let source = kaipkg::hex(&kaipkg::sha256(parsed.source));
    let world = format!("app:{name}/{name}@{version}");
    if manifest.component != name || manifest.world != world {
        return Err(format!("cc built {}, not {world}", manifest.world));
    }
    if manifest.source != built.source() || source != built.source() {
        return Err(String::from(
            "the artifact's sources are not the ones cc named",
        ));
    }
    Ok(())
}

/// The `.c` and `.h` files in the folder at `path` beneath `home`, read whole.
fn sources(home: Handle, path: &str) -> Result<Vec<(String, Vec<u8>)>, String> {
    let dir = files::open(home, path, How::Existing, Rights::READ | Rights::ENUMERATE)
        .map_err(|why| format!("/home/user/{path}/ cannot be read: {why}"))?;
    let read = read_sources(dir);
    files::close(dir);
    let read = read?;
    if read.is_empty() {
        return Err(format!("/home/user/{path}/ holds no .c or .h file"));
    }
    Ok(read)
}

fn read_sources(dir: Handle) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut read = Vec::new();
    let mut total = 0;
    let wanted = |name: &&String| name.ends_with(".c") || name.ends_with(".h");
    let names = files::list(dir)?;
    // The listing stops at LIST_MAX: past it a source would be left out.
    if names.len() >= files::LIST_MAX {
        return Err(format!(
            "the folder holds {} names or more, more than a build reads",
            files::LIST_MAX
        ));
    }
    for name in names.iter().filter(wanted) {
        let file = files::open(dir, name, How::Existing, Rights::READ)?;
        let bytes = read_whole(file, MAX_SOURCE - total);
        files::close(file);
        let bytes = bytes.map_err(|why| format!("{name}: {why}"))?;
        total += bytes.len();
        read.push((name.clone(), bytes));
    }
    Ok(read)
}

/// The file's bytes, refused past `max`.
fn read_whole(file: Handle, max: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    loop {
        let request = Request::Read {
            off: bytes.len() as u64,
            len: fsproto::MAX_DATA as u32,
        };
        match files::ask(file, request)?.0 {
            Reply::Data(data) => {
                let short = data.len() < fsproto::MAX_DATA;
                bytes.extend_from_slice(&data);
                if bytes.len() > max {
                    return Err(format!("the sources pass {} KiB", MAX_SOURCE >> 10));
                }
                if short {
                    return Ok(bytes);
                }
            }
            other => return Err(files::unexpected(&other)),
        }
    }
}

/// `build <name> <version>` to `cc` with `bundle` in an `Shm`: cc's text, and
/// the artifact it wrote back.
fn ask_cc(
    cc: Handle,
    name: &str,
    version: &str,
    bundle: &[u8],
) -> Result<(String, Vec<u8>), String> {
    let shm = {
        let mut held = SHM.lock();
        match *held {
            Some(shm) => shm,
            None => {
                let made = sys::shm_create(shm::PAGES as u64)
                    .map_err(|e| format!("no Shm for the sources ({e:?})"))?;
                *held = Some(made);
                made
            }
        }
    };
    with_shm(shm, |bytes| {
        shm::put(bytes, bundle).map_err(String::from)?;
        // cc maps and writes it, and nothing more.
        let rights = Rights::MAP | Rights::READ | Rights::WRITE | Rights::TRANSFER;
        let copy = sys::handle_dup(shm, rights).map_err(|e| format!("the Shm ({e:?})"))?;
        let request = promptcore::wire::Request::Build {
            component: name,
            version,
        }
        .text();
        let mut reply = vec![0u8; kabi::MAX_MESSAGE];
        let len = match sys::call_h(
            cc,
            request.as_bytes(),
            &mut reply,
            BUILD_CALL_MS,
            &[copy],
            &mut [],
        ) {
            Ok((len, _)) => len.min(reply.len()),
            Err(e) => {
                let _ = sys::handle_close(copy);
                return Err(format!("cc did not answer the build ({e:?})"));
            }
        };
        let text = cc_text(&reply[..len])?;
        let artifact = shm::take(bytes, MAX_ARTIFACT).map_err(String::from)?;
        Ok((text, artifact.to_vec()))
    })
}

/// Runs `f` on `shm`, mapped read-write.
fn with_shm<T>(shm: Handle, f: impl FnOnce(&mut [u8]) -> Result<T, String>) -> Result<T, String> {
    let address = sys::shm_map(shm, true).map_err(|e| format!("the Shm ({e:?})"))?;
    // SAFETY: the kernel mapped the Shm, `shm::PAGES` long, read-write at
    // `address`; this slice is its only reference here, and it ends before the
    // unmap.
    let bytes = unsafe { core::slice::from_raw_parts_mut(address as *mut u8, shm::BYTES) };
    let result = f(bytes);
    let _ = sys::shm_unmap(address);
    result
}

/// cc's `build` reply: its `text`, or why there is none.
fn cc_text(reply: &[u8]) -> Result<String, String> {
    let records = parse_reply(reply).map_err(|e| format!("cc's answer: {e}"))?;
    match records.last() {
        Some(Record::Text(text)) => Ok(String::from_utf8_lossy(text).into_owned()),
        Some(Record::Err { text, .. }) => Err(String::from_utf8_lossy(text).into_owned()),
        Some(Record::Trap { reason, .. }) => Err(format!("the build stopped: {reason}")),
        _ => Err(String::from("cc's answer was not a build's")),
    }
}

/// Hands the artifact at `path` to `pkg`'s `local` as a File that can only be
/// read: the text after its `ok`.
pub fn install(pkg: Handle, name: &str, path: &str) -> Result<String, String> {
    let file = match WRITTEN.lock().take() {
        Some((written, file)) if written == path => file,
        // Only what this build wrote goes to pkg, never a file found by path.
        other => {
            if let Some((_, old)) = other {
                files::close(old);
            }
            return Err(format!("{path} is not the artifact this build wrote"));
        }
    };
    let read_only = sys::handle_dup(file, Rights::READ | Rights::TRANSFER);
    files::close(file);
    let read_only = read_only.map_err(|e| format!("the artifact's File ({e:?})"))?;
    let mut reply = [0u8; PKG_REPLY];
    let request = format!("local {name}");
    let len = match sys::call_h(
        pkg,
        request.as_bytes(),
        &mut reply,
        PKG_CALL_MS,
        &[read_only],
        &mut [],
    ) {
        Ok((len, _)) => len.min(reply.len()),
        Err(e) => {
            let _ = sys::handle_close(read_only);
            return Err(format!("{NO_ANSWER} ({e:?})"));
        }
    };
    pkg_answer(&reply[..len])
}

/// One request to `pkg`: the text after its `ok`.
pub fn ask_pkg(pkg: Handle, request: &str) -> Result<String, String> {
    let mut reply = [0u8; PKG_REPLY];
    let len = sys::call_route(pkg, request.as_bytes(), &mut reply, PKG_CALL_MS)
        .map_err(|e| format!("{NO_ANSWER} ({e:?})"))?;
    pkg_answer(&reply[..len.min(reply.len())])
}

fn pkg_answer(reply: &[u8]) -> Result<String, String> {
    let text = core::str::from_utf8(reply).map_err(|_| String::from("pkg's answer is not text"))?;
    match text.split_once(' ') {
        Some(("ok", rest)) => Ok(String::from(rest)),
        Some(("err", why)) => Err(String::from(why)),
        _ if text == "ok" => Ok(String::new()),
        _ => Err(format!("pkg answered {text:?}")),
    }
}
