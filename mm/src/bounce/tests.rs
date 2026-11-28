use super::*;
use alloc::vec;

const PAGE: u64 = crate::PAGE_SIZE;

fn pool(runs: &[(u64, u64)]) -> BouncePool {
    let mut pool = BouncePool::new();
    for &(base, slots) in runs {
        pool.add(base, slots).unwrap();
    }
    pool
}

#[test]
fn a_pool_driven_to_its_bound_refuses_one_more_and_reuses_a_returned_slot() {
    let mut pool = pool(&[(0x10_0000, 4), (0x20_0000, 2)]);
    assert_eq!((pool.slots(), pool.free()), (6, 6));
    let taken: Vec<u64> = (0..6).map(|_| pool.take().unwrap()).collect();
    assert_eq!(pool.free(), 0);
    assert_eq!(pool.take(), None, "one more than the slots");
    pool.give(taken[3]).unwrap();
    assert_eq!(pool.take(), Some(taken[3]), "the returned slot is reused");
    assert_eq!(pool.take(), None);
    for phys in taken {
        pool.give(phys).unwrap();
    }
    assert_eq!(pool.free(), 6);
}

#[test]
fn slots_never_overlap() {
    let mut pool = pool(&[
        (0x10_0000, 3),
        (0x10_0000 + 3 * SLOT_BYTES, 5),
        (PAGE * 64, 1),
    ]);
    let mut taken: Vec<u64> = core::iter::from_fn(|| pool.take()).collect();
    assert_eq!(taken.len(), 9);
    taken.sort_unstable();
    for pair in taken.windows(2) {
        assert!(pair[0] + SLOT_BYTES <= pair[1], "{pair:#x?}");
    }
    assert!(taken.iter().all(|phys| phys.is_multiple_of(SLOT_BYTES)));
}

#[test]
fn queues_within_their_slots_never_find_the_pool_dry() {
    // Three queues' worth, each driven in turn to all it may hold at once and
    // drained in a different order, many times over.
    let shapes = [(256, 1, false), (16, 3, true), (8, 1, true)];
    let mut pool = BouncePool::new();
    let mut base = 0x100_0000;
    for &(size, buffers, indirect) in &shapes {
        let slots = queue_slots(size, buffers, indirect);
        pool.add(base, slots).unwrap();
        base += slots * SLOT_BYTES;
    }
    let mut held: Vec<Vec<u64>> = vec![Vec::new(); shapes.len()];
    for round in 0..50u64 {
        for (queue, &(size, buffers, indirect)) in shapes.iter().enumerate() {
            while (held[queue].len() as u64) < queue_slots(size, buffers, indirect) {
                held[queue].push(pool.take().expect("a slot within the queues' bound"));
            }
        }
        assert_eq!(pool.free(), 0);
        let queue = (round % 3) as usize;
        let keep = (round as usize * 7) % (held[queue].len() + 1);
        for phys in held[queue].drain(keep..) {
            pool.give(phys).unwrap();
        }
    }
}

#[test]
fn a_bad_run_or_return_is_refused() {
    let mut pool = pool(&[(0x10_0000, 4)]);
    assert_eq!(pool.add(0x20_0000, 0), Err(BounceError::BadRun), "empty");
    assert_eq!(
        pool.add(0x20_0001, 1),
        Err(BounceError::BadRun),
        "unaligned"
    );
    assert_eq!(
        pool.add(0x10_0000 + SLOT_BYTES, 1),
        Err(BounceError::BadRun),
        "inside a run"
    );
    assert_eq!(
        pool.add(0x10_0000 - SLOT_BYTES, 2),
        Err(BounceError::BadRun),
        "over one"
    );
    assert_eq!(
        pool.add(u64::MAX - SLOT_BYTES + 1, 2),
        Err(BounceError::BadRun)
    );
    assert_eq!(pool.slots(), 4, "nothing was added");
    let phys = pool.take().unwrap();
    assert_eq!(pool.give(phys + 1), Err(BounceError::NotASlot));
    assert_eq!(pool.give(0x30_0000), Err(BounceError::NotASlot));
    assert_eq!(pool.give(phys + SLOT_BYTES), Err(BounceError::NotTaken));
    pool.give(phys).unwrap();
    assert_eq!(pool.give(phys), Err(BounceError::NotTaken), "twice");
    assert_eq!(pool.free(), 4);
}

#[test]
fn a_queue_needs_a_slot_a_descriptor_or_a_request_and_its_list() {
    assert_eq!(queue_slots(256, 1, false), 256);
    // One buffer is never indirect: virtio-drivers adds it direct.
    assert_eq!(queue_slots(8, 1, true), 8);
    assert_eq!(queue_slots(4, 2, false), 4);
    // virtio-blk: header, data, status, and the descriptor list.
    assert_eq!(queue_slots(16, 3, true), 64);
}
