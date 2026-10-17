//! Live components (M9a, M9b): the versions Limine loaded as modules
//! (`boot/comp/<component>/<label>.wasm`), the uploads the edit loop adds, and one
//! ledger per component, behind the COMP_* calls. The rules live in the host-tested
//! `livecore` crate; here they are applied under a lock, after everything is copied
//! out of the caller's memory.
//!
//! A process may fetch and report only for the component named like itself, and
//! only the `comphost` program holds the capability at all. Only the `edit`
//! service may upload and read any component's versions.

use super::uaccess::{copy_from_user, copy_to_user, read_struct};
use crate::{log, threads};
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use kabi::Error;
use kabi::live::{
    COMMIT_LEN, Event, Info, MAX_CHECKPOINT, MAX_COMPONENTS, MAX_DETAIL, MAX_LABEL, MAX_UPLOAD,
    Put, Read, Restore, read,
};
use livecore::ledger::{self, Ledger};
use livecore::store::{Bytes, Component};
use livecore::uploads::{self, SyncJob};
use spin::Mutex;

static COMPONENTS: Mutex<Vec<Component>> = Mutex::new(Vec::new());

fn with<R>(f: impl FnOnce(&mut Vec<Component>) -> R) -> R {
    threads::preempt_disable();
    let result = f(&mut COMPONENTS.lock());
    threads::preempt_enable();
    result
}

/// Indexes the component versions among the boot modules, given as (path, bytes).
pub fn init<'a>(modules: impl Iterator<Item = (&'a str, &'static [u8])>) {
    let mut components: Vec<Component> = Vec::new();
    for (path, bytes) in modules {
        let Some((name, label)) = path
            .split_once("/comp/")
            .and_then(|(_, rest)| rest.strip_suffix(".wasm"))
            .and_then(|rest| rest.split_once('/'))
        else {
            continue;
        };
        let index = match components.iter().position(|c| c.name == name) {
            Some(index) => index,
            None if components.len() < MAX_COMPONENTS => {
                components.push(Component::new(name));
                components.len() - 1
            }
            None => {
                log!("live: too many components; {name} left out");
                continue;
            }
        };
        if let Err(why) = components[index].add_version(label, Bytes::Static(bytes)) {
            log!("live: {name} {label} left out: {why}");
        }
    }
    for component in &components {
        let labels: Vec<&str> = component.labels().collect();
        log!(
            "live: {} has versions {}",
            component.name,
            labels.join(", ")
        );
    }
    with(|all| *all = components);
}

fn copy_text(slot: usize, ptr: u64, len: u64, max: usize) -> Result<String, Error> {
    let len = usize::try_from(len).map_err(|_| Error::TooBig)?;
    if len > max {
        return Err(Error::TooBig);
    }
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(len).map_err(|_| Error::NoMemory)?;
    bytes.resize(len, 0);
    copy_from_user(slot, ptr, &mut bytes)?;
    String::from_utf8(bytes).map_err(|_| Error::Invalid)
}

/// The caller's name and pid; the component it may act on is named the same.
fn caller(slot: usize) -> (String, u64) {
    let me = super::identity(slot);
    (me.name.as_str().to_string(), me.pid)
}

/// `comp_fetch(label_ptr, label_len, buf, cap) -> len`
pub fn fetch(
    slot: usize,
    label_ptr: u64,
    label_len: u64,
    buf: u64,
    cap: u64,
) -> Result<u64, Error> {
    let label = copy_text(slot, label_ptr, label_len, MAX_LABEL)?;
    let (name, pid) = caller(slot);
    // A shared handle, so an upload is copied out without holding the lock.
    let bytes = with(|all| {
        let component = all.iter().find(|c| c.name == name).ok_or(Error::NotFound)?;
        component.bytes(&label).cloned()
    })?;
    let len = bytes.as_slice().len() as u64;
    if cap >= len {
        copy_to_user(slot, buf, bytes.as_slice())?;
        with(|all| {
            if let Some(component) = all.iter_mut().find(|c| c.name == name) {
                component.record_fetch(pid, &label);
            }
        });
    }
    Ok(len)
}

