//! The model store's writes (`models`): a blob streamed in, hashed by `fs`, kept
//! only whole and as named, and the hint written for a sealed blob only.

use crate::tests::{ALICE, BLOCKS, BOB, Client};
use crate::*;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use kabi::modelhint::{FNV_ATTR, Hint};
use kaifs::MemDisk;
use sha2::{Digest, Sha256};

/// A model's bytes: GGUF's magic, then filler.
fn model(len: usize) -> Vec<u8> {
    let mut bytes = b"GGUF\x03\0\0\0".to_vec();
    bytes.extend((0..len - 8).map(|i| (i % 253) as u8));
    bytes
}

fn name_of(bytes: &[u8]) -> String {
    let sha: [u8; 32] = Sha256::digest(bytes).into();
    sha.iter().map(|b| format!("{b:02x}")).collect::<String>() + ".gguf"
}

fn store(c: &mut Client<'_>, caller: u64, request: Request) -> Reply {
    c.call(caller, MODELS_ROOT, DIR_RIGHTS, request).0
}

fn fail(reply: Reply) -> Error {
    match reply {
        Reply::Failed(e) => e,
        other => panic!("not a failure: {other:?}"),
    }
}

/// Begins an action, then streams `bytes` as blob `name` in 7 KiB pushes; the
/// finish's reply.
fn import(c: &mut Client<'_>, name: &str, bytes: &[u8]) -> Reply {
    assert_eq!(store(c, ALICE, Request::Begin { action: 1 }), Reply::Done);
    let begin = Request::SealedBegin {
        name: name.into(),
        len: bytes.len() as u64,
    };
    assert_eq!(store(c, ALICE, begin), Reply::Done);
    for piece in bytes.chunks(fsproto::MAX_DATA) {
        let push = Request::SealedPush {
            data: piece.to_vec(),
        };
        assert_eq!(store(c, ALICE, push), Reply::Done);
    }
    store(c, ALICE, Request::SealedFinish)
}

#[test]
fn a_streamed_blob_is_kept_sealed_with_its_fnv() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    let bytes = model((2 << 20) + 4093);
    let name = name_of(&bytes);
    assert_eq!(import(&mut c, &name, &bytes), Reply::Done);
    assert!(matches!(
        store(&mut c, ALICE, Request::Commit),
        Reply::Committed { .. }
    ));
    let sha: [u8; 32] = Sha256::digest(&bytes).into();
    let extents = c.server.model_extents(&sha).unwrap().expect("sealed");
    let blocks: u64 = extents.iter().map(|&(_, n)| n).sum();
    assert_eq!(blocks, (bytes.len() as u64).div_ceil(4096));
    let fs = &mut c.server.fs;
    fs.set_volume(MODELS.into()).unwrap();
    let ino = fs.lookup(ROOT, &name).unwrap().unwrap().ino;
    let mut fnv = kabi::model::Fnv1a::new();
    fnv.update(&bytes);
    let kept = fs.get_xattr(ino, FNV_ATTR).unwrap().unwrap();
    assert_eq!(kept, fnv.finish().to_le_bytes());
    assert_eq!(fs.read_file(ino).unwrap(), bytes);
}

#[test]
fn a_blob_whose_bytes_are_not_its_name_is_never_kept() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    let bytes = model(70_000);
    let liar = name_of(b"some other model");
    assert_eq!(fail(import(&mut c, &liar, &bytes)), Error::Invalid);
    // The failed finish aborted the action: no Commit can keep the file.
    assert_eq!(fail(store(&mut c, ALICE, Request::Commit)), Error::Invalid);
    let fs = &mut c.server.fs;
    fs.set_volume(MODELS.into()).unwrap();
    assert!(fs.lookup(ROOT, &liar).unwrap().is_none());
}

