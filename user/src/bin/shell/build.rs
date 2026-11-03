//! `cc build`'s host half (P4 task 13; `shellcore::ccbuild` is the rest):
//!
//! - the component's `.c` and `.h` files from `/home/user/c/<name>/`, read
//!   whole, at most `MAX_SOURCE` in all;
//! - sent to `cc` in an `Shm` (`ccserve::build::shm`), which cc writes the
//!   artifact over. One `Shm` serves every build: its pages count against
//!   the shell's quota until it ends;
//! - the artifact checked against the bundle sent (`shm::check`) and written to
//!   `/home/user/c/<name>.kai` in an action of its own. The file is the
//!   build's own output, so the write replaces the last build's with no log
//!   entry of its own: the line's entry is the install;
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
use shellcore::ccbuild::{APPS_DIR, CcBuilt, NO_ANSWER};
use spin::Mutex;
use user::sys;

/// cc's wait for a build: its deadline is ten line budgets (20 s, or 120 s
/// when QEMU emulates the CPU), and its first request translates cc.wasm.
/// `app_build` reserves the same in each round.
const BUILD_CALL_MS: u64 = shellcore::appbuild::CC_BUILD_WAIT_MS;
/// cc's wait for a trial, which `app_build` reserves too.
const TRIAL_CALL_MS: u64 = shellcore::appbuild::CC_TRIAL_WAIT_MS;
/// `pkg`'s wait for `local`: it signs, checks, installs and starts the app.
const PKG_CALL_MS: u64 = 60_000;
/// `pkg` reads requests into 64 bytes, and answers in a few hundred.
const PKG_REPLY: usize = 512;

/// The `Shm` every build uses, made by the first.
static SHM: Mutex<Option<Handle>> = Mutex::new(None);
/// The last artifact written, and the File handle it was written through.
static WRITTEN: Mutex<Option<(String, Handle)>> = Mutex::new(None);

/// `dir` beneath `/home/user`, where the shell's `Dir` is.
fn folder(dir: &str) -> &str {
    dir.strip_prefix("/home/user/").unwrap_or(dir)
}

/// Builds component `name` at `version` through `cc` from `dir/name/`, and
/// writes its artifact as `dir/name.kai`.
pub fn build(
    files: &Files,
    cc: Handle,
    dir: &str,
    name: &str,
    version: &str,
) -> Result<CcBuilt, String> {
    let home = files.home_dir()?;
    let mut sources = sources(home, &format!("{}/{name}", folder(dir)))?;
    // cc bundles the files sorted by name, so the shell does too: the artifact
    // then carries this very bundle, which `shm::check` compares.
    sources.sort_by(|a, b| a.0.cmp(&b.0));
    let named: Vec<(&str, &[u8])> = sources
        .iter()
        .map(|(file, bytes)| (file.as_str(), bytes.as_slice()))
        .collect();
    let bundle = livecore::artifact::bundle(&named);
    if bundle.len() > MAX_SOURCE {
        return Err(format!("the sources pass {} KiB", MAX_SOURCE >> 10));
    }
    let (text, artifact) = ask_cc(cc, name, version, &bundle)?;
    let built = CcBuilt::from_reply(&text, format!("{dir}/{name}.kai"))?;
    shm::check(
        &artifact,
        &bundle,
        name,
        version,
        built.bytes,
        built.source(),
    )?;
    let path = format!("{}/{name}.kai", folder(dir));
    let file = files
        .put_whole(&path, &artifact)
        .map_err(|why| format!("{dir}/{name}.kai cannot be written: {why}"))?;
    if let Some((_, old)) = WRITTEN.lock().replace((built.path.clone(), file)) {
        files::close(old);
    }
    Ok(built)
}

/// Writes app `name`'s source as `apps/<name>/main.c`, making the folders if
/// they are new, in an action committed at once (`app_build`).
pub fn app_source(files: &Files, name: &str, text: &str) -> Result<(), String> {
    let apps = folder(APPS_DIR);
    let own = format!("{apps}/{name}");
    let file = files
        .put_whole_in(&[apps, &own], &format!("{own}/main.c"), text.as_bytes())
        .map_err(|why| format!("{APPS_DIR}/{name}/main.c cannot be written: {why}"))?;
    files::close(file);
    Ok(())
}

/// App `name`'s `main.c` as it is now; `None` if it or its folder is not there.
pub fn app_source_read(files: &Files, name: &str) -> Result<Option<String>, String> {
    let path = format!("{}/{name}/main.c", folder(APPS_DIR));
    match files::open(files.home_dir()?, &path, How::Existing, Rights::READ) {
        Ok(file) => {
            let text = read_whole(file, MAX_SOURCE);
            files::close(file);
            let bytes = text.map_err(|why| format!("/home/user/{path} cannot be read: {why}"))?;
            String::from_utf8(bytes)
                .map(Some)
                .map_err(|_| format!("/home/user/{path} is not text"))
        }
        Err(why) if why.ends_with("NotFound") => Ok(None),
        Err(why) => Err(format!("/home/user/{path} cannot be read: {why}")),
    }
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

/// `trial <name>` to `cc` with the `Shm` the last build left its artifact in:
/// the trial's report, or why it failed (`internal: ` first when that is no
/// fault of the program's).
pub fn trial(cc: Handle, name: &str) -> Result<String, String> {
    let internal = shellcore::appbuild::INTERNAL;
    let Some(shm) = *SHM.lock() else {
        return Err(format!("{internal}no build left an artifact to try"));
    };
    // cc maps and reads it, and nothing more.
    let rights = Rights::MAP | Rights::READ | Rights::TRANSFER;
    let copy = sys::handle_dup(shm, rights).map_err(|e| format!("{internal}the Shm ({e:?})"))?;
    let request = promptcore::wire::Request::Trial { component: name }.text();
    let mut reply = vec![0u8; kabi::MAX_MESSAGE];
    let len = match sys::call_h(
        cc,
        request.as_bytes(),
        &mut reply,
        TRIAL_CALL_MS,
        &[copy],
        &mut [],
    ) {
        Ok((len, _)) => len.min(reply.len()),
        Err(e) => {
            let _ = sys::handle_close(copy);
            return Err(format!("{internal}cc did not answer the trial ({e:?})"));
        }
    };
    // Only `err`'s text can be the program's fault; cc says when it is not.
    let records = parse_reply(&reply[..len]).map_err(|e| format!("{internal}cc's answer: {e}"))?;
    match records.last() {
        Some(Record::Text(text)) => Ok(String::from_utf8_lossy(text).into_owned()),
        Some(Record::Err { text, .. }) => Err(String::from_utf8_lossy(text).into_owned()),
        _ => Err(format!("{internal}cc's answer was not a trial's")),
    }
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