/// `comp_event(&Event) -> 0`
pub fn event(slot: usize, event_ptr: u64) -> Result<u64, Error> {
    let raw: Event = read_struct(slot, event_ptr)?;
    let blob_len = usize::try_from(raw.blob_len).map_err(|_| Error::TooBig)?;
    if blob_len > MAX_CHECKPOINT {
        return Err(Error::TooBig);
    }
    let mut blob = Vec::new();
    blob.try_reserve_exact(blob_len)
        .map_err(|_| Error::NoMemory)?;
    blob.resize(blob_len, 0);
    copy_from_user(slot, raw.blob_ptr, &mut blob)?;
    let event = ledger::Event {
        kind: raw.kind,
        reason: raw.reason,
        label: copy_text(slot, raw.label_ptr, raw.label_len, MAX_LABEL)?,
        schema: raw.schema,
        served: raw.served,
        regressions: raw.regressions,
        flags: raw.flags,
        blob,
        detail: copy_text(slot, raw.detail_ptr, raw.detail_len, MAX_DETAIL)?,
    };
    let (name, pid) = caller(slot);
    let line = with(|all| {
        let component = all
            .iter_mut()
            .find(|c| c.name == name)
            .ok_or(Error::NotFound)?;
        component.event(pid, &event)
    })?;
    if let Some(line) = line {
        let clean = llmproto::text::printable(&line, 200, 1);
        log!("live: {name} {clean}");
    }
    Ok(0)
}

/// `comp_restore(&mut Restore, buf, cap) -> checkpoint length`
pub fn restore(slot: usize, restore_ptr: u64, buf: u64, cap: u64) -> Result<u64, Error> {
    let (name, _) = caller(slot);
    let (restore, checkpoint) = with(|all| {
        let component = all.iter().find(|c| c.name == name).ok_or(Error::NotFound)?;
        let mut copy = Vec::new();
        copy.try_reserve_exact(component.checkpoint().len())
            .map_err(|_| Error::NoMemory)?;
        copy.extend_from_slice(component.checkpoint());
        Ok::<(Restore, Vec<u8>), Error>((component.restore(), copy))
    })?;
    // SAFETY: Restore is plain old data with no padding.
    let bytes = unsafe {
        core::slice::from_raw_parts(
            (&raw const restore).cast::<u8>(),
            core::mem::size_of::<Restore>(),
        )
    };
    copy_to_user(slot, restore_ptr, bytes)?;
    if cap >= checkpoint.len() as u64 {
        copy_to_user(slot, buf, &checkpoint)?;
    }
    Ok(checkpoint.len() as u64)
}

/// A copy of `name`'s ledger.
pub fn ledger(name: &str) -> Option<Ledger> {
    with(|all| {
        all.iter()
            .find(|c| c.name == name)
            .map(|c| c.ledger.clone())
    })
}

/// How often version `label` of `name` was fetched.
pub fn fetches(name: &str, label: &str) -> u64 {
    with(|all| {
        all.iter()
            .find(|c| c.name == name)
            .map_or(0, |c| c.fetches(label))
    })
}

/// Every component and its versions.
pub fn versions() -> Vec<(String, Vec<String>)> {
    with(|all| {
        all.iter()
            .map(|c| (c.name.clone(), c.labels().map(String::from).collect()))
            .collect()
    })
}

/// `comp_put(&Put) -> 0`: stores an upload the edit service built. The artifact is
/// copied and its manifest read before the lock is taken; the store's rules then
/// decide under it, with the total held by every component's uploads.
pub fn put(slot: usize, put_ptr: u64) -> Result<u64, Error> {
    let raw: Put = read_struct(slot, put_ptr)?;
    let name = copy_text(slot, raw.name_ptr, raw.name_len, kabi::MAX_NAME)?;
    let base = copy_text(slot, raw.base_ptr, raw.base_len, MAX_LABEL)?;
    let commit = copy_text(slot, raw.commit_ptr, raw.commit_len, COMMIT_LEN)?;
    let len = usize::try_from(raw.artifact_len).map_err(|_| Error::TooBig)?;
    if len > MAX_UPLOAD {
        return Err(Error::TooBig);
    }
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(len).map_err(|_| Error::NoMemory)?;
    bytes.resize(len, 0);
    copy_from_user(slot, raw.artifact_ptr, &mut bytes)?;
    store_upload(&name, bytes, &base, &commit)
}

