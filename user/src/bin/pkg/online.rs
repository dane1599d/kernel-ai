//! `step=online*` (P5 task 8; ADR 0011 §4 and §8): `pkg` installs from the
//! online repository over TLS, on the `NetDest` its plan line gives it, and
//! nowhere else. The metadata is the update repository's, on its Pages host, and
//! the packages are release assets, reached through GitHub's redirect to its
//! release-assets host; the same checks run as on the `pkgrepo=` mirror
//! (`step=mirror*`): metadata against the root the image carries, a package by
//! its length and SHA-256 and then its header.
//!
//! The plan line says which repository, because only a boot case reaches the
//! mock: `online=test:PORT` is xtask's HTTPS mock (its hosts, `Hosts::mock`),
//! reached at 10.0.2.2 on PORT, trusted through the one root certificate the
//! case adds to the image's repository folder (`tls-test-ca.der`). The handle
//! allows the mock's three hosts on PORT and nothing else. The shipping
//! system's repository (`Hosts::github`, Mozilla's roots, port 443) is asked for
//! by the follow-up that gives `pkg` an install request.
//!
//! - `online`: generation 1 from the image; a name off the repository's hosts is
//!   refused by the handle before any query leaves; then [`FROM_MIRROR`] installs;
//! - `online-again` (boot 2): undo, then the same install again, from the store;
//! - `online-tampered`, `online-rollback`: the repository is tampered with (or a
//!   certificate is wrong), and the install is refused by name.

use super::{BootRepo, Calls, Repository, dated, image, install_from, seed_root};
use alloc::format;
use alloc::string::String;
use kabi::handle::{Handle, HandleInfo, Kind, MAX_STARTUP};
use kainet::http::{Dial, TlsConnect};
use kaipkg::fsvol::FsVolume;
use kaipkg::image::Files;
use kaipkg::online::{Hosts, Online};
use user::net::{NetDial, roots_of, tls_config};
use user::{println, sys};

/// How long one request may take, redirects included: a full handshake on an
/// emulated CPU takes seconds.
const REQUEST_NS: u64 = 120_000_000_000;
/// The boot cases' test CA, a file of the image's repository folder.
const TEST_CA: &str = "tls-test-ca.der";
/// The longest certificate read.
const CA_MAX: u64 = 4096;
/// A name no repository host has, which the handle must refuse to resolve.
const OFF_LIST: &str = "elsewhere.kai.test";

/// The port in `online=test:PORT`.
fn test_port(args: &str) -> Option<u16> {
    args.split_whitespace()
        .find_map(|w| w.strip_prefix("online=test:"))
        .and_then(|port| port.parse().ok())
        .filter(|port| *port > 0)
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

/// The date, for the certificates' validity: the same clock `kaituf` uses.
fn now_s() -> Option<u64> {
    sys::pkg_time().ok()
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
    let port = test_port(args).ok_or("step=online takes online=test:PORT")?;
    image(vol, Some(step))?;
    let dest = netdest().ok_or("pkg has no NetDest handle")?;
    let mut dial = NetDial {
        dest,
        https_port: port,
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
    let ca = BootRepo
        .file(TEST_CA, CA_MAX)?
        .ok_or("the image's repository folder has no test CA")?;
    let roots = roots_of(&ca).map_err(|e| format!("the test CA is refused: {e:?}"))?;
    let config = tls_config(roots, now_s).map_err(|e| format!("no TLS configuration: {e:?}"))?;
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
