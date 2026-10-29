//! The store over `kai-fs 1`: kaipkg's own store, through `fsvol::FsVolume`,
//! through fsserve's server on an in-memory disk, as `pkg` runs it over `fs`.
//! The server is started again between steps, as a reboot starts `fs` again on
//! the same disk: an install survives it, and an undo after it puts the previous
//! generation back (the `pkg-store` case, on the host).

use fsproto::{Reply, Request};
use fsserve::{Call, DIR_RIGHTS, Server};
use kabi::fs::HOME_USER;
use kabi::handle::Rights;
use kaifs::MemDisk;
use kaipkg::bundle;
use kaipkg::fsvol::{FsVolume, Transport};
use kaipkg::generation::{Generation, Time};
use kaipkg::mirror::TufCache;
use kaipkg::resolve::{Entry, Want};
use kaipkg::store::{self, Ctx, Error, Ledger, Refused, Source, Store, Volume};
use kaipkg::version::Version;
use kaituf::updater::Cache;

const PROVIDED: &[(&str, Version)] = &[("kai:app", Version::new(0, 1, 0))];

/// A handle as the server sees one: its badge and rights.
type Badge = (u64, Rights);

struct Served<'d> {
    server: Server<&'d mut MemDisk>,
    now: u64,
    calls: usize,
    /// The call `fs` stops at, as a crash there would: it and every later one
    /// fail without reaching the server.
    stop_at: usize,
}

impl Transport for Served<'_> {
    type Handle = Badge;

    fn call(
        &mut self,
        (badge, rights): Badge,
        request: Request,
    ) -> Result<(Reply, Option<Badge>), String> {
        self.now += 1_000;
        self.calls += 1;
        if self.calls >= self.stop_at {
            return Err("fs stopped".into());
        }
        let bytes = fsproto::encode_request(&request);
        let answer = self.server.call(&Call {
            caller: 7,
            badge,
            rights,
            request: &bytes,
            now: self.now,
        });
        let reply = fsproto::decode_reply(&answer.reply).map_err(|e| format!("{e:?}"))?;
        Ok((reply, answer.mint.map(|m| (m.badge, m.rights))))
    }

    fn close(&mut self, _: Badge) {}
}

/// The store over a server started on `disk`, rooted at `/home/user` (the
/// folder does not matter to the store; `pkg` roots it at `/system/pkg`).
fn volume(disk: &mut MemDisk) -> FsVolume<Served<'_>> {
    let (server, _) = Server::start(disk, [9; 16]).unwrap();
    let served = Served {
        server,
        now: 0,
        calls: 0,
        stop_at: usize::MAX,
    };
    FsVolume::new(served, (HOME_USER, DIR_RIGHTS))
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

fn package(name: &str, payload: usize) -> Vec<u8> {
    let header = format!(
        "kai-package 1\nname {name}\nversion 1.0.0\nkind data\nsummary A test file\nlicence MIT\n"
    );
    let bytes: Vec<u8> = (0..payload).map(|i| (i * 31 % 251) as u8).collect();
    bundle::build(&[(bundle::HEADER, header.as_bytes()), ("data.bin", &bytes)])
}

fn entry(name: &str, bytes: &[u8]) -> Entry {
    Entry {
        name: name.into(),
        version: Version::new(1, 0, 0),
        sha: kaipkg::sha256(bytes),
        size: bytes.len() as u64,
        depends: Vec::new(),
        grants: Vec::new(),
        yanked: false,
    }
}

struct Memory<'a> {
    bytes: &'a [u8],
    pieces: usize,
    /// Pieces it serves before it serves no more, to stop a download midway.
    limit: usize,
}

impl Source for Memory<'_> {
    fn fetch(&mut self, _: &Entry, offset: u64, len: usize) -> Result<Vec<u8>, String> {
        if self.pieces == self.limit {
            return Err("the source stopped".into());
        }
        self.pieces += 1;
        let start = (offset as usize).min(self.bytes.len());
        Ok(self.bytes[start..(start + len).min(self.bytes.len())].to_vec())
    }
}

