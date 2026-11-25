//! [`Request::SealedRemove`]: a model blob leaves the store with its hint and
//! the undo snapshots that held it, and its blocks come back once the GC pass
//! it starts has run.

use crate::tests::{ALICE, BLOCKS, BOB, Client};
use crate::*;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use kabi::modelhint::Hint;
use kaifs::MemDisk;
use kaifs::vol::SnapKind;
use sha2::{Digest, Sha256};

/// A model's bytes, `seed` making each blob's differ.
fn model(len: usize, seed: u8) -> Vec<u8> {
    let mut bytes = b"GGUF\x03\0\0\0".to_vec();
    bytes.extend((0..len - 8).map(|i| (i % 253) as u8 ^ seed));
    bytes
}

fn name_of(bytes: &[u8]) -> String {
    let sha: [u8; 32] = Sha256::digest(bytes).into();
    sha.iter().map(|b| format!("{b:02x}")).collect::<String>() + ".gguf"
}

fn store(c: &mut Client<'_>, caller: u64, request: Request) -> Reply {
    c.call(caller, MODELS_ROOT, DIR_RIGHTS, request).0
}

fn remove(c: &mut Client<'_>, caller: u64, name: &str) -> Reply {
    store(c, caller, Request::SealedRemove { name: name.into() })
}

/// Imports `bytes` as a blob in one action, committed; its name.
fn import(c: &mut Client<'_>, bytes: &[u8]) -> String {
    let name = name_of(bytes);
    assert_eq!(store(c, ALICE, Request::Begin { action: 1 }), Reply::Done);
    let begin = Request::SealedBegin {
        name: name.clone(),
        len: bytes.len() as u64,
    };
    assert_eq!(store(c, ALICE, begin), Reply::Done);
    for piece in bytes.chunks(fsproto::MAX_DATA) {
        let push = Request::SealedPush {
            data: piece.to_vec(),
        };
        assert_eq!(store(c, ALICE, push), Reply::Done);
    }
    assert_eq!(store(c, ALICE, Request::SealedFinish), Reply::Done);
    assert!(matches!(
        store(c, ALICE, Request::Commit),
        Reply::Committed { .. }
    ));
    name
}

fn hint(c: &mut Client<'_>) -> Option<Hint> {
    let mut block = [0u8; kaifs::BLOCK];
    c.server.fs.io.dev.read(0, &mut block).unwrap();
    Hint::decode(&block)
}

/// How many undo snapshots the models volume has.
fn undo_snapshots(c: &mut Client<'_>) -> usize {
    let fs = c.server.fs();
    let active = fs.volume();
    fs.set_volume(MODELS.into()).unwrap();
    let undo = fs
        .snapshots()
        .values()
        .filter(|s| s.kind == SnapKind::Undo)
        .count();
    fs.set_volume(active).unwrap();
    undo
}

/// Runs `idle` until the GC pass is done.
fn collect(c: &mut Client<'_>) {
    for round in 0..10_000u64 {
        if !c.server.fs().gc_running() {
            return;
        }
        c.server.idle(1_000_000_000 + round);
    }
    panic!("the GC pass never ended");
}

#[test]
fn a_removed_blob_takes_its_snapshots_and_its_blocks_come_back() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    let old = model((4 << 20) + 77, 1);
    let old_name = import(&mut c, &old);
    // A later import's undo snapshot is of the store with the old blob in it.
    let new_name = import(&mut c, &model((2 << 20) + 5, 2));
    // Each import's undo snapshot is of the store before it: the first's is
    // empty, the second's holds the old blob.
    assert_eq!(undo_snapshots(&mut c), 2);
    let hint_new = Request::ModelHint {
        name: Some(new_name.clone()),
    };
    assert_eq!(store(&mut c, ALICE, hint_new), Reply::Done);
    let (free_before, _) = c.server.fs().space();

    assert_eq!(remove(&mut c, ALICE, &old_name), Reply::Done);
    assert_eq!(undo_snapshots(&mut c), 1, "the one holding it is dropped");
    collect(&mut c);
    // A freed block is reusable two commits on, as any is in kaifs.
    for _ in 0..2 {
        let sync = c.call(ALICE, HOME_USER, DIR_RIGHTS, Request::Sync).0;
        assert!(matches!(sync, Reply::Committed { .. }), "{sync:?}");
    }
    let (free_after, _) = c.server.fs().space();
    let blob_blocks = old.len().div_ceil(kaifs::BLOCK) as u64;
    assert!(
        free_after >= free_before + blob_blocks,
        "{free_before} free before, {free_after} after, the blob was {blob_blocks}"
    );

    // Gone across a restart; the other blob is whole and reads as before.
    drop(c);
    let (mut c, _) = Client::start(&mut disk);
    let info = Request::ModelInfo {
        name: old_name.clone(),
    };
    assert_eq!(store(&mut c, ALICE, info), Reply::Failed(Error::Invalid));
    assert_eq!(
        remove(&mut c, ALICE, &old_name),
        Reply::Failed(Error::NotFound)
    );
    let info = Request::ModelInfo { name: new_name };
    assert!(matches!(store(&mut c, ALICE, info), Reply::Model { .. }));
}

