extern crate std;

use super::*;
use std::collections::HashMap;
use std::vec;

const ALL: Rights = Rights::all();

fn sched(rights: Rights) -> Entry {
    Entry::new(Kind::Sched, rights, 1)
}

#[test]
fn a_new_table_has_room_for_256_and_handles_are_never_zero() {
    let mut table = Table::new().unwrap();
    assert_eq!((table.capacity(), table.len()), (TABLE_START, 0));
    let handle = table.insert(sched(Rights::READ)).unwrap();
    assert_ne!(handle, Handle::NONE);
    assert_eq!(table.get(handle).unwrap().kind, Kind::Sched);
    assert_eq!(table.get(Handle::NONE), Err(Error::BadHandle));
}

#[test]
fn check_wants_the_kind_and_every_right() {
    let mut table = Table::new().unwrap();
    let handle = table
        .insert(sched(Rights::READ | Rights::SET_POLICY))
        .unwrap();
    assert!(table.check(handle, Kind::Sched, Rights::SET_POLICY).is_ok());
    assert_eq!(
        table.check(handle, Kind::Model, Rights::empty()),
        Err(Error::BadHandle)
    );
    assert_eq!(
        table.check(handle, Kind::Sched, Rights::MAP),
        Err(Error::Permission)
    );
    assert!(table.holds(Kind::Sched, Rights::READ));
    assert!(!table.holds(Kind::Sched, Rights::WRITE));
}

#[test]
fn a_closed_handle_never_resolves_even_after_its_slot_is_reused() {
    let mut table = Table::new().unwrap();
    let first = table.insert(sched(Rights::READ)).unwrap();
    assert_eq!(table.close(first).unwrap().rights, Rights::READ);
    assert_eq!(table.close(first), Err(Error::BadHandle));
    let second = table.insert(sched(ALL)).unwrap();
    assert_eq!(first.index(), second.index());
    assert_ne!(first, second);
    assert_eq!(table.get(first), Err(Error::BadHandle));
    assert!(
        !table.holds(Kind::Sched, Rights::READ)
            || table.find(Kind::Sched, Rights::READ) == Some(second)
    );
}

#[test]
fn dup_needs_duplicate_and_a_subset() {
    let mut table = Table::new().unwrap();
    let plain = table
        .insert(sched(Rights::READ | Rights::SET_POLICY))
        .unwrap();
    assert_eq!(table.dup(plain, Rights::READ), Err(Error::Permission));
    let source = table
        .insert(sched(Rights::DUPLICATE | Rights::READ | Rights::SET_POLICY))
        .unwrap();
    let narrow = table.dup(source, Rights::READ).unwrap();
    assert_eq!(table.get(narrow).unwrap().rights, Rights::READ);
    assert_eq!(table.dup(source, Rights::MAP), Err(Error::Invalid));
    assert_eq!(table.dup(narrow, Rights::READ), Err(Error::Permission));
}

#[test]
fn replace_narrows_and_closes_the_old_handle_even_when_refused() {
    let mut table = Table::new().unwrap();
    let old = table
        .insert(sched(Rights::READ | Rights::SET_POLICY))
        .unwrap();
    let new = table.replace(old, Rights::READ).unwrap();
    assert_eq!(table.get(old), Err(Error::BadHandle));
    assert_eq!(table.get(new).unwrap().rights, Rights::READ);
    assert_eq!(table.replace(new, Rights::SET_POLICY), Err(Error::Invalid));
    assert_eq!(table.get(new), Err(Error::BadHandle));
    assert!(table.is_empty());
}

#[test]
fn a_once_handle_is_closed_by_its_first_use_and_never_found() {
    let mut table = Table::new().unwrap();
    let once = table
        .insert(Entry {
            flags: Flags::ONCE,
            ..sched(Rights::SET_POLICY)
        })
        .unwrap();
    assert_eq!(table.find(Kind::Sched, Rights::SET_POLICY), None);
    assert!(table.check(once, Kind::Sched, Rights::SET_POLICY).is_ok());
    assert_eq!(
        table.check(once, Kind::Sched, Rights::SET_POLICY),
        Err(Error::BadHandle)
    );
}

#[test]
fn the_table_grows_to_4096_and_no_further() {
    let mut table = Table::new().unwrap();
    for _ in 0..TABLE_MAX {
        table.insert(sched(Rights::READ)).unwrap();
    }
    assert_eq!((table.capacity(), table.len()), (TABLE_MAX, TABLE_MAX));
    assert_eq!(table.insert(sched(Rights::READ)), Err(Error::Full));
}

#[test]
fn a_slot_whose_generation_would_wrap_is_retired() {
    let mut table = Table::new().unwrap();
    table.slots[0].generation = MAX_GENERATION - 1;
    let handle = table.insert(sched(Rights::READ)).unwrap();
    assert_eq!(handle.index(), 0);
    table.close(handle).unwrap();
    let last = table.insert(sched(Rights::READ)).unwrap();
    assert_eq!((last.index(), last.generation()), (0, MAX_GENERATION));
    table.close(last).unwrap();
    // Slot 0 is gone for good; the next entry takes slot 1.
    assert_eq!(table.insert(sched(Rights::READ)).unwrap().index(), 1);
    assert_eq!(table.get(last), Err(Error::BadHandle));
}

