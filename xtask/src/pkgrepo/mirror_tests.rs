//! Installs from the mirror end to end (P4 task 2's host half): xtask's server
//! ([`super::serve`]) over TCP, `kaipkg::mirror`'s client, and the store on a
//! `MemVolume`, as `pkg` will run it with PKG_FETCH. ADR 0011's five tamper
//! cases are each refused by name: a flipped object byte, an expired timestamp,
//! an older snapshot, an unknown key, and a header that differs from its index
//! entry.

use super::mirrors::{at_version, big, package, tampered};
use super::tests::{NOW, hello, keys, notes};
use super::tuf;
use super::{Repository, serve};
use kaipkg::generation::{Generation, Time};
use kaipkg::memvol::MemVolume;
use kaipkg::mirror::{self, Exchange, Listing, Mirror, TUF_DIR, TufCache};
use kaipkg::resolve::Want;
use kaipkg::store::{self, Ctx, Error, Ledger, Refused, Store, Volume};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;

/// One request a connection, as the kernel's exchange sends it.
struct Tcp(u16);

impl Exchange for Tcp {
    fn exchange(&mut self, request: &[u8]) -> Result<Vec<u8>, String> {
        let mut stream = TcpStream::connect(("127.0.0.1", self.0)).map_err(|e| e.to_string())?;
        stream.write_all(request).map_err(|e| e.to_string())?;
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .map_err(|e| e.to_string())?;
        Ok(response)
    }
}

struct NoLedger;

impl Ledger for NoLedger {
    fn switched(&mut self, _: &mut dyn Volume, _: u64, _: &Generation) -> Result<(), String> {
        Ok(())
    }

    fn collected(&mut self, _: &mut dyn Volume, _: &[u64], _: usize) -> Result<(), String> {
        Ok(())
    }
}

fn file<'a>(repo: &'a Repository, path: &str) -> &'a [u8] {
    let found = repo.files.iter().find(|(p, _)| p == path);
    &found.unwrap_or_else(|| panic!("no {path}")).1
}

fn serve(repo: &Repository) -> u16 {
    let copy = Repository {
        files: repo.files.clone(),
    };
    serve::start(copy, 0, Arc::new(|_: &str, _, _| {})).unwrap()
}

/// A device that trusts `repo`'s root, as the image's.
fn device(repo: &Repository) -> MemVolume {
    let mut vol = MemVolume::default();
    let root = file(repo, "metadata/1.root.json");
    vol.write(&format!("{TUF_DIR}/root.json"), root).unwrap();
    vol
}

fn listing(vol: &mut MemVolume, port: u16, now: Time) -> (Result<Listing, String>, usize) {
    let mut mirror = Mirror::new(Tcp(port));
    let listing = mirror::index(&mut mirror, &mut TufCache { vol }, now.0);
    (listing, mirror.requests)
}

fn install(vol: &mut MemVolume, port: u16, name: &str) -> (Result<u64, Error>, usize) {
    let entries = listing(vol, port, NOW).0.unwrap().entries;
    let mut ledger = NoLedger;
    let mut cx = Ctx {
        vol,
        ledger: &mut ledger,
        now: NOW,
        action: 1,
    };
    let store = Store::default();
    store.setup(&mut cx, Vec::new()).unwrap();
    let mut mirror = Mirror::new(Tcp(port));
    let want = [Want { name, range: None }];
    let got = store.install(
        &mut cx,
        &mut mirror,
        &entries,
        kaipkg::PROVIDED,
        &want,
        false,
    );
    (got, mirror.requests)
}

fn refused(result: Result<u64, Error>) -> Refused {
    match result {
        Err(Error::Refused(why)) => why,
        other => panic!("not refused: {other:?}"),
    }
}

