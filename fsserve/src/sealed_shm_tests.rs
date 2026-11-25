//! [`Request::SealedPushShm`]: a model blob pushed 1 MiB a round trip through
//! an `Shm` is the blob 7 KiB pushes make, and the hash check refuses one whose
//! bytes are not its name, as it does for messages.

use crate::tests::{ALICE, BLOCKS, Client};
use crate::*;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use kabi::modelhint::FNV_ATTR;
use kaifs::MemDisk;
use sha2::{Digest, Sha256};

fn model(len: usize) -> Vec<u8> {
    let mut bytes = b"GGUF\x03\0\0\0".to_vec();
    bytes.extend((0..len - 8).map(|i| (i % 251) as u8));
    bytes
}

fn name_of(bytes: &[u8]) -> String {
    let sha: [u8; 32] = Sha256::digest(bytes).into();
    sha.iter().map(|b| format!("{b:02x}")).collect::<String>() + ".gguf"
}

fn store(c: &mut Client<'_>, request: Request, shm: Option<&mut [u8]>) -> Reply {
    let bytes = fsproto::encode_request(&request);
    let answer = c.server.call_shm(
        &Call {
            caller: ALICE,
            badge: MODELS_ROOT,
            rights: DIR_RIGHTS,
            request: &bytes,
            now: 1_000,
        },
        shm,
    );
    fsproto::decode_reply(&answer.reply).unwrap()
}

/// Begins an action and blob `name`, then pushes `bytes` through `shm` in
/// pieces of its length; the finish's reply.
fn import_shm(c: &mut Client<'_>, name: &str, bytes: &[u8], shm: &mut [u8]) -> Reply {
    assert_eq!(store(c, Request::Begin { action: 1 }, None), Reply::Done);
    let begin = Request::SealedBegin {
        name: name.into(),
        len: bytes.len() as u64,
    };
    assert_eq!(store(c, begin, None), Reply::Done);
    for piece in bytes.chunks(shm.len()) {
        shm[..piece.len()].copy_from_slice(piece);
        let push = Request::SealedPushShm {
            len: piece.len() as u32,
        };
        assert_eq!(store(c, push, Some(shm)), Reply::Done);
    }
    store(c, Request::SealedFinish, None)
}

#[test]
fn a_blob_pushed_through_an_shm_is_kept_sealed_with_its_fnv() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    let bytes = model((5 << 20) + 4093);
    let name = name_of(&bytes);
    let mut shm = vec![0u8; MAX_SHM_DATA];
    assert_eq!(import_shm(&mut c, &name, &bytes, &mut shm), Reply::Done);
    assert!(matches!(
        store(&mut c, Request::Commit, None),
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
fn a_blob_pushed_through_an_shm_whose_bytes_are_not_its_name_is_never_kept() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    let bytes = model(3 << 20);
    let liar = name_of(b"some other model");
    let mut shm = vec![0u8; MAX_SHM_DATA];
    let finish = import_shm(&mut c, &liar, &bytes, &mut shm);
    assert_eq!(finish, Reply::Failed(Error::Invalid));
    // The failed finish aborted the action: no Commit can keep the file.
    assert_eq!(
        store(&mut c, Request::Commit, None),
        Reply::Failed(Error::Invalid)
    );
    let fs = &mut c.server.fs;
    fs.set_volume(MODELS.into()).unwrap();
    assert!(fs.lookup(ROOT, &liar).unwrap().is_none());
}

#[test]
fn a_bulk_push_needs_its_shm_the_bytes_it_names_and_the_store() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    let bytes = model(100_000);
    let name = name_of(&bytes);
    assert_eq!(
        store(&mut c, Request::Begin { action: 1 }, None),
        Reply::Done
    );
    let begin = Request::SealedBegin {
        name,
        len: bytes.len() as u64,
    };
    assert_eq!(store(&mut c, begin, None), Reply::Done);
    let mut shm = vec![0u8; 64 << 10];
    // No `Shm`, or more bytes than it holds or than a request may move.
    let push = |len| Request::SealedPushShm { len };
    assert_eq!(
        store(&mut c, push(4096), None),
        Reply::Failed(Error::Invalid)
    );
    assert_eq!(
        store(&mut c, push((64 << 10) + 1), Some(&mut shm)),
        Reply::Failed(Error::Invalid)
    );
    let mut big = vec![0u8; MAX_SHM_DATA + 4096];
    assert_eq!(
        store(&mut c, push(MAX_SHM_DATA as u32 + 1), Some(&mut big)),
        Reply::Failed(Error::Invalid)
    );
    // Off the store's `Dir`: refused before anything is pushed.
    let bytes_req = fsproto::encode_request(&push(4096));
    let home = c.server.call_shm(
        &Call {
            caller: ALICE,
            badge: HOME_USER,
            rights: DIR_RIGHTS,
            request: &bytes_req,
            now: 2_000,
        },
        Some(&mut shm),
    );
    assert_eq!(
        fsproto::decode_reply(&home.reply).unwrap(),
        Reply::Failed(Error::Permission)
    );
}
