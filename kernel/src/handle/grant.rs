//! The kernel's grant tree (ADR 0001 D5, D6), over `captable::grant`.
//!
//! The tree sits behind a lock; a copy of each node's generation sits in an
//! atomic, so checking a handle's grant ([`Kernel`]) is one atomic read and
//! takes no lock. Revoking a node bumps the generations of it and its
//! descendants, then wakes every call made through a route given under one of
//! them, which fails at once with `Revoked`.
//!
//! The trusted path makes one `request N` node under `user` per typed request
//! ([`new_request`]) and revokes it when the result line is written; after a yes
//! it mints a `ONCE` handle under it ([`mint_once`]) whose badge names a digest
//! held here, which the action's owner checks ([`check_once`]).

use super::HANDLES;
use crate::threads;
use captable::grant::{self, GrantRef, Life, Live, NODES, Tree};
use captable::once::{Book, Verdict};
use captable::{Entry, Table};
use core::sync::atomic::{AtomicU32, Ordering};
use kabi::handle::{Flags, Handle, Kind, Rights};
use sched::Tid;
use spin::Mutex;

static TREE: Mutex<Tree> = Mutex::new(Tree::new());
/// Each node's generation, as the tree has it, for checks that take no lock.
static GENERATIONS: [AtomicU32; NODES] = [const { AtomicU32::new(1) }; NODES];
static BOOK: Mutex<Book> = Mutex::new(Book::new());
/// Calls in flight through routes given under a grant: the caller's thread and
/// the route's grant, so a revoke can wake the call to fail.
const MAX_IN_FLIGHT: usize = 32;
static IN_FLIGHT: Mutex<[Option<(Tid, GrantRef)>; MAX_IN_FLIGHT]> =
    Mutex::new([None; MAX_IN_FLIGHT]);

/// The kernel's view of which grants are alive.
pub struct Kernel;

impl Live for Kernel {
    fn alive(&self, grant: GrantRef) -> bool {
        GENERATIONS
            .get(usize::from(grant.node))
            .is_some_and(|g| g.load(Ordering::Acquire) == grant.generation)
    }
}

fn tree_error(error: grant::Error) -> kabi::Error {
    match error {
        grant::Error::Revoked => kabi::Error::Revoked,
        grant::Error::Full => kabi::Error::NoMemory,
        grant::Error::Invalid => kabi::Error::Invalid,
    }
}

/// The root `node` (`grant::KERNEL`, `SYSTEM` or `USER`).
pub fn root(node: u16) -> GrantRef {
    TREE.lock().root(node)
}

/// A new node under `parent`, made by `owner` (0 for the kernel).
pub fn create(
    parent: GrantRef,
    owner: u64,
    life: Life,
    label: &[u8],
) -> Result<GrantRef, kabi::Error> {
    TREE.lock()
        .create(parent, owner, life, label, 0)
        .map_err(tree_error)
}

/// The node for typed request `n` under `user`, for the trusted path.
pub fn new_request(n: u64) -> Result<GrantRef, kabi::Error> {
    // "request N", written without the heap: the trusted path makes one per line.
    let mut label = [0u8; grant::LABEL];
    label[..8].copy_from_slice(b"request ");
    let mut digits = [0u8; 20];
    let mut at = digits.len();
    let mut rest = n;
    loop {
        at -= 1;
        digits[at] = b'0' + (rest % 10) as u8;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    let len = (8 + digits.len() - at).min(grant::LABEL);
    label[8..len].copy_from_slice(&digits[at..at + len - 8]);
    create(root(grant::USER), 0, Life::Request, &label[..len])
}

/// Revokes `node` and everything below it; how many nodes went. Calls in
/// flight through routes under any of them are woken and fail.
pub fn revoke(node: GrantRef) -> Result<usize, kabi::Error> {
    let mut freed = [0u16; NODES];
    let count = {
        let mut tree = TREE.lock();
        let count = tree.revoke(node, &mut freed).map_err(tree_error)?;
        for &index in &freed[..count] {
            let index = usize::from(index);
            GENERATIONS[index].store(tree.generation(index), Ordering::Release);
        }
        count
    };
    let mut wake = [None; MAX_IN_FLIGHT];
    for (slot, call) in IN_FLIGHT.lock().iter().enumerate() {
        if let Some((tid, grant)) = call
            && !Kernel.alive(*grant)
        {
            wake[slot] = Some(*tid);
        }
    }
    for tid in wake.into_iter().flatten() {
        x86_64::instructions::interrupts::without_interrupts(|| threads::wake(tid));
    }
    Ok(count)
}

/// Revokes every node the process `pid` made, when it ends: what it gave out
/// under them goes with it.
pub fn revoke_owned(pid: u64) {
    while let Some(node) = TREE.lock().owned_by(pid) {
        if revoke(node).is_err() {
            break;
        }
    }
}

/// Records that the calling thread waits on a call through a route given under
/// `grant`, so a revoke wakes it; `None` if no slot is free (the call still
/// fails at its next wake-up or deadline).
pub fn track(grant: GrantRef) -> Option<usize> {
    let tid = threads::current_tid();
    let mut calls = IN_FLIGHT.lock();
    let free = calls.iter().position(Option::is_none)?;
    calls[free] = Some((tid, grant));
    Some(free)
}

pub fn untrack(slot: Option<usize>) {
    if let Some(slot) = slot {
        IN_FLIGHT.lock()[slot] = None;
    }
}

/// The trusted path, after the user's yes: a `ONCE` handle in `slot`'s table to
/// `object` of `kind` with `rights`, under the request node `request`, whose
/// badge names `digest` (ADR 0001 D6). The kernel never hands this out to ring 3
/// on its own.
pub fn mint_once(
    slot: usize,
    kind: Kind,
    rights: Rights,
    object: u64,
    request: GrantRef,
    digest: [u8; 32],
) -> Result<Handle, kabi::Error> {
    if !Kernel.alive(request) {
        return Err(kabi::Error::Revoked);
    }
    let badge = BOOK
        .lock()
        .hold(request, digest, &Kernel)
        .ok_or(kabi::Error::Busy)?;
    let entry = Entry {
        flags: Flags::ONCE,
        badge,
        grant: Some(request),
        ..Entry::new(kind, rights, object)
    };
    insert(slot, entry)
}

/// Whether the call an owner got, digested as `digest`, is the one shown for the
/// `ONCE` badge `badge`.
pub fn check_once(badge: u64, digest: &[u8; 32]) -> Verdict {
    BOOK.lock().check(badge, digest, &Kernel)
}

/// Puts `entry` in `slot`'s table.
pub fn insert(slot: usize, entry: Entry) -> Result<Handle, kabi::Error> {
    let mut guard = HANDLES[slot].lock();
    let table: &mut Table = &mut guard.as_mut().ok_or(kabi::Error::NotFound)?.table;
    table.insert(entry).map_err(kabi::Error::from)
}

/// Nodes in use, the roots included.
pub fn nodes() -> usize {
    TREE.lock().len()
}