#[test]
fn a_package_installs_from_the_mirror_in_pieces_and_undo_brings_no_fetch() {
    let repo = tuf::build(&[notes(), hello(), big()], &keys(1), NOW).unwrap();
    let port = serve(&repo);
    let mut vol = device(&repo);
    let (found, _) = listing(&mut vol, port, NOW);
    let found = found.unwrap();
    assert!(found.refused.is_empty(), "{:?}", found.refused);
    let names: Vec<&str> = found.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["big", "hello", "notes"]);
    // The metadata it verified is kept, so the next refresh guards against rollback.
    assert!(
        vol.files()
            .contains_key(&format!("{TUF_DIR}/snapshot.json"))
    );
    let (n, fetches) = install(&mut vol, port, "big");
    assert_eq!(n, Ok(2));
    assert_eq!(fetches, 2, "300 KiB comes in two 192 KiB pieces");
    let store = Store::default();
    let mut ledger = NoLedger;
    let mut cx = Ctx {
        vol: &mut vol,
        ledger: &mut ledger,
        now: NOW,
        action: 2,
    };
    assert_eq!(store.rollback(&mut cx, 1), Ok(3));
    let (n, fetches) = install(&mut vol, port, "big");
    assert_eq!((n, fetches), (Ok(4), 0), "the store already holds it");
    let summary = store::check(&mut vol, &store, NOW).unwrap();
    assert_eq!(summary.current, 4);
}

#[test]
fn a_flipped_object_byte_is_refused_by_name() {
    let repo = tuf::build(&[big()], &keys(1), NOW).unwrap();
    let sha = crate::sha256_hex(&big().bytes);
    let flipped = tampered(&repo, &format!("targets/{sha}.big-1.0.0.kpkg"), |b| {
        b[200_000] ^= 1
    });
    let port = serve(&flipped);
    let mut vol = device(&repo);
    let (result, _) = install(&mut vol, port, "big");
    assert_eq!(refused(result), Refused::BadHash("big".into()));
    let partial = vol.files().keys().any(|p| p.contains(".partial"));
    assert!(!partial, "a refused download leaves nothing");
}

#[test]
fn a_target_the_mirror_does_not_serve_is_a_fetch_error_not_a_tamper() {
    let repo = tuf::build(&[big()], &keys(1), NOW).unwrap();
    let sha = crate::sha256_hex(&big().bytes);
    let target = format!("targets/{sha}.big-1.0.0.kpkg");
    // Listed by the index, but answered 404, as while the mirror syncs.
    let syncing = Repository {
        files: repo
            .files
            .iter()
            .filter(|(p, _)| *p != target)
            .cloned()
            .collect(),
    };
    let port = serve(&syncing);
    let mut vol = device(&repo);
    let (result, fetches) = install(&mut vol, port, "big");
    assert_eq!(fetches, 1);
    match result {
        Err(Error::Store(why)) => assert!(why.contains("the mirror has no"), "{why}"),
        other => panic!("not a fetch error: {other:?}"),
    }
}

#[test]
fn an_expired_timestamp_lists_nothing() {
    let repo = tuf::build(&[big()], &keys(1), NOW).unwrap();
    let port = serve(&repo);
    let late = Time(NOW.0 + 7 * 86_400);
    let (result, _) = listing(&mut device(&repo), port, late);
    let err = result.unwrap_err();
    assert!(err.starts_with("expired: timestamp"), "{err}");
}

#[test]
fn an_older_snapshot_than_the_device_holds_is_refused() {
    let repo = tuf::build(&[big()], &keys(1), NOW).unwrap();
    let mut vol = device(&repo);
    let newer = at_version(&repo, &keys(1), 2, 2).unwrap();
    let (seen, _) = listing(&mut vol, serve(&newer), NOW);
    assert!(seen.is_ok(), "{seen:?}");
    // A newer timestamp that names the snapshot the device has moved past.
    let rolled_back = at_version(&repo, &keys(1), 3, 1).unwrap();
    let (result, _) = listing(&mut vol, serve(&rolled_back), NOW);
    let err = result.unwrap_err();
    assert!(
        err.starts_with("bad version: ") && err.contains("snapshot"),
        "{err}"
    );
}