/// A tiny xorshift, so the property runs are the same every time.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Property: under any sequence of inserts, dups, replaces, closes and checks,
/// on handles live, closed and forged, (1) rights never grow: every open entry's
/// rights are a subset of the rights of the entry it came from; (2) a handle
/// once closed never resolves again; (3) every open handle resolves to the entry
/// the model says it holds.
#[test]
fn rights_never_grow_and_dead_handles_never_resolve() {
    for seed in 1..=64u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let mut table = Table::new().unwrap();
        // The model: open handles and their entries, and every handle closed.
        let mut open: HashMap<Handle, Entry> = HashMap::new();
        let mut dead: vec::Vec<Handle> = vec::Vec::new();
        let pick = |rng: &mut Rng, open: &HashMap<Handle, Entry>, dead: &[Handle]| {
            let mut keys: vec::Vec<Handle> = open.keys().copied().collect();
            keys.sort();
            match rng.below(4) {
                0 if !dead.is_empty() => dead[rng.below(dead.len() as u64) as usize],
                1 => Handle(rng.next() as u32),
                _ if !keys.is_empty() => keys[rng.below(keys.len() as u64) as usize],
                _ => Handle(rng.next() as u32),
            }
        };
        for _ in 0..2000 {
            let rights = Rights::from_bits_truncate(rng.next() as u32);
            match rng.below(6) {
                0 => {
                    let entry = Entry {
                        flags: if rng.below(8) == 0 {
                            Flags::ONCE
                        } else {
                            Flags::empty()
                        },
                        ..Entry::new(Kind::ALL[rng.below(12) as usize], rights, rng.below(4))
                    };
                    if let Ok(handle) = table.insert(entry) {
                        assert!(
                            !dead.contains(&handle),
                            "seed {seed}: a dead handle came back"
                        );
                        open.insert(handle, entry);
                    }
                }
                1 => {
                    let handle = pick(&mut rng, &open, &dead);
                    match (table.dup(handle, rights), open.get(&handle).copied()) {
                        (Ok(new), Some(source)) => {
                            assert!(source.rights.contains(Rights::DUPLICATE));
                            assert!(source.rights.contains(rights), "seed {seed}: dup widened");
                            open.insert(new, Entry { rights, ..source });
                        }
                        (Ok(_), None) => panic!("seed {seed}: dup of a handle not open"),
                        (Err(_), _) => {}
                    }
                }
                2 => {
                    let handle = pick(&mut rng, &open, &dead);
                    let source = open.remove(&handle);
                    match (table.replace(handle, rights), source) {
                        (Ok(new), Some(source)) => {
                            assert!(
                                source.rights.contains(rights),
                                "seed {seed}: replace widened"
                            );
                            open.insert(new, Entry { rights, ..source });
                            dead.push(handle);
                        }
                        (Err(Error::Invalid), Some(_)) => dead.push(handle),
                        (Ok(_), None) => panic!("seed {seed}: replace of a handle not open"),
                        (Err(error), Some(_)) => panic!("seed {seed}: replace failed {error:?}"),
                        (Err(_), None) => {}
                    }
                }
                3 => {
                    let handle = pick(&mut rng, &open, &dead);
                    match (table.close(handle), open.remove(&handle)) {
                        (Ok(entry), Some(want)) => {
                            assert_eq!(entry, want);
                            dead.push(handle);
                        }
                        (Err(Error::BadHandle), None) => {}
                        (got, want) => panic!("seed {seed}: close gave {got:?}, model {want:?}"),
                    }
                }
                _ => {
                    let handle = pick(&mut rng, &open, &dead);
                    let kind = Kind::ALL[rng.below(12) as usize];
                    let asked = rights & Rights::from_bits_truncate(rng.next() as u32);
                    let got = table.check(handle, kind, asked);
                    match open.get(&handle).copied() {
                        Some(want) if want.kind == kind && want.rights.contains(asked) => {
                            assert_eq!(got, Ok(want));
                            if want.flags.contains(Flags::ONCE) {
                                open.remove(&handle);
                                dead.push(handle);
                            }
                        }
                        Some(want) if want.kind != kind => assert_eq!(got, Err(Error::BadHandle)),
                        Some(_) => assert_eq!(got, Err(Error::Permission)),
                        None => assert_eq!(got, Err(Error::BadHandle)),
                    }
                }
            }
            for handle in &dead {
                assert_eq!(table.get(*handle), Err(Error::BadHandle), "seed {seed}");
            }
        }
        assert_eq!(table.len(), open.len());
        for (handle, want) in &open {
            assert_eq!(table.get(*handle), Ok(want), "seed {seed}");
        }
    }
}
