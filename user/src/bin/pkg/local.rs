//! `cc build`'s requests (P4 task 13): the device's `local` repository
//! ([`kaipkg::local`]) as the shell reaches it, through its `uses=pkg` route.
//!
//! - `local-next <component>`: the version the next build takes, `ok 0.0.<n>`;
//!   read-only.
//! - `local <component>`, with the artifact's File handle moved by CALL_H, and
//!   nothing else: published into `local`, signed by the device key, checked,
//!   installed as a new generation and started; `ok <name> <version>
//!   generation <n>`. `pkg` reads that one file through its handle and no other.
//! - `switch-back <to> <from>`: the shell's undo, back to generation `<to>`,
//!   only while `<from>` is still current; `ok generation <n>`.
//!
//! Anything that fails is `err <why>`.

use super::{Calls, KNOWN, NoLedger, now, start_apps};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use kabi::Error;
use kabi::handle::Handle;
use kaipkg::fsvol::FsVolume;
use kaipkg::local;
use kaipkg::store::{self, Ctx, Store};
use user::sys;

/// The action a `local` install or a switch back is, until `pkg` holds
/// `/system/log` (P2 task 12) and takes the shell's log id with the request.
const ACTION: u64 = 3;

/// `text`'s answer if it is one of these requests, else None.
pub(crate) fn answer(vol: &mut FsVolume<Calls>, text: &str, handles: &[Handle]) -> Option<String> {
    let (verb, rest) = text.split_once(' ')?;
    let answer = match verb {
        "local-next" => local::next(vol, rest).map(|version| format!("ok {version}")),
        "local" => install(vol, rest, handles),
        "switch-back" => switch_back(vol, rest),
        _ => return None,
    };
    Some(answer.unwrap_or_else(|why| format!("err {why}")))
}

/// `local <component>`: the artifact through its handle, at most 1 MiB, named
/// `component` by its own manifest.
fn install(
    vol: &mut FsVolume<Calls>,
    component: &str,
    handles: &[Handle],
) -> Result<String, String> {
    let [file] = handles else {
        return Err("local takes the artifact's File handle and nothing else".into());
    };
    let artifact = vol.read_handle(*file, kabi::live::MAX_ARTIFACT as u64)?;
    let manifest = livecore::artifact::parse(&artifact)?.manifest;
    if manifest.component != component {
        return Err(format!(
            "the artifact is component {}, not {component}",
            manifest.component
        ));
    }
    let key = local::device_key(vol, entropy)?;
    let taken = taken(vol)?;
    let taken: Vec<&str> = taken.iter().map(String::as_str).collect();
    let time = now(vol);
    let published = local::publish(vol, &key, &artifact, &taken, time)?;
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
    for line in start_apps(vol, n)? {
        user::println!("pkg: started from local: {line}");
    }
    Ok(format!(
        "ok {} {} generation {n}",
        published.name, published.version
    ))
}

/// The names a build cannot take: every package the image's repository and the
/// mirror listed at this boot, and every installed one `local` did not build.
fn taken(vol: &mut FsVolume<Calls>) -> Result<Vec<String>, String> {
    let (image, mirror) = KNOWN.lock().clone();
    let mut taken: Vec<String> = image
        .iter()
        .chain(&mirror)
        .map(|i| i.name.clone())
        .collect();
    let built = local::names(vol)?;
    let current = store::current(vol)?;
    let installed = store::generation(vol, current)?.packages;
    taken.extend(
        installed
            .into_iter()
            .map(|p| p.name)
            .filter(|name| !built.contains(name)),
    );
    Ok(taken)
}

/// `switch-back <to> <from>`: compare-and-set on `current`, as the shell's
/// undo of an install is.
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