#[test]
fn metadata_signed_by_an_unknown_key_is_refused() {
    let ours = tuf::build(&[big()], &keys(1), NOW).unwrap();
    let theirs = tuf::build(&[big()], &keys(9), NOW).unwrap();
    let (result, _) = listing(&mut device(&ours), serve(&theirs), NOW);
    let err = result.unwrap_err();
    assert!(err.starts_with("below the signature threshold"), "{err}");
}

#[test]
fn a_header_that_differs_from_its_index_entry_is_refused_by_name() {
    // Its index says 1.0.0; the bytes it serves say 9.9.9.
    let mut liar = package("liar", "1.0.0", b"data");
    liar.bytes = package("liar", "9.9.9", b"data").bytes;
    let repo = tuf::build(&[liar], &keys(1), NOW).unwrap();
    let port = serve(&repo);
    let mut vol = device(&repo);
    let (result, _) = install(&mut vol, port, "liar");
    assert_eq!(refused(result), Refused::HeaderDiffers("liar".into()));
}

#[test]
fn each_case_mirror_gives_the_answer_its_boot_case_expects() {
    use super::mirrors::{self, MirrorKind};
    let keys = keys(1);
    let image = [notes()];
    let built = |kind| mirrors::build(kind, &image, &keys, NOW).unwrap();
    // The device trusts the image's root, which the same keys sign.
    let device_of = || device(&built(MirrorKind::Signed));
    let (n, fetches) = install(&mut device_of(), serve(&built(MirrorKind::Signed)), "big");
    assert_eq!((n, fetches), (Ok(2), 2));
    let (flipped, _) = install(
        &mut device_of(),
        serve(&built(MirrorKind::FlippedByte)),
        "big",
    );
    assert_eq!(refused(flipped), Refused::BadHash("big".into()));
    let (header, _) = install(
        &mut device_of(),
        serve(&built(MirrorKind::HeaderDiffers)),
        "big",
    );
    assert_eq!(refused(header), Refused::HeaderDiffers("big".into()));
    let index_err =
        |vol: &mut MemVolume, kind| listing(vol, serve(&built(kind)), NOW).0.unwrap_err();
    assert!(index_err(&mut device_of(), MirrorKind::Expired).starts_with("expired: timestamp"));
    assert!(
        index_err(&mut device_of(), MirrorKind::UnknownKey)
            .starts_with("below the signature threshold")
    );
    let mut vol = device_of();
    assert!(
        listing(&mut vol, serve(&built(MirrorKind::Newer)), NOW)
            .0
            .is_ok()
    );
    assert!(index_err(&mut vol, MirrorKind::RolledBack).starts_with("bad version"));
}

#[test]
fn the_catalogue_lists_what_is_installed_then_the_mirrors_offers() {
    use super::mirrors::{self, MirrorKind};
    use kaipkg::catalog;
    let keys = keys(1);
    let image = [notes()];
    let signed = mirrors::build(MirrorKind::Signed, &image, &keys, NOW).unwrap();
    let mut vol = device(&signed);
    let listing = listing(&mut vol, serve(&signed), NOW).0.unwrap();
    // As pkg-install's boot 1 leaves it: notes from the image, big from the mirror.
    let installed: Vec<_> = listing
        .indexes
        .iter()
        .filter(|i| i.name == "notes" || i.name == "big")
        .cloned()
        .collect();
    let from_image: Vec<_> = listing
        .indexes
        .iter()
        .filter(|i| i.name == "notes")
        .cloned()
        .collect();
    let rows = catalog::rows(&installed, &from_image, &listing.indexes);
    let text = catalog::page(&rows, 0, kabi::MAX_MESSAGE);
    let starts: Vec<String> = text
        .lines()
        .map(|line| line.splitn(5, '\t').take(4).collect::<Vec<_>>().join(" "))
        .collect();
    assert_eq!(
        starts,
        [
            "kai-catalog 1",
            "installed big 1.0.0 data",
            "installed notes 1.0.0 app",
            "mirror extra 1.0.0 data",
        ]
    );
    assert_eq!(catalog::parse(&text).unwrap(), rows);
}
