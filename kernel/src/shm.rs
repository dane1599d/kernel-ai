//! Shared pages: the `Shm` kernel object (ADR 0001 D4).
//!
//! - SHM_CREATE makes zeroed pages, counted against the creator's page quota, and
//!   gives it a handle with MAP, READ, WRITE, DUPLICATE, TRANSFER and INSPECT.
//! - SHM_MAP maps object `n` at `SHM_BASE + n * SHM_SLOT` in the caller, read-only
//!   or read-write, never executable; SHM_UNMAP removes it. The pages are freed
//!   once no mapping and no handle is left ([`collect`]).
//! - Each mapping remembers the grant node its handle came through. Revoking that
//!   node ([`on_revoke`]) points every page of the mapping at one private zero
//!   page and shoots the range down on every CPU: the holder keeps running but
//!   sees and shares nothing. Spike S01-1 M6 measures it.
//!
//! Lock order: OBJECTS, then a process's SPACE, then HANDLES, then MEMORY.

use crate::handle::{self, grant::Kernel};
use crate::process::{self, MAX_PROCESSES};
use crate::{memory, smp, threads};
use alloc::vec::Vec;
use captable::Entry;
use captable::grant::{GrantRef, Live};
use kabi::Error;
use kabi::handle::{Handle, Kind, Rights};
use kabi::layout::{SHM_BASE, SHM_SLOT};
use mm::PAGE_SIZE;
use spin::Mutex;

/// Objects that may exist at once.
pub const MAX_OBJECTS: usize = 64;
/// The most pages one object may hold: its slot.
pub const MAX_PAGES: u64 = SHM_SLOT / PAGE_SIZE;

/// What SHM_CREATE's handle carries.
pub fn created_rights() -> Rights {
    Rights::MAP
        | Rights::READ
        | Rights::WRITE
        | Rights::DUPLICATE
        | Rights::TRANSFER
        | Rights::INSPECT
}

struct Mapping {
    pid: u64,
    slot: usize,
    /// The grant node of the handle it was mapped through, if any.
    grant: Option<GrantRef>,
    /// The private zero page every page points at once that node is revoked.
    zero: Option<u64>,
}

struct Object {
    generation: u32,
    frames: Vec<u64>,
    mappings: Vec<Mapping>,
}

struct Objects {
    objects: [Option<Object>; MAX_OBJECTS],
    /// Survives the object, so a handle to a freed one is never taken for a new one.
    generations: [u32; MAX_OBJECTS],
}

static OBJECTS: Mutex<Objects> = Mutex::new(Objects {
    objects: [const { None }; MAX_OBJECTS],
    generations: [0; MAX_OBJECTS],
});

fn with<R>(f: impl FnOnce(&mut Objects) -> R) -> R {
    threads::preempt_disable();
    let result = f(&mut OBJECTS.lock());
    threads::preempt_enable();
    result
}

/// A handle's object field: generation and index.
fn pack(index: usize, generation: u32) -> u64 {
    (u64::from(generation) << 32) | index as u64
}

fn unpack(object: u64) -> (usize, u32) {
    ((object & 0xFFFF_FFFF) as usize, (object >> 32) as u32)
}

/// Where object `index` is mapped.
pub fn base(index: usize) -> u64 {
    SHM_BASE + index as u64 * SHM_SLOT
}

/// A new object of `pages` zeroed pages; its handle object field. The caller
/// charges a quota if it has one.
pub fn create_object(pages: u64) -> Result<u64, Error> {
    if pages == 0 {
        return Err(Error::Invalid);
    }
    if pages > MAX_PAGES {
        return Err(Error::TooBig);
    }
    let mut frames = Vec::new();
    frames
        .try_reserve_exact(pages as usize)
        .map_err(|_| Error::NoMemory)?;
    for _ in 0..pages {
        match memory::with(|m| m.allocate_zeroed_frame()) {
            Some(frame) => frames.push(frame),
            None => {
                free_frames(&frames);
                return Err(Error::NoMemory);
            }
        }
    }
    let placed = with(|objects| {
        let index = objects.objects.iter().position(Option::is_none)?;
        let generation = objects.generations[index].wrapping_add(1).max(1);
        objects.generations[index] = generation;
        objects.objects[index] = Some(Object {
            generation,
            frames: core::mem::take(&mut frames),
            mappings: Vec::new(),
        });
        Some(pack(index, generation))
    });
    placed.ok_or_else(|| {
        free_frames(&frames);
        Error::Busy
    })
}