/// Stores `bytes` as the next upload of `name`; also how the boot case checks that
/// a boot artifact cannot pass as an upload.
pub fn store_upload(name: &str, bytes: Vec<u8>, base: &str, commit: &str) -> Result<u64, Error> {
    let meta = uploads::meta(&bytes);
    let bytes = Arc::new(bytes);
    let label = with(|all| {
        let used_elsewhere: usize = all
            .iter()
            .filter(|c| c.name != name)
            .map(Component::upload_bytes)
            .sum();
        let component = all
            .iter_mut()
            .find(|c| c.name == name)
            .ok_or(Error::NotFound)?;
        match &meta {
            Ok(meta) => component.put(meta, bytes, base, commit, used_elsewhere),
            Err(error) => {
                if *error == Error::Invalid {
                    component.uploads.rejected += 1;
                }
                Err(*error)
            }
        }
    })?;
    log!(
        "live: {name} {label} uploaded (built from {base}, commit {})",
        commit.get(..12).unwrap_or("")
    );
    Ok(0)
}

/// `comp_read(&Read) -> len`: a component's Info, or a version's bytes.
pub fn read(slot: usize, read_ptr: u64) -> Result<u64, Error> {
    let raw: Read = read_struct(slot, read_ptr)?;
    let name = copy_text(slot, raw.name_ptr, raw.name_len, kabi::MAX_NAME)?;
    match raw.what {
        read::INFO => {
            let info = with(|all| {
                all.iter()
                    .find(|c| c.name == name)
                    .map(Component::info)
                    .ok_or(Error::NotFound)
            })?;
            if (raw.buf_cap as usize) < core::mem::size_of::<Info>() {
                return Err(Error::TooBig);
            }
            // SAFETY: Info is plain old data with no padding.
            let bytes = unsafe {
                core::slice::from_raw_parts(
                    (&raw const info).cast::<u8>(),
                    core::mem::size_of::<Info>(),
                )
            };
            copy_to_user(slot, raw.buf_ptr, bytes)?;
            Ok(bytes.len() as u64)
        }
        read::ARTIFACT => {
            let label = copy_text(slot, raw.label_ptr, raw.label_len, MAX_LABEL)?;
            let bytes = with(|all| {
                let component = all.iter().find(|c| c.name == name).ok_or(Error::NotFound)?;
                let label = match label.as_str() {
                    "" => component.ledger.active().ok_or(Error::NotFound)?,
                    label => label,
                };
                component.bytes(label).cloned()
            })?;
            let len = bytes.as_slice().len() as u64;
            if raw.buf_cap >= len {
                copy_to_user(slot, raw.buf_ptr, bytes.as_slice())?;
            }
            Ok(len)
        }
        _ => Err(Error::Invalid),
    }
}

/// Starts recording commits and rollbacks for git.
pub fn enable_sync() {
    with(|all| all.iter_mut().for_each(|c| c.uploads.outbox.enable()));
}

/// The oldest job any component has not had acknowledged.
pub fn next_sync() -> Option<(String, SyncJob)> {
    with(|all| {
        all.iter().find_map(|c| {
            c.uploads
                .outbox
                .peek()
                .map(|job| (c.name.clone(), job.clone()))
        })
    })
}

pub fn ack_sync(name: &str, seq: u64, sha: &str) {
    with(|all| {
        if let Some(component) = all.iter_mut().find(|c| c.name == name) {
            component.uploads.outbox.ack(seq, sha);
        }
    });
}

/// A copy of `name`'s record; versions' bytes are shared, not copied.
pub fn component(name: &str) -> Option<Component> {
    with(|all| all.iter().find(|c| c.name == name).cloned())
}
