//! Shared pages: the `Shm` kernel object (ADR 0001 D4).
//!
//! - SHM_CREATE makes zeroed pages, counted against the creator's page quota, and
//!   gives it a handle with MAP, READ, WRITE, DUPLICATE, TRANSFER and INSPECT.
//! - SHM_MAP maps object `n` at `SHM_BASE + n * SHM_SLOT` in the caller, read-only
//!   or read-write, never executable; SHM_UNMAP removes it. The pages are freed
//!   once no mapping and no handle is left ([`collect`]): no handle in a table,
//!   none moving between tables in a call or reply ([`transit`]), and the
//!   object not pinned: one is from its making until its first handle is in a
//!   table ([`settle`]).
//! - The creator's quota charge goes back when the object is freed, if the
//!   creator still runs (audit K #2). A process makes at most
//!   [`MAX_PER_PROCESS`] objects at once, and the last free slot is the
//!   kernel's (the screen's back buffer), so no one process takes them all.
//! - Each mapping remembers the grant node its handle came through, and gets a
//!   private zero page when it is made. Revoking that node ([`on_revoke`]) points
//!   every page of the mapping at that zero page and shoots the range down on
//!   every CPU: the holder keeps running but sees and shares nothing. Spike S01-1
//!   M6 measures it; nothing is allocated on that path.
//!
//! Lock order: OBJECTS, then a process's SPACE, then HANDLES, then MEMORY.

use crate::handle::{self, grant::Kernel};
use crate::process::{self, MAX_PROCESSES};
use crate::{hpet, memory, smp, threads};
use alloc::vec::Vec;
use captable::Entry;
use captable::grant::{GrantRef, Live};
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use kabi::Error;
use kabi::handle::{Handle, Kind, Rights};
use kabi::layout::{SHM_BASE, SHM_SLOT};
use mm::PAGE_SIZE;
use spin::Mutex;

/// Objects that may exist at once.
pub const MAX_OBJECTS: usize = 64;
/// The most pages one object may hold: its slot.
pub const MAX_PAGES: u64 = SHM_SLOT / PAGE_SIZE;
/// Objects one process may have made and not yet freed (audit K #14).
pub const MAX_PER_PROCESS: usize = 16;
/// Free slots only [`create_kernel_object`] may take.
const KERNEL_RESERVED: usize = 1;

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
    zero: u64,
    revoked: bool,
}

struct Object {
    generation: u32,
    frames: Vec<u64>,
    /// The kernel's own frames (the screen's back buffer): never freed here.
    kernel: bool,
    mappings: Vec<Mapping>,
    /// Made and its first handle not yet in a table: never collected.
    pinned: bool,
    /// Who SHM_CREATE charged for it, to refund when it is freed.
    charge: Option<Charge>,
}

#[derive(Clone, Copy)]
struct Charge {
    pid: u64,
    slot: usize,
    pages: u64,
}

/// Handles to each object moving between tables ([`transit`]).
static TRANSIT: [AtomicU32; MAX_OBJECTS] = [const { AtomicU32::new(0) }; MAX_OBJECTS];

/// A handle to `object` left a table to move to another (`up`), or reached
/// it or was dropped on the way: while any is moving, the object stays.
pub fn transit(object: u64, up: bool) {
    let (index, _) = unpack(object);
    let Some(count) = TRANSIT.get(index) else {
        return;
    };
    if up {
        count.fetch_add(1, Ordering::SeqCst);
    } else {
        let _ = count.try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1));
    }
}

struct Objects {
    objects: [Option<Object>; MAX_OBJECTS],
    /// Survives the object, so a handle to a freed one is never taken for a new one.
    generations: [u32; MAX_OBJECTS],
}

/// Nanoseconds [`on_revoke`] has spent in TLB shootdowns, for M6's split.
pub static SHOOTDOWN_NS: AtomicU64 = AtomicU64::new(0);
/// Nanoseconds spent in [`on_revoke`] in all, shootdowns included.
pub static ON_REVOKE_NS: AtomicU64 = AtomicU64::new(0);

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
    create_charged(pages, None, false)
}