fn free_frames(frames: &[u64]) {
    memory::with(|m| {
        for &frame in frames {
            let _ = m.free_frame(frame);
        }
    });
}

/// SHM_CREATE for the process in `slot`.
pub fn create(slot: usize, pages: u64) -> Result<u64, Error> {
    let max_pages = process::identity(slot).limits.max_pages;
    // Checked and counted under one lock, so two threads cannot both fit. Counted
    // until the process ends, even if the pages are freed before.
    let charged = process::lock_space(slot)
        .as_mut()
        .map(|space| space.try_charge(pages, max_pages));
    if !charged.ok_or(Error::Dead)? {
        return Err(Error::NoMemory);
    }
    let object = match create_object(pages) {
        Ok(object) => object,
        Err(error) => {
            if let Some(space) = process::lock_space(slot).as_mut() {
                space.refund(pages);
            }
            return Err(error);
        }
    };
    let entry = Entry::new(Kind::Shm, created_rights(), object);
    match handle::with(slot, |h| h.table.insert(entry))?.map_err(Error::from) {
        Ok(handle) => Ok(u64::from(handle.0)),
        Err(error) => {
            collect();
            Err(error)
        }
    }
}

/// SHM_MAP: maps the object `raw` names in `slot`'s space; returns its address.
pub fn map(slot: usize, raw: u64, write: bool) -> Result<u64, Error> {
    let handle = Handle(u32::try_from(raw).map_err(|_| Error::BadHandle)?);
    let mut rights = Rights::MAP | Rights::READ;
    if write {
        rights |= Rights::WRITE;
    }
    let entry = handle::with(slot, |h| h.table.check(handle, Kind::Shm, rights, &Kernel))?
        .map_err(Error::from)?;
    map_entry(slot, &entry, write)
}

/// Maps the object `entry` names, for a holder whose handle was already checked.
pub fn map_entry(slot: usize, entry: &Entry, write: bool) -> Result<u64, Error> {
    let (index, generation) = unpack(entry.object);
    let pid = process::identity(slot).pid;
    let address = with(|objects| {
        let object = objects
            .objects
            .get_mut(index)
            .and_then(Option::as_mut)
            .filter(|o| o.generation == generation)
            .ok_or(Error::BadHandle)?;
        if object.mappings.iter().any(|m| m.pid == pid) {
            return Err(Error::Busy);
        }
        let address = base(index);
        let mut space = process::lock_space(slot);
        let space = space.as_mut().ok_or(Error::Dead)?;
        let mapped = memory::with(|m| {
            for (n, &frame) in object.frames.iter().enumerate() {
                let page = address + n as u64 * PAGE_SIZE;
                if m.map_shared_page(space, page, frame, write).is_err() {
                    for undo in 0..n as u64 {
                        let _ = m.set_shared_page(space, address + undo * PAGE_SIZE, None);
                    }
                    return Err(Error::NoMemory);
                }
            }
            Ok(())
        });
        mapped?;
        object.mappings.push(Mapping {
            pid,
            slot,
            grant: entry.grant,
            zero: None,
        });
        Ok(address)
    })?;
    // A revoke that ran between the check and the mapping finds it now.
    on_revoke();
    Ok(address)
}

