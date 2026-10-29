//! Packages (lane P): BOOT_READ, the image's signed repository for `pkg`;
//! COMP_ADD, a component version from an installed package (P4 task 1); and
//! PKG_FETCH, a range of a file from the `pkgrepo=` mirror (P4 task 2). ADR
//! 0011 §8, §12.

use super::net::{Destination, exchange_to};
use crate::log;
use crate::process::uaccess::{copy_from_user, copy_to_user, read_struct};
use alloc::vec::Vec;
use kabi::live::{Add, MAX_ARTIFACT, valid_label};
use kabi::{Error, call};
use livecore::store::PackageArtifact;
use supervise::{Folder, MAX_BOOT_PATH};

/// The calls this area serves.
const CALLS: &[u64] = &[
    call::BOOT_READ,
    call::COMP_ADD,
    call::PKG_FETCH,
    call::PKG_TIME,
];

/// One of [`CALLS`], or `None` for a number this area does not have.
pub(super) fn handle(slot: usize, number: u64, a: &[u64; 6]) -> Option<Result<u64, Error>> {
    CALLS.contains(&number).then(|| match number {
        call::BOOT_READ => boot_read(slot, a[0], a[1], a[2], a[3]),
        call::COMP_ADD => comp_add(slot, a[0]),
        call::PKG_FETCH => pkg_fetch(slot, a[0]),
        _ => pkg_time(slot),
    })
}

/// `pkg_time() -> seconds`: the date for TUF's expiry checks, only for the
/// holder of `/system/pkg`, as TRUST_TIME is for `updated`.
fn pkg_time(slot: usize) -> Result<u64, Error> {
    holds_the_store(slot)?;
    crate::clock::unix_seconds().ok_or(Error::NotFound)
}

/// `pkg_fetch(&HttpRequest) -> len`: only for the holder of `/system/pkg`, and
/// only to the fixed `pkgrepo=` mirror, with the one request shape
/// `kabi::pkgrepo::check` takes (in `exchange_to`).
fn pkg_fetch(slot: usize, request_ptr: u64) -> Result<u64, Error> {
    holds_the_store(slot)?;
    exchange_to(slot, request_ptr, Destination::Pkgrepo)
}

/// `boot_read(path_ptr, path_len, buf, cap) -> len`: only beneath the folder the
/// caller's plan line grants with `bootfiles=` (set at spawn from the kernel's own
/// plan, for that line's own program), and only a plain path there
/// (`supervise::BootFiles::allows`), looked up exactly.
fn boot_read(slot: usize, path_ptr: u64, path_len: u64, buf: u64, cap: u64) -> Result<u64, Error> {
    let files = crate::process::identity(slot)
        .boot
        .ok_or(Error::Permission)?;
    let len = usize::try_from(path_len).map_err(|_| Error::TooBig)?;
    if len > MAX_BOOT_PATH {
        return Err(Error::TooBig);
    }
    let mut path = [0u8; MAX_BOOT_PATH];
    copy_from_user(slot, path_ptr, &mut path[..len])?;
    let path = core::str::from_utf8(&path[..len]).map_err(|_| Error::Invalid)?;
    if !files.allows(path) {
        return Err(Error::Permission);
    }
    let data = crate::bootfiles::exact(path).ok_or(Error::NotFound)?;
    if cap >= data.len() as u64 {
        copy_to_user(slot, buf, data)?;
    }
    Ok(data.len() as u64)
}

/// Whether the caller holds `/system/pkg`: spawned with the folder its plan line
/// grants (`dir=pkg`, ADR 0011 §12), and that line is its own program's. No
/// capability bit or name stands for it.
fn holds_the_store(slot: usize) -> Result<(), Error> {
    let me = crate::process::identity(slot);
    if !me.folders {
        return Err(Error::Permission);
    }
    let grants = crate::process::supervision::plan_grants(me.name.as_str(), me.program.as_str())
        .map_err(|_| Error::Permission)?;
    match grants.dirs.contains(&Folder::Pkg) {
        true => Ok(()),
        false => Err(Error::Permission),
    }
}

/// `comp_add(&live::Add) -> 0`: the artifact is copied out and hashed before the
/// component lock is taken; the store only adds (`Component::add_package`).
fn comp_add(slot: usize, add_ptr: u64) -> Result<u64, Error> {
    holds_the_store(slot)?;
    let add: Add = read_struct(slot, add_ptr)?;
    let name_len = usize::try_from(add.name_len).map_err(|_| Error::TooBig)?;
    if name_len > kabi::MAX_NAME {
        return Err(Error::TooBig);
    }
    let mut name = [0u8; kabi::MAX_NAME];
    copy_from_user(slot, add.name_ptr, &mut name[..name_len])?;
    let name = core::str::from_utf8(&name[..name_len]).map_err(|_| Error::Invalid)?;
    if !valid_label(name) {
        return Err(Error::Invalid);
    }
    let len = usize::try_from(add.artifact_len).map_err(|_| Error::TooBig)?;
    if len > MAX_ARTIFACT {
        return Err(Error::TooBig);
    }
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(len).map_err(|_| Error::NoMemory)?;
    bytes.resize(len, 0);
    copy_from_user(slot, add.artifact_ptr, &mut bytes)?;
    let artifact = PackageArtifact::check(bytes, &add.sha)?;
    let label = crate::process::components::add_package(name, artifact)?;
    // A local override shadows it: say so, never hide it.
    match crate::boot_manifest::override_of(name) {
        Some(local) => log!(
            "live: {name} {label} added from an installed package, but the boot manifest's local {local} runs instead (live_good/live_pending)"
        ),
        None => log!("live: {name} {label} added from an installed package"),
    }
    Ok(0)
}
