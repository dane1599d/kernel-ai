//! Clients' pools and processes that are no more: each pool is one of the
//! kernel's 64 `Shm` objects, held here by a mapping and a handle until
//! `desktop` lets both go, so a pool nothing uses and the pools of a client
//! that died or was dropped are released, or no window could open once
//! about 60 had come and gone.

use super::*;

/// Lets a pool go: unmaps it and closes the handle the client moved here.
pub(super) fn release(pool: &Pool) {
    let _ = sys::shm_unmap(pool.address);
    let _ = sys::handle_close(pool.handle);
}

/// Lets every pool of `pid` go.
fn release_all(state: &mut State, pid: u64) {
    for pool in state.pools.iter().filter(|p| p.pid == pid) {
        release(pool);
    }
    state.pools.retain(|p| p.pid != pid);
}

/// Ends a client: its windows close, its buffers are forgotten and its pools
/// go. For a bad message, and for a process that is gone.
pub(super) fn drop_client(state: &mut State, at: usize) {
    let mut client = state.clients.remove(at);
    let pid = client.pid;
    let effects = client.close_all(&mut state.desk);
    deliver(state, &effects);
    state.shown.retain(|(_, owner, ..)| *owner != pid);
    release_all(state, pid);
}

/// A window of `at`'s client closed: if its pool holds no buffer any window
/// shows or the client keeps, the pool goes, and a later buffer on it is refused.
pub(super) fn after_closed(state: &mut State, at: usize, pid: u64, pool: Option<u32>) {
    let Some(pool) = pool else { return };
    let shown = state
        .shown
        .iter()
        .any(|(_, owner, buffer, _)| *owner == pid && buffer.pool == pool);
    if shown || state.clients[at].pool_in_use(pool) {
        return;
    }
    if let Some(held) = state
        .pools
        .iter()
        .position(|p| p.pid == pid && p.pool == pool)
    {
        release(&state.pools.remove(held));
    }
    state.clients[at].pool_gone(pool);
}

/// Drops the clients whose process is not among `alive` (the kernel's list),
/// as a crashed or stopped `comphost` leaves its windows and pools behind.
pub(super) fn reap(state: &mut State, alive: &[u64]) {
    while let Some(at) = state
        .clients
        .iter()
        .position(|c| !alive.contains(&c.pid) || c.too_far_behind())
    {
        println!(
            "desktop: pid {} is gone or never reads its events; its windows close",
            state.clients[at].pid
        );
        drop_client(state, at);
    }
}