/// [`create_object`], refunding `charge` when it is freed. Busy when the
/// creator has [`MAX_PER_PROCESS`] already, or only the kernel's slots are free.
/// With `pinned`, it is not collected until [`settle`].
fn create_charged(pages: u64, charge: Option<Charge>, pinned: bool) -> Result<u64, Error> {
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
        let free = objects.objects.iter().filter(|o| o.is_none()).count();
        let mine = |o: &Object| charge.is_some_and(|c| o.charge.is_some_and(|d| d.pid == c.pid));
        let made = objects.objects.iter().flatten().filter(|o| mine(o)).count();
        if free <= KERNEL_RESERVED || made >= MAX_PER_PROCESS {
            return None;
        }
        let index = objects.objects.iter().position(Option::is_none)?;
        let generation = objects.generations[index].wrapping_add(1).max(1);
        objects.generations[index] = generation;
        objects.objects[index] = Some(Object {
            generation,
            frames: core::mem::take(&mut frames),
            kernel: false,
            mappings: Vec::new(),
            pinned,
            charge,
        });
        Some(pack(index, generation))
    });
    placed.ok_or_else(|| {
        free_frames(&frames);
        Error::Busy
    })
}

/// An object over `pages` frames from `phys` that the kernel owns, such as the
/// screen's back buffer (ADR 0005 §2): not zeroed, charged to no one, and its
/// frames never freed when the object goes. Pinned until [`settle`].
pub fn create_kernel_object(phys: u64, pages: u64) -> Result<u64, Error> {
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
    frames.extend((0..pages).map(|n| phys + n * PAGE_SIZE));
    with(|objects| {
        let index = objects.objects.iter().position(Option::is_none)?;
        let generation = objects.generations[index].wrapping_add(1).max(1);
        objects.generations[index] = generation;
        objects.objects[index] = Some(Object {
            generation,
            frames,
            kernel: true,
            mappings: Vec::new(),
            pinned: true,
            charge: None,
        });
        Some(pack(index, generation))
    })
    .ok_or(Error::Busy)
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
    let me = process::identity(slot);
    let max_pages = me.limits.max_pages;
    // Checked and counted under one lock, so two threads cannot both fit.
    // Counted until the object is freed ([`collect`] refunds it).
    let charged = process::lock_space(slot)
        .as_mut()
        .map(|space| space.try_charge(pages, max_pages));
    if !charged.ok_or(Error::Dead)? {
        return Err(Error::NoMemory);
    }
    let charge = Charge {
        pid: me.pid,
        slot,
        pages,
    };
    let object = match create_charged(pages, Some(charge), true) {
        Ok(object) => object,
        Err(error) => {
            if let Some(space) = process::lock_space(slot).as_mut() {
                space.refund(pages);
            }
            return Err(error);
        }
    };
    let entry = Entry::new(Kind::Shm, created_rights(), object);
    let inserted = handle::with(slot, |h| h.table.insert(entry));
    settle(object);
    match inserted?.map_err(Error::from) {
        Ok(handle) => Ok(u64::from(handle.0)),
        Err(error) => {
            collect();
            Err(error)
        }
    }
}

/// SHM_MAP: maps the object `raw` names in `slot`'s space; returns its address.
/// Invalid when the object holds fewer than `min_pages` pages, so a server
/// that will touch that many never faults on a short one (0: no check).
pub fn map(slot: usize, raw: u64, write: bool, min_pages: u64) -> Result<u64, Error> {
    let handle = Handle(u32::try_from(raw).map_err(|_| Error::BadHandle)?);
    let mut rights = Rights::MAP | Rights::READ;
    if write {
        rights |= Rights::WRITE;
    }
    let entry = handle::with(slot, |h| h.table.check(handle, Kind::Shm, rights, &Kernel))?
        .map_err(Error::from)?;
    if min_pages > 0 && pages_of(&entry)? < min_pages {
        return Err(Error::Invalid);
    }
    map_entry(slot, &entry, write)
}

/// How many pages the object `entry` names holds.
fn pages_of(entry: &Entry) -> Result<u64, Error> {
    let (index, generation) = unpack(entry.object);
    with(|objects| {
        objects
            .objects
            .get(index)
            .and_then(Option::as_ref)
            .filter(|o| o.generation == generation)
            .map(|o| o.frames.len() as u64)
            .ok_or(Error::BadHandle)
    })
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
        let zero = memory::with(|m| {
            let zero = m.allocate_zeroed_frame().ok_or(Error::NoMemory)?;
            for (n, &frame) in object.frames.iter().enumerate() {
                let page = address + n as u64 * PAGE_SIZE;
                if m.map_shared_page(space, page, frame, write).is_err() {
                    let _ = m.set_shared_range(space, address, n as u64, None);
                    let _ = m.free_frame(zero);
                    return Err(Error::NoMemory);
                }
            }
            Ok(zero)
        })?;
        object.mappings.push(Mapping {
            pid,
            slot,
            grant: entry.grant,
            zero,
            revoked: false,
        });
        Ok(address)
    })?;
    // A revoke that ran between the check and the mapping finds it now.
    on_revoke();
    Ok(address)
}