#[test]
fn a_blob_is_written_only_whole_in_its_owners_action_on_the_store() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    let bytes = model(20_000);
    let name = name_of(&bytes);
    let begin = |name: &str| Request::SealedBegin {
        name: name.into(),
        len: bytes.len() as u64,
    };
    // Outside an action, and on any other handle, nothing is begun.
    assert_eq!(fail(store(&mut c, ALICE, begin(&name))), Error::Invalid);
    let home = c.call(ALICE, HOME_USER, DIR_RIGHTS, begin(&name)).0;
    assert_eq!(fail(home), Error::Permission);
    let read_only = DIR_RIGHTS.difference(Rights::WRITE);
    let reply = c.call(ALICE, MODELS_ROOT, read_only, begin(&name)).0;
    assert_eq!(fail(reply), Error::Permission);
    // Only a store name: 64 lower-case hex digits and `.gguf`.
    assert_eq!(
        store(&mut c, ALICE, Request::Begin { action: 1 }),
        Reply::Done
    );
    let upper = name.to_uppercase().replace(".GGUF", ".gguf");
    for bad in [
        "../x.gguf",
        "x.gguf",
        name.trim_end_matches(".gguf"),
        &upper,
    ] {
        assert_eq!(
            fail(store(&mut c, ALICE, begin(bad))),
            Error::Invalid,
            "{bad}"
        );
    }
    assert_eq!(store(&mut c, ALICE, begin(&name)), Reply::Done);
    // One blob at a time; another caller's change waits; no commit half way.
    assert_eq!(fail(store(&mut c, ALICE, begin(&name))), Error::Busy);
    let push = Request::SealedPush {
        data: bytes[..100].to_vec(),
    };
    assert_eq!(fail(store(&mut c, BOB, push.clone())), Error::Busy);
    assert_eq!(store(&mut c, ALICE, push), Reply::Done);
    assert_eq!(fail(store(&mut c, ALICE, Request::Commit)), Error::Invalid);
    // Finished short of its length: refused, and the action with it, so
    // nothing of the blob stays.
    let short = store(&mut c, ALICE, Request::SealedFinish);
    assert_ne!(
        short,
        Reply::Done,
        "a blob short of its length is not finished"
    );
    assert_eq!(fail(store(&mut c, ALICE, Request::Commit)), Error::Invalid);
    let fs = &mut c.server.fs;
    fs.set_volume(MODELS.into()).unwrap();
    assert!(fs.lookup(ROOT, &name).unwrap().is_none());
    let push = Request::SealedPush {
        data: bytes[..10].to_vec(),
    };
    assert_eq!(fail(store(&mut c, ALICE, push.clone())), Error::Invalid);
    // Abort drops a blob begun in the action.
    assert_eq!(
        store(&mut c, ALICE, Request::Begin { action: 2 }),
        Reply::Done
    );
    assert_eq!(store(&mut c, ALICE, begin(&name)), Reply::Done);
    assert_eq!(store(&mut c, ALICE, push.clone()), Reply::Done);
    assert_eq!(store(&mut c, ALICE, Request::Abort), Reply::Done);
    assert_eq!(fail(store(&mut c, ALICE, push)), Error::Invalid);
    let fs = &mut c.server.fs;
    fs.set_volume(MODELS.into()).unwrap();
    assert!(fs.lookup(ROOT, &name).unwrap().is_none());
}

#[test]
fn the_hint_names_only_a_sealed_blob_and_is_written_outside_an_action() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    let bytes = model(30_000);
    let name = name_of(&bytes);
    let hint = |name: Option<&str>| Request::ModelHint {
        name: name.map(String::from),
    };
    // Not a blob of the store yet.
    assert_eq!(
        fail(store(&mut c, ALICE, hint(Some(&name)))),
        Error::Invalid
    );
    assert_eq!(import(&mut c, &name, &bytes), Reply::Done);
    // Never inside an action: the caller's own, or another's.
    assert_eq!(
        fail(store(&mut c, ALICE, hint(Some(&name)))),
        Error::Invalid
    );
    assert_eq!(fail(store(&mut c, BOB, hint(Some(&name)))), Error::Busy);
    assert!(matches!(
        store(&mut c, ALICE, Request::Commit),
        Reply::Committed { .. }
    ));
    let elsewhere = c.call(ALICE, SYSTEM_UPDATES, DIR_RIGHTS, hint(None)).0;
    assert_eq!(fail(elsewhere), Error::Permission);
    assert_eq!(store(&mut c, ALICE, hint(Some(&name))), Reply::Done);
    let mut block = [0u8; kaifs::BLOCK];
    c.server.fs.io.dev.read(0, &mut block).unwrap();
    let mut fnv = kabi::model::Fnv1a::new();
    fnv.update(&bytes);
    assert_eq!(
        Hint::decode(&block[..kabi::modelhint::SIZE]),
        Some(Hint {
            len: bytes.len() as u64,
            fnv: fnv.finish(),
            sha256: Sha256::digest(&bytes).into(),
        })
    );
    // An ordinary file of a store name is not a model.
    let plain = name_of(b"plain");
    let fs = &mut c.server.fs;
    fs.set_volume(MODELS.into()).unwrap();
    let ino = fs.create(ROOT, &plain).unwrap();
    fs.write_file(ino, b"plain").unwrap();
    fs.commit().unwrap();
    assert_eq!(
        fail(store(&mut c, ALICE, hint(Some(&plain)))),
        Error::Invalid
    );
    // No hint: the next boot maps no model from the store.
    assert_eq!(store(&mut c, ALICE, hint(None)), Reply::Done);
    c.server.fs.io.dev.read(0, &mut block).unwrap();
    assert_eq!(Hint::decode(&block[..kabi::modelhint::SIZE]), None);
    // The filesystem is untouched by the hint: it still mounts and checks.
    assert!(
        c.server
            .model_extents(&Sha256::digest(&bytes).into())
            .unwrap()
            .is_some()
    );
}
