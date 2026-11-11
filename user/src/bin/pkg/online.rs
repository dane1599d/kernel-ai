//! `step=online*` (P5 task 8; ADR 0011 §4 and §8): `pkg` installs from the
//! online repository over TLS, on the `NetDest` its plan line gives it, and
//! nowhere else. The metadata is the update repository's, on its Pages host, and
//! the packages are release assets, reached through GitHub's redirect to its
//! release-assets host; the same checks run as on the `pkgrepo=` mirror
//! (`step=mirror*`): metadata against the root the image carries, a package by
//! its length and SHA-256 and then its header.
//!
//! The TLS client is `user::tls`'s, shared with the other programs that reach the
//! network: the transport on the handle, Mozilla's roots and the boot bundle's
//! `netroots/`, ENTROPY and NET_TIME.
//!
//! The plan line says which repository, because only a boot case reaches the
//! mock: `online=test` is xtask's HTTPS mock (`Hosts::mock`), whose test CA is
//! the bundle's one extra root. The handle allows its three hosts on port 443 and
//! nothing else. The shipping system's repository (`Hosts::github`) is asked for
//! by the follow-up that gives `pkg` an install request.
//!
//! - `online`: generation 1 from the image; a name off the repository's hosts is
//!   refused by the handle before any query leaves; then [`FROM_MIRROR`] installs;
//! - `online-again` (boot 2): undo, then the same install again, from the store;
//! - `online-tampered`, `online-rollback`: the repository is tampered with, and
//!   the install is refused by name.

use super::{Calls, Repository, dated, image, install_from, seed_root};
use alloc::format;
use alloc::string::String;
use kabi::handle::{Handle, HandleInfo, Kind, MAX_STARTUP};
use kainet::http::{Dial, TlsConnect};
use kaipkg::fsvol::FsVolume;
use kaipkg::online::{Hosts, Online};
use user::tls::{self, NetDial};
use user::{println, sys};

/// How long one request may take, redirects included: a full handshake on an
/// emulated CPU takes seconds.
const REQUEST_NS: u64 = 120_000_000_000;
/// A name no repository host has, which the handle must refuse to resolve.
const OFF_LIST: &str = "elsewhere.kai.test";

/// Whether the plan line asks for xtask's HTTPS mock (`online=test`).
fn test_repository(args: &str) -> bool {
    args.split_whitespace().any(|w| w == "online=test")
}

/// The `NetDest` handle among the startup handles.
fn netdest() -> Option<Handle> {
    let mut list = [HandleInfo::default(); MAX_STARTUP];
    let count = sys::startup(&mut list).ok()?.min(MAX_STARTUP);
    list[..count]
        .iter()
        .find(|info| info.kind() == Some(Kind::NetDest))
        .map(|info| Handle(info.handle))
}

/// A host off the repository's is refused by the handle, so no query for it
/// leaves: the destination `pkg` was given is the one the plan line named.
fn off_list_refused(dial: &mut NetDial) -> Result<(), String> {
    match dial.dial(OFF_LIST, 443) {
        Err(_) => {
            println!("pkg: the destination refused {OFF_LIST}, which is not the repository's");
            Ok(())
        }
        Ok(_) => Err(format!("the destination let {OFF_LIST} through")),
    }
}

pub(crate) fn run(vol: &mut FsVolume<Calls>, step: &str, args: &str) -> Result<String, String> {
    if !test_repository(args) {
        return Err(String::from("step=online takes online=test"));
    }
    image(vol, Some(step))?;
    let dest = netdest().ok_or("pkg has no NetDest handle")?;
    let mut dial = NetDial {
        dest,
        bulk: false,
        deadline_ns: u64::MAX,
    };
    if step == "online" {
        off_list_refused(&mut dial)?;
    }
    let what = "the online repository";
    seed_root(vol, what)?;
    let time = match dated(vol) {
        Ok(time) => time,
        Err(why) => return Ok(format!("{what} is refused: {why}")),
    };
    let config = tls::config().map_err(|e| format!("no TLS configuration: {e:?}"))?;
    let connect = TlsConnect { config, dial };
    let clock = || sys::time_ns();
    let source = Online::new(Hosts::mock(), connect, &clock, REQUEST_NS);
    install_from(
        vol,
        step,
        Repository {
            what,
            time,
            source,
            requests: |source| source.requests,
        },
    )
}