/// SHM_INFO: how many pages the object `raw` names in `slot`'s table holds,
/// for a holder that must know its size before it trusts a mapping (cc's
/// request `Shm`), whatever the handle's rights. Read with `handle::entry`,
/// not `check`: asking a `ONCE` handle's size must not use it up.
pub fn info(slot: usize, raw: u64) -> Result<u64, Error> {
    let handle = Handle(u32::try_from(raw).map_err(|_| Error::BadHandle)?);
    let entry = handle::entry(slot, handle)?;
    if entry.kind != Kind::Shm {
        return Err(Error::BadHandle);
    }
    let (index, generation) = unpack(entry.object);
    with(|objects| {
        objects
            .objects
            .get(index)
            .and_then(Option::as_ref)
            .filter(|o| o.generation == generation)
            .map(|o| o.frames.len() as u64)
            .ok_or(Error::BadHandle)
    })
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
                let _ = m.set_shared_range(space, address, pages, None);
            });
            space.cr3()
        };
        // With no lock but OBJECTS held, so a CPU that faults meanwhile can go on.
        smp::tlb_shootdown_range(&[cr3], address, pages);
        Ok::<_, Error>(object.mappings.remove(at).zero)
    })?;
    free_frames(&[zero]);
    collect();
    Ok(0)
}

/// After a grant node was revoked: every mapping made through a handle under a
/// dead node gets its private zero page in place of the object's, on every CPU.
pub fn on_revoke() {
    let begun = hpet::nanos();
    with(|objects| {
        for (index, object) in objects.objects.iter_mut().enumerate() {
            let Some(object) = object else { continue };
            let pages = object.frames.len() as u64;
            // Every holder maps the object at one address: one shootdown for all.
            let mut cr3s: Vec<u64> = Vec::new();
            for mapping in object.mappings.iter_mut() {
                if mapping.revoked || mapping.grant.is_none_or(|grant| Kernel.alive(grant)) {
                    continue;
                }
                mapping.revoked = true;
                let space = process::lock_space(mapping.slot);
                if let Some(space) = space.as_ref() {
                    memory::with(|m| {
                        let _ = m.set_shared_range(space, base(index), pages, Some(mapping.zero));
                    });
                    cr3s.push(space.cr3());
                }
            }
            if !cr3s.is_empty() {
                let start = hpet::nanos();
                smp::tlb_shootdown_range(&cr3s, base(index), pages);
                SHOOTDOWN_NS.fetch_add(hpet::nanos() - start, Ordering::Relaxed);
            }
        }
    });
    ON_REVOKE_NS.fetch_add(hpet::nanos() - begun, Ordering::Relaxed);
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
                zeros.push(m.zero);
                false
            });
        }
    });
    free_frames(&zeros);
}

/// Frees every object no mapping and no handle refers to, and gives each
/// one's quota charge back to its creator if it still runs.
pub fn collect() {
    let mut freed = Vec::new();
    let mut refunds = Vec::new();
    with(|objects| {
        for (index, place) in objects.objects.iter_mut().enumerate() {
            // The transit count, then the tables, then whether any handle
            // moved meanwhile, as the NetDest sweep does (`handle::moves`).
            let before = handle::moves();
            let unused = place.as_ref().is_some_and(|object| {
                object.mappings.is_empty()
                    && !object.pinned
                    && TRANSIT[index].load(Ordering::SeqCst) == 0
                    && !held(pack(index, object.generation))
                    && handle::moves() == before
            });
            if !unused {
                continue;
            }
            let Some(object) = place.take() else {
                continue;
            };
            if let Some(charge) = object.charge
                && refunds.try_reserve(1).is_ok()
            {
                refunds.push(charge);
            }
            if !object.kernel {
                freed.extend(object.frames);
            }
        }
    });
    free_frames(&freed);
    for charge in refunds {
        refund(charge);
    }
}

/// Unpins `object` once its first handle is in a table, or could not be put
/// there: from now on [`collect`] frees it when nothing names it.
pub fn settle(object: u64) {
    let (index, generation) = unpack(object);
    with(|objects| {
        if let Some(Some(o)) = objects.objects.get_mut(index)
            && o.generation == generation
        {
            o.pinned = false;
        }
    });
}

/// Gives `charge` back to its creator, if the process in its slot is still it.
fn refund(charge: Charge) {
    // The pid is checked with the space's lock held: a space is installed
    // and taken away under that lock, so the slot cannot pass to another
    // process between the check and the refund (#1155's review).
    let mut space = process::lock_space(charge.slot);
    if process::identity(charge.slot).pid != charge.pid {
        return;
    }
    if let Some(space) = space.as_mut() {
        space.refund(charge.pages);
    }
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
