//! `cc build`'s requests (P4 task 13): the device's `local` repository
//! ([`kaipkg::local`]) as the shell reaches it, through its `uses=pkg` route.
//!
//! - `local-next <component>`: the version the next build takes, `ok <version>`;
//!   read-only, from any caller.
//! - `local <component>`, from the shell only, with the artifact's File handle
//!   moved by CALL_H, and nothing else: published into `local`, signed by the
//!   device key, checked, installed as a new generation and started; `ok <name>
//!   <version> generation <n>`. `pkg` reads that one file through its handle and
//!   no other.
//! - `switch-back <to> <from>`, from the shell only: its undo of such an
//!   install, back to `<from>`'s parent `<to>`, only while `<from>` is still
//!   current, and each app it takes out ends (`withdraw`); `ok generation <n>`.
//!
//! Anything that fails is `err <why>`. Every other holder of a `uses=pkg`
//! route (the desktop's catalogue, say) reads, never installs or switches.

use super::{BootRepo, Calls, KNOWN, NoLedger, now, start_apps, withdraw_apps};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use kabi::Error;
use kabi::handle::{Handle, Kind};
use kaipkg::fsvol::FsVolume;
use kaipkg::local;
use kaipkg::sign::Key;
use kaipkg::store::{self, Ctx, Store};
use user::{println, sys};

/// The action a `local` install or a switch back is, until `pkg` holds
/// `/system/log` (P2 task 12) and takes the shell's log id with the request.
const ACTION: u64 = 3;
/// The one process that may install from `local` or switch back.
const SHELL: &str = "shell";

/// `text`'s answer, from process `caller`, if it is one of these requests,
/// else None.
pub(crate) fn answer(
    vol: &mut FsVolume<Calls>,
    text: &str,
    handles: &[Handle],
    caller: u64,
) -> Option<String> {
    let (verb, rest) = text.split_once(' ')?;
    let answer = match verb {
        "local-next" => next(vol, rest),
        "local" | "switch-back" if !from_shell(caller) => {
            Err(format!("only the shell may ask for {verb}"))
        }
        "local" => install(vol, rest, handles),
        "switch-back" => switch_back(vol, rest),
        _ => return None,
    };
    Some(answer.unwrap_or_else(|why| format!("err {why}")))
}

/// Whether `pid` is the shell, by the name the kernel gave it from its plan
/// line.
fn from_shell(pid: u64) -> bool {
    let Ok(metrics) = sys::metrics() else {
        return false;
    };
    let count = (metrics.process_count as usize).min(metrics.processes.len());
    metrics.processes[..count]
        .iter()
        .any(|p| p.pid == pid && pid != 0 && p.name() == SHELL)
}

/// `local-next <component>`.
fn next(vol: &mut FsVolume<Calls>, component: &str) -> Result<String, String> {
    let key = local::device_key(vol, entropy)?;
    let version = local::next(vol, &key, kaipkg::PROVIDED, component)?;
    Ok(format!("ok {version}"))
}

/// `local <component>`: the artifact through its File handle, at most 1 MiB,
/// named `component` by its own manifest.
fn install(
    vol: &mut FsVolume<Calls>,
    component: &str,
    handles: &[Handle],
) -> Result<String, String> {
    let [file] = handles else {
        return Err("local takes the artifact's File handle and nothing else".into());
    };
    let info = sys::handle_info(*file).map_err(|e| format!("the handle: {e:?}"))?;
    if info.kind() != Some(Kind::File) {
        return Err("local takes a File handle, not another kind".into());
    }
    let artifact = vol.read_handle(*file, kabi::live::MAX_ARTIFACT as u64)?;
    let manifest = livecore::artifact::parse(&artifact)?.manifest;
    if manifest.component != component {
        return Err(format!(
            "the artifact is component {}, not {component}",
            manifest.component
        ));
    }
    let key = local::device_key(vol, entropy)?;
    let taken = taken(vol, &key)?;
    let taken: Vec<&str> = taken.iter().map(String::as_str).collect();
    let time = now(vol);
    let published = local::publish(vol, &key, &artifact, &taken, kaipkg::PROVIDED, time)?;
    let checked = local::check(vol, &key, kaipkg::PROVIDED)?;
    if let Some((path, why)) = checked.refused.first() {
        return Err(format!("local's {path} is refused: {why}"));
    }
    let mut ledger = NoLedger;
    let mut cx = Ctx {
        vol,
        ledger: &mut ledger,
        now: time,
        action: ACTION,
    };
    let n = local::install(
        &Store::default(),
        &mut cx,
        &checked,
        &published,
        kaipkg::PROVIDED,
    )
    .map_err(|e| format!("{e:?}"))?;
    // The switch is made: the reply says so, with its generation for the
    // shell's undo, even if starting the app fails.
    match start_apps(vol, n) {
        Ok(started) => {
            for line in started {
                println!("pkg: started from local: {line}");
            }
        }
        Err(why) => println!("pkg: generation {n} is current, but not started: {why}"),
    }
    Ok(format!(
        "ok {} {} generation {n}",
        published.name, published.version
    ))
}