/// SHM_UNMAP: removes `slot`'s mapping that starts at `address`.
pub fn unmap(slot: usize, address: u64) -> Result<u64, Error> {
    let offset = address.checked_sub(SHM_BASE).ok_or(Error::Invalid)?;
    if !offset.is_multiple_of(SHM_SLOT) {
        return Err(Error::Invalid);
    }
    let index = usize::try_from(offset / SHM_SLOT).map_err(|_| Error::Invalid)?;
    let pid = process::identity(slot).pid;
    let zero = with(|objects| {
        let object = objects
            .objects
            .get_mut(index)
            .and_then(Option::as_mut)
            .ok_or(Error::NotFound)?;
        let at = object
            .mappings
            .iter()
            .position(|m| m.pid == pid)
            .ok_or(Error::NotFound)?;
        let pages = object.frames.len() as u64;
        let cr3 = {
            let space = process::lock_space(slot);
            let space = space.as_ref().ok_or(Error::Dead)?;
            memory::with(|m| {
                for n in 0..pages {
                    let _ = m.set_shared_page(space, address + n * PAGE_SIZE, None);
                }
            });
            space.cr3()
        };
        // With no lock but OBJECTS held, so a CPU that faults meanwhile can go on.
        smp::tlb_shootdown_range(&[cr3], address, pages);
        Ok::<_, Error>(object.mappings.remove(at).zero)
    })?;
    if let Some(frame) = zero {
        free_frames(&[frame]);
    }
    collect();
    Ok(0)
}

/// After a grant node was revoked: every mapping made through a handle under a
/// dead node gets its private zero page in place of the object's, on every CPU.
pub fn on_revoke() {
    with(|objects| {
        for (index, object) in objects.objects.iter_mut().enumerate() {
            let Some(object) = object else { continue };
            let pages = object.frames.len() as u64;
            // Every holder maps the object at one address: one shootdown for all.
            let mut cr3s: Vec<u64> = Vec::new();
            for mapping in object.mappings.iter_mut() {
                let revoked = mapping.grant.is_some_and(|grant| !Kernel.alive(grant));
                if !revoked || mapping.zero.is_some() {
                    continue;
                }
                let Some(zero) = memory::with(|m| m.allocate_zeroed_frame()) else {
                    continue;
                };
                mapping.zero = Some(zero);
                let address = base(index);
                let space = process::lock_space(mapping.slot);
                if let Some(space) = space.as_ref() {
                    memory::with(|m| {
                        for n in 0..pages {
                            let _ = m.set_shared_page(space, address + n * PAGE_SIZE, Some(zero));
                        }
                    });
                    cr3s.push(space.cr3());
                }
            }
            if !cr3s.is_empty() {
                smp::tlb_shootdown_range(&cr3s, base(index), pages);
            }
        }
    });
}

/// When the process `pid` ends, before its space goes: its mappings are dropped
/// (the space's teardown never frees an object's frames).
pub fn forget(pid: u64) {
    let mut zeros = Vec::new();
    with(|objects| {
        for object in objects.objects.iter_mut().flatten() {
            object.mappings.retain(|m| {
                if m.pid != pid {
                    return true;
                }
                if let Some(zero) = m.zero {
                    zeros.push(zero);
                }
                false
            });
        }
    });
    free_frames(&zeros);
}

/// Frees every object no mapping and no handle refers to.
pub fn collect() {
    let mut freed = Vec::new();
    with(|objects| {
        for (index, place) in objects.objects.iter_mut().enumerate() {
            let unused = place.as_ref().is_some_and(|object| {
                object.mappings.is_empty() && !held(pack(index, object.generation))
            });
            if unused && let Some(object) = place.take() {
                freed.extend(object.frames);
            }
        }
    });
    free_frames(&freed);
}

/// Whether any process holds a live handle to `object`: one whose grant node, if
/// it has one, is not revoked.
fn held(object: u64) -> bool {
    (0..MAX_PROCESSES).any(|slot| {
        handle::with(slot, |h| {
            h.table.iter().any(|(_, entry)| {
                entry.kind == Kind::Shm
                    && entry.object == object
                    && entry.grant.is_none_or(|grant| Kernel.alive(grant))
            })
        })
        .unwrap_or(false)
    })
}

/// The object `object` names: its frames, for a test that looks behind the
/// mappings.
pub fn frames_of(object: u64) -> Option<Vec<u64>> {
    let (index, generation) = unpack(object);
    with(|objects| {
        objects
            .objects
            .get(index)
            .and_then(Option::as_ref)
            .filter(|o| o.generation == generation)
            .map(|o| o.frames.clone())
    })
}
