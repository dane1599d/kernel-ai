//! The KV store (agents OS plan F4) on a generated model: a preempted request's
//! state, parked in the pool, spilled, demoted from the pool to a spill, or
//! dropped, answers as a fresh slot does when it comes back; the pool and the
//! region hold to their room and give it back.

extern crate std;

use crate::chat::{Engine, Role, Turn};
use crate::fixture::{self, Spec};
use crate::kvstore::{
    Applied, KvStore, MAX_POOL_BYTES, Move, Place, Region, Unkept, pool_budget, request_key,
};
use crate::segments::{self, Key, Snapshot};
use crate::segments_tests::{ROOMY, key, later, reply, saved};
use alloc::vec;
use alloc::vec::Vec;
use quant::Tier;

/// A region in memory, which can be made to fail its writes.
struct Mem {
    bytes: Vec<u8>,
    fail_writes: bool,
}

impl Mem {
    fn new(len: usize) -> Self {
        Self {
            bytes: vec![0; len],
            fail_writes: false,
        }
    }
}

impl Region for Mem {
    fn size(&self) -> u64 {
        self.bytes.len() as u64
    }

    fn read_at(&mut self, at: u64, buf: &mut [u8]) -> bool {
        let at = at as usize;
        match self.bytes.get(at..at + buf.len()) {
            Some(part) => {
                buf.copy_from_slice(part);
                true
            }
            None => false,
        }
    }

    fn write_at(&mut self, at: u64, bytes: &[u8]) -> bool {
        let at = at as usize;
        match self.bytes.get_mut(at..at + bytes.len()) {
            Some(part) if !self.fail_writes => {
                part.copy_from_slice(bytes);
                true
            }
            _ => false,
        }
    }

    fn flush(&mut self) -> bool {
        true
    }
}

const REQUEST: u64 = 3;
const AGENT: u32 = 7;
const ROOM: usize = 4 << 20;

fn engine(bytes: &[u8]) -> Engine<'_> {
    Engine::load(bytes, Tier::best(), 256, ROOMY).unwrap()
}

/// The state of [`saved`], computed in `engine`'s slot.
fn computed(engine: &mut Engine<'_>) {
    reply(engine, &saved(), 1);
}

/// The next reply after a restore of `n` positions, against a fresh engine's.
/// With none restored, the reply reuses only what the slot's last prompt
/// shares with it, as any prompt does.
fn answers_as_fresh(bytes: &[u8], restored: &mut Engine<'_>, n: usize) {
    let got = reply(restored, &later(), 12);
    let want = reply(&mut engine(bytes), &later(), 12);
    if n > 0 {
        assert_eq!(got.cached, n);
    }
    assert_eq!((got.bytes, got.tokens), (want.bytes, want.tokens));
}

/// The agent's state goes `route`, another prompt takes the slot, and the
/// state comes back: the positions the restore gives.
fn round_trip(
    bytes: &[u8],
    store: &mut KvStore<Mem>,
    route: &[Place],
    k: &Key,
) -> (Engine<'static>, usize) {
    // The engine outlives this call only through `bytes`, which the tests leak.
    let bytes: &'static [u8] = Vec::leak(bytes.to_vec());
    let mut e = engine(bytes);
    computed(&mut e);
    // As agentsched's moves come: out of the slot, on through the route, back in.
    let mut from = Place::Slot;
    for &to in route {
        let mv = Move {
            request: REQUEST,
            from,
            to,
        };
        assert_eq!(
            store.apply(&mut e, k, mv, AGENT, &saved()),
            Ok(Applied::Kept)
        );
        from = to;
    }
    reply(&mut e, &[Turn::new(Role::User, "something else")], 4);
    let back = Move {
        request: REQUEST,
        from,
        to: Place::Slot,
    };
    let Ok(Applied::Restored(n)) = store.apply(&mut e, k, back, AGENT, &later()) else {
        panic!("{route:?} did not come back");
    };
    (e, n)
}

#[test]
fn a_state_restored_from_every_place_answers_as_a_fresh_slot() {
    let bytes = fixture::build(&Spec::tiny());
    let k = key(Tier::best());
    let routes: [&[Place]; 4] = [
        &[Place::Ram],
        &[Place::Spill],
        &[Place::Ram, Place::Spill],
        &[Place::Cold],
    ];
    for route in routes {
        let mut store = KvStore::new(MAX_POOL_BYTES, Some(Mem::new(ROOM)));
        let (mut e, n) = round_trip(&bytes, &mut store, route, &k);
        assert_eq!(n > 0, route != [Place::Cold], "{route:?}");
        answers_as_fresh(&bytes, &mut e, n);
        // Restored, it is kept nowhere: its pool bytes and its room are free.
        assert_eq!(store.place(REQUEST), None, "{route:?}");
        assert_eq!(store.ram_bytes(), 0);
    }
}

#[test]
fn a_demoted_snapshot_is_the_file_the_slot_saves() {
    let bytes = fixture::build(&Spec::tiny());
    let k = key(Tier::best());
    let mut e = engine(&bytes);
    computed(&mut e);
    let mut saved_file = Vec::new();
    e.save_slot(&k, &mut |b| {
        saved_file.extend_from_slice(b);
        true
    })
    .unwrap();
    let mut snapshot = Snapshot::new();
    e.snapshot_slot(&k, &mut snapshot).unwrap();
    let mut from_snapshot = Vec::new();
    segments::save_snapshot(&snapshot, &mut |b| {
        from_snapshot.extend_from_slice(b);
        true
    })
    .unwrap();
    assert_eq!(from_snapshot, saved_file);
    assert_eq!(segments::snapshot_file_bytes(&snapshot), saved_file.len());
    assert_eq!(e.slot_file_bytes(), saved_file.len());
    assert!(segments::save_snapshot(&Snapshot::new(), &mut |_| true).is_err());
}