/// The names a build cannot take: every package the image's repository holds,
/// every one the mirror listed when this device last trusted it, and every one
/// installed whose object `local` did not build. If the image's names cannot
/// be read, nothing is published.
fn taken(vol: &mut FsVolume<Calls>, key: &Key) -> Result<Vec<String>, String> {
    let (image, mirror) = KNOWN.lock().clone();
    let mut taken: Vec<String> = match image.is_empty() {
        false => image.iter().map(|i| i.name.clone()).collect(),
        true => image_names()?,
    };
    match mirror.is_empty() {
        false => taken.extend(mirror.iter().map(|i| i.name.clone())),
        true => taken.extend(kaipkg::mirror::cached_names(vol)?),
    }
    let built = local::shas(vol, key, kaipkg::PROVIDED)?;
    let current = store::current(vol)?;
    let installed = store::generation(vol, current)?.packages;
    taken.extend(
        installed
            .into_iter()
            .filter(|p| !built.contains(&p.sha))
            .map(|p| p.name),
    );
    Ok(taken)
}

/// Every package name the image's repository lists, read again from the boot
/// bundle: those it keeps, and those it refuses too.
fn image_names() -> Result<Vec<String>, String> {
    let checked = kaipkg::image::check(&mut BootRepo, kaipkg::PROVIDED)
        .map_err(|why| format!("the image's names cannot be read: {why}"))?;
    let mut names: Vec<String> = checked.indexes.iter().map(|i| i.name.clone()).collect();
    let refused = checked.refused.iter().filter_map(|(path, _)| {
        let target = path.strip_suffix(".kpkg")?;
        Some(target.rsplit_once('-')?.0.to_string())
    });
    names.extend(refused);
    Ok(names)
}

/// `switch-back <to> <from>`: compare-and-set on `current`, back to its parent
/// only, as the shell's undo of an install is.
fn switch_back(vol: &mut FsVolume<Calls>, rest: &str) -> Result<String, String> {
    let numbers: Vec<u64> = rest
        .split(' ')
        .map(|n| {
            n.parse::<u64>()
                .map_err(|_| "switch-back takes <to> <from>")
        })
        .collect::<Result<_, _>>()?;
    let [to, from] = numbers[..] else {
        return Err("switch-back takes <to> <from>".into());
    };
    let current = store::current(vol)?;
    if current != from {
        return Err(format!(
            "generation {current} is current, not {from}: nothing switched"
        ));
    }
    let parent = store::generation(vol, from)?.parent;
    if to != parent {
        return Err(format!(
            "generation {from}'s parent is {parent}, not {to}: nothing switched"
        ));
    }
    let time = now(vol);
    let mut ledger = NoLedger;
    let mut cx = Ctx {
        vol,
        ledger: &mut ledger,
        now: time,
        action: ACTION,
    };
    let n = Store::default()
        .rollback(&mut cx, to)
        .map_err(|e| format!("{e:?}"))?;
    println!("pkg: switched back from generation {from} to {to}'s packages as generation {n}");
    // The switch is made; an app it took out ends, and one that does not is
    // shown, never hidden.
    match withdraw_apps(vol, from, n) {
        Ok(ended) => {
            for line in ended {
                println!("pkg: ended, undone: {line}");
            }
        }
        Err(why) => println!("pkg: generation {n} is current, but its undone apps run on: {why}"),
    }
    Ok(format!("ok generation {n}"))
}

/// The device key's seed: ENTROPY's, and none without a hardware source, so no
/// key is made from a guessable one.
fn entropy() -> Result<[u8; 32], String> {
    let mut seed = [0u8; 32];
    match sys::entropy(&mut seed) {
        Ok(32) => Ok(seed),
        Ok(n) => Err(format!("ENTROPY gave {n} bytes, not 32")),
        Err(Error::NoEntropy) => Err("no entropy source: the device key is not made".to_string()),
        Err(e) => Err(format!("ENTROPY: {e:?}")),
    }
}