#[test]
fn a_hint_naming_another_blob_is_left_alone() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    let gone = import(&mut c, &model(100_000, 3));
    let kept = import(&mut c, &model(100_000, 4));
    let hint_kept = Request::ModelHint {
        name: Some(kept.clone()),
    };
    assert_eq!(store(&mut c, ALICE, hint_kept), Reply::Done);
    let before = hint(&mut c);
    assert!(before.is_some());
    assert_eq!(remove(&mut c, ALICE, &gone), Reply::Done);
    assert_eq!(hint(&mut c), before);
}

#[test]
fn a_remove_is_refused_inside_an_action_off_the_store_and_for_a_bad_name() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    let name = import(&mut c, &model(50_000, 5));
    // Inside the caller's own action: refused, so no 30-day undo snapshot
    // keeps the blob.
    assert_eq!(
        store(&mut c, ALICE, Request::Begin { action: 2 }),
        Reply::Done
    );
    assert_eq!(remove(&mut c, ALICE, &name), Reply::Failed(Error::Invalid));
    // Another caller's action holds `fs`.
    assert_eq!(remove(&mut c, BOB, &name), Reply::Failed(Error::Busy));
    assert_eq!(store(&mut c, ALICE, Request::Abort), Reply::Done);
    // Only on the store's `Dir`, and only with WRITE.
    let home = c.call(
        ALICE,
        HOME_USER,
        DIR_RIGHTS,
        Request::SealedRemove { name: name.clone() },
    );
    assert_eq!(home.0, Reply::Failed(Error::Permission));
    let read_only = c.call(
        ALICE,
        MODELS_ROOT,
        Rights::READ,
        Request::SealedRemove { name: name.clone() },
    );
    assert_eq!(read_only.0, Reply::Failed(Error::Permission));
    // A name that is not a blob's, and a blob that is not there.
    assert_eq!(
        remove(&mut c, ALICE, "notes.txt"),
        Reply::Failed(Error::Invalid)
    );
    let missing = name_of(b"never imported");
    assert_eq!(
        remove(&mut c, ALICE, &missing),
        Reply::Failed(Error::NotFound)
    );
    // Still there after all of that.
    let info = Request::ModelInfo { name };
    assert!(matches!(store(&mut c, ALICE, info), Reply::Model { .. }));
}

/// The blob the hint names is the model the next boot maps: refused, and
/// neither it nor the hint changes. Only `fs` reads the hint, so only `fs`
/// can tell; `updated` says to switch to another model first.
#[test]
fn the_blob_the_hint_names_is_refused_and_nothing_changes() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    let booted = import(&mut c, &model(150_000, 6));
    let hint_it = Request::ModelHint {
        name: Some(booted.clone()),
    };
    assert_eq!(store(&mut c, ALICE, hint_it), Reply::Done);
    let before = hint(&mut c);
    let generation = c.server.generation();
    assert_eq!(
        remove(&mut c, ALICE, &booted),
        Reply::Failed(Error::Permission)
    );
    assert_eq!(hint(&mut c), before);
    assert_eq!(c.server.generation(), generation, "nothing was committed");
    let info = Request::ModelInfo { name: booted };
    assert!(matches!(store(&mut c, ALICE, info), Reply::Model { .. }));
}

/// A disk filled until only the reserve is left: removing a model may spend
/// half of it (with none, it was refused NoSpace), so a model can always be
/// taken out to make room, and its blocks come back once the GC pass it
/// starts has run. A removal refused spends nothing.
#[test]
fn a_model_is_removed_on_a_disk_filled_to_the_reserve() {
    let mut disk = MemDisk::new(2 * 1024);
    let (mut c, _) = Client::start(&mut disk);
    let blob = model(1 << 20, 7);
    let name = import(&mut c, &blob);
    let made = crate::reserve_tests::fill(&mut c);
    assert!(made.len() > 100, "{} files", made.len());
    let (filled, _) = c.server.fs().space();
    let missing = name_of(b"never imported");
    assert_eq!(
        remove(&mut c, ALICE, &missing),
        Reply::Failed(Error::NotFound)
    );
    let (refused, _) = c.server.fs().space();
    // Its only write is the commit any change outside an action makes first,
    // which may give back blocks waiting to be freed.
    assert!(
        refused >= filled,
        "{filled} free, {refused} after a refused removal"
    );
    // Then the blocks above the reserve that are too few for one more file,
    // taken one at a time as growth would, so the reserve is all that is left.
    let reserve = c.server.fs().io.alloc.reserve();
    while c.server.fs().io.alloc.alloc(0).is_ok() {}
    let (full, _) = c.server.fs().space();
    assert_eq!(full, reserve);

    assert_eq!(remove(&mut c, ALICE, &name), Reply::Done);
    let (spent, _) = c.server.fs().space();
    assert!(spent >= reserve / 2, "{spent} free, the reserve {reserve}");
    collect(&mut c);
    for _ in 0..2 {
        let sync = c.call(ALICE, HOME_USER, DIR_RIGHTS, Request::Sync).0;
        assert!(matches!(sync, Reply::Committed { .. }), "{sync:?}");
    }
    let (freed, _) = c.server.fs().space();
    let blob_blocks = blob.len().div_ceil(kaifs::BLOCK) as u64;
    assert!(
        freed >= full + blob_blocks / 2,
        "{full} free full, {freed} after, the blob was {blob_blocks}"
    );
    let info = Request::ModelInfo { name };
    assert_eq!(store(&mut c, ALICE, info), Reply::Failed(Error::Invalid));
}