#[test]
fn the_pool_and_the_region_keep_to_their_room() {
    let bytes = fixture::build(&Spec::tiny());
    let k = key(Tier::best());
    let mut e = engine(&bytes);
    computed(&mut e);
    // A pool too small for the snapshot keeps nothing.
    let mut store = KvStore::new(16, Some(Mem::new(ROOM)));
    assert_eq!(
        store.park(&e, &k, REQUEST, Place::Ram),
        Err(Unkept::PoolFull)
    );
    assert_eq!(store.place(REQUEST), None);
    // No region, or one too small: no spill.
    let mut none: KvStore<Mem> = KvStore::new(MAX_POOL_BYTES, None);
    assert_eq!(
        none.park(&e, &k, REQUEST, Place::Spill),
        Err(Unkept::NoRoom)
    );
    let file = e.slot_file_bytes();
    let mut small = KvStore::new(MAX_POOL_BYTES, Some(Mem::new(file - 1)));
    assert_eq!(
        small.park(&e, &k, REQUEST, Place::Spill),
        Err(Unkept::NoRoom)
    );
    // A region with room for one file holds two in turn: a restored one's room
    // is given back.
    let mut one = KvStore::new(MAX_POOL_BYTES, Some(Mem::new(file)));
    one.park(&e, &k, 1, Place::Spill).unwrap();
    assert_eq!(one.park(&e, &k, 2, Place::Spill), Err(Unkept::NoRoom));
    one.restore(&mut e, &k, 1, &later()).unwrap();
    computed(&mut e);
    one.park(&e, &k, 2, Place::Spill).unwrap();
    assert_eq!(one.place(2), Some(Place::Spill));
}

#[test]
fn a_spill_that_fails_keeps_nothing_and_the_run_prefills() {
    let bytes = fixture::build(&Spec::tiny());
    let k = key(Tier::best());
    let mut e = engine(&bytes);
    computed(&mut e);
    let mut failing = Mem::new(ROOM);
    failing.fail_writes = true;
    let mut store = KvStore::new(MAX_POOL_BYTES, Some(failing));
    assert_eq!(
        store.park(&e, &k, REQUEST, Place::Spill),
        Err(Unkept::Failed)
    );
    assert_eq!(store.place(REQUEST), None);
    assert_eq!(store.restore(&mut e, &k, REQUEST, &later()), Ok(0));
    // A demotion that fails leaves the state cold, and the pool empty.
    let mut store = KvStore::new(MAX_POOL_BYTES, Some(Mem::new(16)));
    store.park(&e, &k, REQUEST, Place::Ram).unwrap();
    assert!(store.ram_bytes() > 0);
    assert_eq!(store.demote(REQUEST, Place::Spill), Err(Unkept::NoRoom));
    assert_eq!((store.place(REQUEST), store.ram_bytes()), (None, 0));
}

#[test]
fn another_requests_state_is_refused_and_the_slot_left_empty() {
    let bytes = fixture::build(&Spec::tiny());
    let k = key(Tier::best());
    let other = Key {
        pack_id: segments::spill_pack_id(9, 9),
        ..k
    };
    for to in [Place::Ram, Place::Spill] {
        let mut e = engine(&bytes);
        computed(&mut e);
        let mut store = KvStore::new(MAX_POOL_BYTES, Some(Mem::new(ROOM)));
        store.park(&e, &k, REQUEST, to).unwrap();
        assert!(
            store.restore(&mut e, &other, REQUEST, &later()).is_err(),
            "{to:?}"
        );
        answers_as_fresh(&bytes, &mut e, 0);
    }
}

#[test]
fn the_pool_takes_what_is_free_up_to_512_mib() {
    assert_eq!(pool_budget(6 << 30), 512 << 20);
    assert_eq!(pool_budget(100 << 20), 100 << 20);
    assert_eq!(pool_budget(0), 0);
}

/// A state kept for one agent's request does not come back for another
/// agent's, as the rig keys them: the slot is left empty.
#[test]
fn a_move_back_for_another_agent_is_refused() {
    let bytes = fixture::build(&Spec::tiny());
    let model = key(Tier::best());
    assert_ne!(
        request_key(&model, AGENT, REQUEST),
        request_key(&model, AGENT + 1, REQUEST)
    );
    for to in [Place::Ram, Place::Spill] {
        let mut e = engine(&bytes);
        computed(&mut e);
        let mut store = KvStore::new(MAX_POOL_BYTES, Some(Mem::new(ROOM)));
        let out = Move {
            request: REQUEST,
            from: Place::Slot,
            to,
        };
        store.apply(&mut e, &model, out, AGENT, &saved()).unwrap();
        let back = Move {
            request: REQUEST,
            from: to,
            to: Place::Slot,
        };
        let got = store.apply(&mut e, &model, back, AGENT + 1, &later());
        assert!(matches!(got, Err(Unkept::Refused(_))), "{to:?}: {got:?}");
        answers_as_fresh(&bytes, &mut e, 0);
    }
    let mut store: KvStore<Mem> = KvStore::new(MAX_POOL_BYTES, None);
    let nowhere = Move {
        request: REQUEST,
        from: Place::Cold,
        to: Place::Ram,
    };
    let mut e = engine(&bytes);
    assert_eq!(
        store.apply(&mut e, &model, nowhere, AGENT, &saved()),
        Err(Unkept::Elsewhere)
    );
}
