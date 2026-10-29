//! Installs from the mirror end to end (P4 task 2's host half): xtask's server
//! ([`super::serve`]) over TCP, `kaipkg::mirror`'s client, and the store on a
//! `MemVolume`, as `pkg` will run it with PKG_FETCH. ADR 0011's five tamper
//! cases are each refused by name: a flipped object byte, an expired timestamp,
//! an older snapshot, an unknown key, and a header that differs from its index
//! entry.

use super::tests::{NOW, hello, keys, notes, tampered};
use super::tuf::{self, envelope};
use super::{Package, Repository, serve};
use kaipkg::bundle;
use kaipkg::generation::{Generation, Time};
use kaipkg::header::Header;
use kaipkg::memvol::MemVolume;
use kaipkg::mirror::{self, Exchange, Listing, Mirror, TUF_DIR, TufCache};
use kaipkg::resolve::Want;
use kaipkg::store::{self, Ctx, Error, Ledger, Refused, Store, Volume};
use serde_json::Value;
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

/// A 300 KiB `data` package: two of the store's 192 KiB pieces.
fn big() -> Package {
    package("big", "1.0.0", &vec![7u8; 300 << 10])
}

fn package(name: &str, version: &str, payload: &[u8]) -> Package {
    let header_text = format!(
        "kai-package 1\nname {name}\nversion {version}\nkind data\nsummary A test package\nlicence MIT\n"
    );
    Package {
        header: Header::parse(&header_text).unwrap(),
        bytes: bundle::build(&[(bundle::HEADER, header_text.as_bytes()), ("data", payload)]),
        header_text,
        actions: Vec::new(),
        grants: Vec::new(),
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
fn an_expired_timestamp_lists_nothing() {
    let repo = tuf::build(&[big()], &keys(1), NOW).unwrap();
    let port = serve(&repo);
    let late = Time(NOW.0 + 7 * 86_400);
    let (result, _) = listing(&mut device(&repo), port, late);
    let err = result.unwrap_err();
    assert!(err.starts_with("expired: timestamp"), "{err}");
}

/// `repo` with snapshot and timestamp at `version`, signed again by `keys(1)`.
fn at_version(repo: &Repository, version: u64, snapshot_version: u64) -> Repository {
    let keys = keys(1);
    let mut snapshot: Value =
        serde_json::from_slice(file(repo, "metadata/1.snapshot.json")).unwrap();
    snapshot["signed"]["version"] = snapshot_version.into();
    let snapshot = envelope(&snapshot["signed"], &keys.snapshot).unwrap();
    let mut timestamp: Value =
        serde_json::from_slice(file(repo, "metadata/timestamp.json")).unwrap();
    let signed = &mut timestamp["signed"];
    signed["version"] = version.into();
    signed["meta"]["snapshot.json"] = serde_json::json!({
        "version": snapshot_version,
        "length": snapshot.len(),
        "hashes": { "sha256": crate::sha256_hex(&snapshot) },
    });
    let timestamp = envelope(signed, &keys.timestamp).unwrap();
    let mut files: Vec<(String, Vec<u8>)> = repo
        .files
        .iter()
        .filter(|(p, _)| p != "metadata/timestamp.json")
        .cloned()
        .collect();
    files.push((
        format!("metadata/{snapshot_version}.snapshot.json"),
        snapshot,
    ));
    files.push(("metadata/timestamp.json".into(), timestamp));
    Repository { files }
}

#[test]
fn an_older_snapshot_than_the_device_holds_is_refused() {
    let repo = tuf::build(&[big()], &keys(1), NOW).unwrap();
    let mut vol = device(&repo);
    let newer = at_version(&repo, 2, 2);
    let (seen, _) = listing(&mut vol, serve(&newer), NOW);
    assert!(seen.is_ok(), "{seen:?}");
    // A newer timestamp that names the snapshot the device has moved past.
    let rolled_back = at_version(&repo, 3, 1);
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