fn with<R>(
    vol: &mut FsVolume<Served<'_>>,
    now: u64,
    f: impl FnOnce(&Store, &mut Ctx<'_>) -> R,
) -> R {
    let mut ledger = NoLedger;
    let mut cx = Ctx {
        vol,
        ledger: &mut ledger,
        now: Time(now),
        action: 1,
    };
    f(&Store::default(), &mut cx)
}

fn names(vol: &mut dyn Volume, n: u64) -> Vec<String> {
    store::generation(vol, n)
        .unwrap()
        .packages
        .iter()
        .map(|p| p.name.clone())
        .collect()
}

#[test]
fn an_install_survives_a_restart_and_undo_after_one_puts_the_last_generation_back() {
    let mut disk = MemDisk::new(16 * 1024);
    let hello = package("hello", 300 << 10);
    {
        let mut vol = volume(&mut disk);
        with(&mut vol, 100, |store, cx| {
            assert_eq!(store.setup(cx, Vec::new()), Ok(true));
            let mut source = Memory {
                bytes: &hello,
                pieces: 0,
                limit: usize::MAX,
            };
            let want = [Want {
                name: "hello",
                range: None,
            }];
            assert_eq!(
                store.install(
                    cx,
                    &mut source,
                    &[entry("hello", &hello)],
                    PROVIDED,
                    &want,
                    false
                ),
                Ok(2)
            );
            assert_eq!(source.pieces, 2);
        });
        assert_eq!(names(&mut vol, 2), ["hello"]);
    }
    {
        let mut vol = volume(&mut disk);
        assert_eq!(store::current(&mut vol), Ok(2));
        assert!(store::present(&mut vol, &kaipkg::sha256(&hello)).unwrap());
        let summary = store::check(&mut vol, &Store::default(), Time(200)).unwrap();
        assert_eq!((summary.current, summary.generations), (2, vec![1, 2]));
        // The partial download beside the objects is not one of them.
        assert_eq!(summary.objects.len(), 1);
        with(&mut vol, 200, |store, cx| {
            assert_eq!(store.rollback(cx, 1), Ok(3))
        });
    }
    let mut vol = volume(&mut disk);
    assert_eq!(store::current(&mut vol), Ok(3));
    assert!(names(&mut vol, 3).is_empty());
    store::check(&mut vol, &Store::default(), Time(300)).unwrap();
}

#[test]
fn a_download_stopped_midway_resumes_after_a_restart() {
    let mut disk = MemDisk::new(16 * 1024);
    let big = package("big", 500 << 10);
    let index = [entry("big", &big)];
    let want = [Want {
        name: "big",
        range: None,
    }];
    {
        let mut vol = volume(&mut disk);
        with(&mut vol, 100, |store, cx| {
            store.setup(cx, Vec::new()).unwrap();
            let mut source = Memory {
                bytes: &big,
                pieces: 0,
                limit: 1,
            };
            assert!(matches!(
                store.install(cx, &mut source, &index, PROVIDED, &want, false),
                Err(Error::Store(_))
            ));
        });
        // The first piece is kept, as `.partial/<hex>` in the store's eyes.
        assert_eq!(vol.list(store::PARTIAL).unwrap().len(), 1);
        assert_eq!(store::current(&mut vol), Ok(1));
    }
    let mut vol = volume(&mut disk);
    with(&mut vol, 200, |store, cx| {
        let mut source = Memory {
            bytes: &big,
            pieces: 0,
            limit: usize::MAX,
        };
        assert_eq!(
            store.install(cx, &mut source, &index, PROVIDED, &want, false),
            Ok(2)
        );
        // 500 KiB is three pieces; one came before the restart.
        assert_eq!(source.pieces, 2);
    });
    assert!(vol.list(store::PARTIAL).unwrap().is_empty());
    store::check(&mut vol, &Store::default(), Time(300)).unwrap();
}

#[test]
fn a_package_whose_bytes_differ_is_refused_and_not_kept() {
    let mut disk = MemDisk::new(16 * 1024);
    let real = package("hello", 10_000);
    let mut other = real.clone();
    let last = other.len() - 1;
    other[last] ^= 1;
    let mut vol = volume(&mut disk);
    with(&mut vol, 100, |store, cx| {
        store.setup(cx, Vec::new()).unwrap();
        let mut source = Memory {
            bytes: &other,
            pieces: 0,
            limit: usize::MAX,
        };
        let want = [Want {
            name: "hello",
            range: None,
        }];
        assert_eq!(
            store.install(
                cx,
                &mut source,
                &[entry("hello", &real)],
                PROVIDED,
                &want,
                false
            ),
            Err(Error::Refused(Refused::BadHash("hello".into())))
        );
    });
    assert!(vol.list(store::PARTIAL).unwrap().is_empty());
    assert!(vol.list(store::OBJECTS).unwrap().is_empty());
    assert_eq!(store::current(&mut vol), Ok(1));
}

#[test]
fn a_tuf_cache_save_stopped_at_any_call_keeps_the_copy_before_it() {
    let mut disk = MemDisk::new(16 * 1024);
    // Several of fs's writes each, so a save has calls between its truncate
    // and its last bytes.
    let old = vec![b'o'; 3 * fsproto::MAX_DATA + 5];
    let new = vec![b'n'; 3 * fsproto::MAX_DATA + 9];
    let mut vol = volume(&mut disk);
    TufCache { vol: &mut vol }.save("timestamp", &old).unwrap();
    drop(vol);
    for stop in 1.. {
        let mut vol = volume(&mut disk);
        vol.transport.stop_at = vol.transport.calls + stop;
        let saved = TufCache { vol: &mut vol }.save("timestamp", &new);
        drop(vol);
        // fs started again on the disk, as after a reboot.
        let mut vol = volume(&mut disk);
        let kept = TufCache { vol: &mut vol }.load("timestamp").unwrap();
        if saved.is_ok() {
            assert_eq!(kept, Some(new), "the save that finished");
            assert!(stop > 4, "the save took {stop} calls");
            break;
        }
        assert_eq!(
            kept.as_ref(),
            Some(&old),
            "stopped at the save's call {stop}"
        );
    }
    // A copy that can't be read is an error, never "nothing kept".
    let mut vol = volume(&mut disk);
    vol.transport.stop_at = vol.transport.calls + 2;
    assert!(TufCache { vol: &mut vol }.load("timestamp").is_err());
}
