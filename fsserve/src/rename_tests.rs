//! [`Request::Rename`] into the `Dir` moved with it ([`Server::call_into`]):
//! DELETE on the source, CREATE on the destination, in one volume, and a
//! taken name refused.

use super::tests::{ALICE, BLOCKS, Client};
use super::*;
use fsproto::{decode_reply, encode_request};
use kaifs::MemDisk;

fn open(c: &mut Client<'_>, dir: u64, path: &str, how: How) -> Mint {
    let request = Request::Open {
        path: path.into(),
        rights: DIR_RIGHTS.bits(),
        how,
    };
    match c.call(ALICE, dir, DIR_RIGHTS, request) {
        (Reply::Opened(..), Some(mint)) => mint,
        other => panic!("{path}: {other:?}"),
    }
}

/// Rename `from` in `source` to `to` in the `Dir` `into`, with the source's
/// rights `rights`.
fn rename_into(
    c: &mut Client<'_>,
    source: &Mint,
    rights: Rights,
    from: &str,
    to: &str,
    into: (u64, Rights),
) -> Reply {
    let bytes = encode_request(&Request::Rename {
        from: from.into(),
        to: to.into(),
    });
    let answer = c.server.call_into(
        &Call {
            caller: ALICE,
            badge: source.badge,
            rights,
            request: &bytes,
            now: 1_000_000,
        },
        into,
    );
    decode_reply(&answer.reply).unwrap()
}

fn exists(c: &mut Client<'_>, dir: u64, name: &str) -> bool {
    let request = Request::Open {
        path: name.into(),
        rights: Rights::READ.bits(),
        how: How::Existing,
    };
    matches!(c.call(ALICE, dir, DIR_RIGHTS, request).0, Reply::Opened(..))
}

#[test]
fn a_rename_moves_into_the_dir_it_carried() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    let a = open(&mut c, HOME_USER, "a", How::Mkdir);
    let b = open(&mut c, HOME_USER, "b", How::Mkdir);
    open(&mut c, a.badge, "x", How::CreateFile);
    let moved = rename_into(&mut c, &a, a.rights, "x", "y", (b.badge, b.rights));
    assert_eq!(moved, Reply::Done);
    assert!(!exists(&mut c, a.badge, "x"));
    assert!(exists(&mut c, b.badge, "y"));
}

#[test]
fn a_rename_into_a_dir_needs_delete_here_and_create_there() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    let a = open(&mut c, HOME_USER, "a", How::Mkdir);
    let b = open(&mut c, HOME_USER, "b", How::Mkdir);
    open(&mut c, a.badge, "x", How::CreateFile);
    let no_create = (b.badge, b.rights.difference(Rights::CREATE));
    let refused = rename_into(&mut c, &a, a.rights, "x", "x", no_create);
    assert_eq!(refused, Reply::Failed(Error::Permission));
    let no_delete = a.rights.difference(Rights::DELETE);
    let refused = rename_into(&mut c, &a, no_delete, "x", "x", (b.badge, b.rights));
    assert_eq!(refused, Reply::Failed(Error::Permission));
    assert!(exists(&mut c, a.badge, "x"), "nothing moved");
}

#[test]
fn a_rename_into_a_dir_refuses_a_taken_name_a_file_and_another_volume() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    let a = open(&mut c, HOME_USER, "a", How::Mkdir);
    let b = open(&mut c, HOME_USER, "b", How::Mkdir);
    open(&mut c, a.badge, "x", How::CreateFile);
    let file = open(&mut c, b.badge, "y", How::CreateFile);
    let taken = rename_into(&mut c, &a, a.rights, "x", "y", (b.badge, b.rights));
    assert_eq!(taken, Reply::Failed(Error::Exists));
    let into_file = rename_into(&mut c, &a, a.rights, "x", "z", (file.badge, DIR_RIGHTS));
    assert_eq!(into_file, Reply::Failed(Error::NotDir));
    // `/system/state` is in the system volume, `a` in home.
    let other = (kabi::fs::SYSTEM_STATE, DIR_RIGHTS);
    let across = rename_into(&mut c, &a, a.rights, "x", "z", other);
    assert_eq!(across, Reply::Failed(Error::Invalid));
    assert!(exists(&mut c, a.badge, "x"), "nothing moved");
}
