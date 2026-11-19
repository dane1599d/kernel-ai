//! `kai-fs 1`'s bulk requests over kaifs's in-memory disk: a file goes in and
//! comes back in 1 MiB pieces through a buffer standing in for the `Shm`, and
//! matches what the 7 KiB requests see.

use super::tests::{ALICE, Client};
use super::*;
use alloc::vec;
use fsproto::{decode_reply, encode_request};
use kaifs::MemDisk;

/// One request with `shm` as the `Shm` it moved in.
fn bulk(c: &mut Client, badge: u64, rights: Rights, request: Request, shm: &mut [u8]) -> Reply {
    let bytes = encode_request(&request);
    let answer = c.server.call_shm(
        &Call {
            caller: ALICE,
            badge,
            rights,
            request: &bytes,
            now: 1_000,
        },
        Some(shm),
    );
    decode_reply(&answer.reply).unwrap()
}

fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 13 % 251) as u8).collect()
}

#[test]
fn a_file_in_one_mib_pieces_reads_back_the_same_either_way() {
    let mut disk = MemDisk::new(4096);
    let (mut c, _) = Client::start(&mut disk);
    let file = c.open("bulk.bin", How::CreateFile).unwrap();
    // Not a whole number of pieces, so the last one and the read are short.
    let bytes = pattern(3 * MAX_SHM_DATA + 12_345);
    let mut shm = vec![0u8; MAX_SHM_DATA];
    c.home(Request::Begin { action: 1 });
    for (at, piece) in bytes.chunks(MAX_SHM_DATA).enumerate() {
        shm[..piece.len()].copy_from_slice(piece);
        let off = (at * MAX_SHM_DATA) as u64;
        let len = piece.len() as u32;
        let wrote = bulk(
            &mut c,
            file.badge,
            FILE_RIGHTS,
            Request::WriteShm { off, len },
            &mut shm,
        );
        assert_eq!(wrote, Reply::Done, "piece {at}");
    }
    let Reply::Committed { .. } = c.home(Request::Commit).0 else {
        panic!("not committed");
    };
    let mut back = Vec::new();
    loop {
        let off = back.len() as u64;
        let len = MAX_SHM_DATA as u32;
        match bulk(
            &mut c,
            file.badge,
            FILE_RIGHTS,
            Request::ReadShm { off, len },
            &mut shm,
        ) {
            Reply::Copied { len: 0 } => break,
            Reply::Copied { len } => back.extend_from_slice(&shm[..len as usize]),
            other => panic!("{other:?}"),
        }
    }
    assert!(back == bytes);
    // A 7 KiB read sees the same bytes in the middle of a piece.
    let off = MAX_SHM_DATA as u64 + 100;
    let len = MAX_DATA as u32;
    let Reply::Data(data) = c
        .call(ALICE, file.badge, FILE_RIGHTS, Request::Read { off, len })
        .0
    else {
        panic!("no data");
    };
    assert_eq!(data, bytes[off as usize..off as usize + MAX_DATA]);
}

#[test]
fn a_bulk_write_joins_a_run_of_small_writes() {
    let mut disk = MemDisk::new(4096);
    let (mut c, _) = Client::start(&mut disk);
    let file = c.open("mixed.bin", How::CreateFile).unwrap();
    let bytes = pattern(MAX_DATA + MAX_SHM_DATA);
    let mut shm = bytes[MAX_DATA..].to_vec();
    c.home(Request::Begin { action: 1 });
    assert_eq!(c.write(&file, 0, &bytes[..MAX_DATA]), Reply::Done);
    let request = Request::WriteShm {
        off: MAX_DATA as u64,
        len: MAX_SHM_DATA as u32,
    };
    assert_eq!(
        bulk(&mut c, file.badge, FILE_RIGHTS, request, &mut shm),
        Reply::Done
    );
    c.home(Request::Commit);
    let mut back = vec![0u8; bytes.len()];
    let request = Request::ReadShm {
        off: 0,
        len: u32::MAX,
    };
    // A read is never longer than the `Shm`.
    let mut big = vec![0u8; bytes.len()];
    let Reply::Copied { len } = bulk(&mut c, file.badge, FILE_RIGHTS, request, &mut big) else {
        panic!("not copied");
    };
    assert_eq!(len as usize, MAX_SHM_DATA);
    back[..MAX_SHM_DATA].copy_from_slice(&big[..MAX_SHM_DATA]);
    assert!(back[..MAX_SHM_DATA] == bytes[..MAX_SHM_DATA]);
}

#[test]
fn a_bulk_request_needs_its_shm_its_rights_and_a_file() {
    let mut disk = MemDisk::new(4096);
    let (mut c, _) = Client::start(&mut disk);
    let file = c.open("r.bin", How::CreateFile).unwrap();
    let mut shm = vec![0u8; 4096];
    let write = |len| Request::WriteShm { off: 0, len };
    let read = Request::ReadShm { off: 0, len: 16 };
    // No `Shm` moved in: the plain call refuses both.
    assert_eq!(
        failed(c.call(ALICE, file.badge, FILE_RIGHTS, write(16))),
        Error::Invalid
    );
    assert_eq!(
        failed(c.call(ALICE, file.badge, FILE_RIGHTS, read.clone())),
        Error::Invalid
    );
    // A write past the `Shm`'s end, or past the most one carries.
    let over = 4097;
    assert_eq!(
        bulk(&mut c, file.badge, FILE_RIGHTS, write(over), &mut shm),
        Reply::Failed(Error::Invalid)
    );
    let mut huge = vec![0u8; MAX_SHM_DATA + 1];
    assert_eq!(
        bulk(
            &mut c,
            file.badge,
            FILE_RIGHTS,
            write(huge.len() as u32),
            &mut huge
        ),
        Reply::Failed(Error::Invalid)
    );
    // READ alone may read but not write; on a folder, neither.
    assert_eq!(
        bulk(&mut c, file.badge, Rights::READ, write(16), &mut shm),
        Reply::Failed(Error::Permission)
    );
    assert_eq!(
        bulk(&mut c, file.badge, Rights::READ, read.clone(), &mut shm),
        Reply::Copied { len: 0 }
    );
    assert_eq!(
        bulk(&mut c, HOME_USER, DIR_RIGHTS, read, &mut shm),
        Reply::Failed(Error::IsDir)
    );
}

fn failed(reply: (Reply, Option<Mint>)) -> Error {
    match reply {
        (Reply::Failed(e), _) => e,
        other => panic!("not a failure: {other:?}"),
    }
}
