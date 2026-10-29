//! The store's operations over kaifs on a memory disk: what each one switches
//! to, what it refuses, and what GC keeps (ADR 0011 §5-§7).

mod common;

use common::*;
use kaifs::{Fs, MemDisk};
use kaipkg::generation::Time;
use kaipkg::resolve::Want;
use kaipkg::store::{self, Ctx, Error, Policy, Refused, Store, Volume};
use kaipkg::version::Range;

const DAY: u64 = 86_400;

struct World {
    vol: KaifsVol<MemDisk>,
    ledger: TestLedger,
    store: Store,
    now: Time,
    action: u64,
}

impl World {
    fn new() -> World {
        let fs = Fs::mkfs(MemDisk::new(4096), *b"kaipkg store tst", "store").unwrap();
        let mut w = World {
            vol: KaifsVol::new(fs),
            ledger: TestLedger,
            store: Store {
                policy: Policy {
                    keep_last: 3,
                    keep_secs: 5 * DAY,
                },
            },
            now: Time::parse("2026-11-02T10:31:00Z").unwrap(),
            action: 1,
        };
        let store = w.store;
        assert_eq!(store.setup(&mut w.cx(), Vec::new()), Ok(true));
        assert_eq!(store.setup(&mut w.cx(), Vec::new()), Ok(false));
        w
    }

    fn cx(&mut self) -> Ctx<'_> {
        self.action += 1;
        self.now.0 += DAY;
        Ctx {
            vol: &mut self.vol,
            ledger: &mut self.ledger,
            now: self.now,
            action: self.action,
        }
    }

    fn install_from(&mut self, name: &str, tamper: Tamper, force: bool) -> Result<u64, Error> {
        let (index, mut repo) = spoiled(name, tamper);
        repo.piece = 4 << 10;
        let store = self.store;
        let want = [Want { name, range: None }];
        store.install(&mut self.cx(), &mut repo, &index, OS, &want, force)
    }

    fn install(&mut self, name: &str) -> Result<u64, Error> {
        self.install_from(name, Tamper::None, false)
    }

    fn names(&mut self) -> Vec<(String, bool)> {
        let cur = store::current(&mut self.vol).unwrap();
        let g = store::generation(&mut self.vol, cur).unwrap();
        g.packages
            .into_iter()
            .map(|p| (p.name, p.explicit))
            .collect()
    }

    fn check(&mut self) -> State {
        let (store, now) = (self.store, self.now);
        state(&mut self.vol, &store, now).unwrap()
    }
}

fn refused(r: Result<u64, Error>) -> Refused {
    match r {
        Err(Error::Refused(why)) => why,
        other => panic!("not refused: {other:?}"),
    }
}

#[test]
fn the_image_seeds_generation_1_once() {
    let fs = Fs::mkfs(MemDisk::new(4096), *b"kaipkg store tst", "store").unwrap();
    let mut vol = KaifsVol::new(fs);
    let mut ledger = TestLedger;
    let store = Store {
        policy: Policy::default(),
    };
    let image: Vec<_> = ["ui-kit", "weather"]
        .into_iter()
        .map(|name| {
            let bytes = object(find(name));
            (entry_for(find(name), &bytes), bytes)
        })
        .collect();
    let mut seed = |action, image: &[(kaipkg::resolve::Entry, Vec<u8>)]| {
        let mut cx = Ctx {
            vol: &mut vol,
            ledger: &mut ledger,
            now: Time::parse("2026-11-02T10:31:00Z").unwrap(),
            action,
        };
        store.seed(&mut cx, image)
    };
    // An object whose bytes are not its entry's stops the seed before any switch.
    let mut spoiled = image.clone();
    spoiled[1].1[40] ^= 1;
    let err = seed(1, &spoiled);
    assert_eq!(err, Err(Error::Refused(Refused::BadHash("weather".into()))));
    assert_eq!(seed(2, &image), Ok(true));
    assert_eq!(seed(3, &image[..1]), Ok(false));
    assert_eq!(store::current(&mut vol), Ok(1));
    let first = store::generation(&mut vol, 1).unwrap();
    let names: Vec<_> = first
        .packages
        .iter()
        .map(|p| (&*p.name, p.explicit))
        .collect();
    assert_eq!(names, [("ui-kit", true), ("weather", true)]);
    assert_eq!(first.packages[1].grants, find("weather").grants);
    for (entry, _) in &image {
        assert_eq!(store::present(&mut vol, &entry.sha), Ok(true));
    }
    let now = Time::parse("2026-11-03T10:31:00Z").unwrap();
    let checked = state(&mut vol, &store, now).unwrap();
    assert_eq!(checked.store.current, 1);
}

#[test]
fn an_install_brings_its_dependencies_and_one_switch() {
    let mut w = World::new();
    assert_eq!(w.install("notes"), Ok(2));
    let mut names = w.names();
    names.sort();
    let want = [("notes", true), ("spell-en", false), ("ui-kit", false)];
    assert_eq!(names, want.map(|(n, e)| (n.to_string(), e)));
    let s = w.check();
    assert_eq!(s.store.objects.len(), 3);
    assert_eq!(
        refused(w.install("notes")),
        Refused::AlreadyInstalled("notes".into())
    );
}

#[test]
fn grants_follow_the_generation_and_roll_back_with_it() {
    let mut w = World::new();
    w.install("maps").unwrap();
    let s = w.check();
    assert!(s.grants.contains("pkg maps location\n"));
    assert!(s.grants.contains("pkg weather network\n"));
    let store = w.store;
    assert_eq!(store.rollback(&mut w.cx(), 1), Ok(3));
    let s = w.check();
    assert_eq!(s.grants, BASE_GRANTS);
    // Undoing the rollback is another switch: forward to 2's packages again.
    assert_eq!(store.rollback(&mut w.cx(), 2), Ok(4));
    assert!(w.check().grants.contains("pkg maps location\n"));
    assert_eq!(
        refused(store.rollback(&mut w.cx(), 4)),
        Refused::NothingToDo
    );
    assert_eq!(
        refused(store.rollback(&mut w.cx(), 9)),
        Refused::NoGeneration(9)
    );
}

#[test]
fn a_remove_keeps_what_others_need_and_fetches_nothing() {
    let mut w = World::new();
    w.install("notes").unwrap();
    w.install("weather").unwrap();
    let store = w.store;
    store.remove(&mut w.cx(), &index(), OS, "notes").unwrap();
    let mut names = w.names();
    names.sort();
    let want = [("ui-kit", false), ("weather", true)];
    assert_eq!(names, want.map(|(n, e)| (n.to_string(), e)));
    assert_eq!(
        refused(store.remove(&mut w.cx(), &index(), OS, "notes")),
        Refused::NotInstalled("notes".into())
    );
    w.check();
}

#[test]
fn tampered_downloads_are_refused_and_never_visible() {
    let mut w = World::new();
    assert_eq!(
        refused(w.install_from("camera", Tamper::FlipByte, false)),
        Refused::BadHash("camera".into())
    );
    assert_eq!(
        refused(w.install_from("camera", Tamper::WrongHeader, false)),
        Refused::HeaderDiffers("camera".into())
    );
    let s = w.check();
    assert_eq!(s.store.current, 1);
    assert!(s.store.objects.is_empty());
    assert!(w.vol.list(store::PARTIAL).unwrap().is_empty());
}

#[test]
fn a_download_resumes_from_its_partial_file() {
    let mut w = World::new();
    let (index, mut repo) = spoiled("spell-en", Tamper::None);
    repo.piece = 4 << 10;
    let entry = index.iter().find(|e| e.name == "spell-en").unwrap().clone();
    // A download cut off after two pieces, as a crash leaves it.
    let bytes = object(find("spell-en"));
    let partial = format!("{}/{}", store::PARTIAL, kaipkg::hex(&entry.sha));
    w.vol.append(&partial, &bytes[..8 << 10]).unwrap();
    w.vol.commit().unwrap();
    let store = w.store;
    let want = [Want {
        name: "spell-en",
        range: Some(Range::parse("^1").unwrap()),
    }];
    store
        .install(&mut w.cx(), &mut repo, &index, OS, &want, false)
        .unwrap();
    assert_eq!(repo.fetched, bytes.len() - (8 << 10));
    w.check();
}

#[test]
fn a_failed_probation_goes_back_to_good_and_is_not_retried() {
    let mut w = World::new();
    let store = w.store;
    w.install("camera").unwrap();
    assert_eq!(store.fail_probation(&mut w.cx()), Ok(3));
    let s = w.check();
    assert_eq!((s.store.current, s.store.good, s.store.failed), (3, 1, 1));
    assert_eq!(
        refused(w.install("camera")),
        Refused::Failed("camera".into())
    );
    assert_eq!(w.install_from("camera", Tamper::None, true), Ok(4));
    assert_eq!(store.mark_good(&mut w.cx()), Ok(4));
    assert_eq!(refused(store.mark_good(&mut w.cx())), Refused::NothingToDo);
    assert_eq!(
        refused(store.fail_probation(&mut w.cx())),
        Refused::NothingToDo
    );
}

#[test]
fn gc_keeps_the_newest_the_recent_current_and_good() {
    let mut w = World::new();
    let store = w.store;
    for name in ["notes", "camera", "maps"] {
        w.install(name).unwrap();
        store.remove(&mut w.cx(), &index(), OS, name).unwrap();
    }
    // Generations 1-7; days pass so only the newest three are recent.
    w.now.0 += 10 * DAY;
    let (gens, objects) = store.gc(&mut w.cx()).unwrap();
    assert_eq!(gens, 3, "1 is good, 5-7 are the newest; 2-4 go");
    assert!(objects > 0);
    let s = w.check();
    assert_eq!(s.store.generations, vec![1, 5, 6, 7]);
    assert_eq!(
        refused(store.gc(&mut w.cx()).map(|_| 0)),
        Refused::NothingToDo
    );
    assert!(s.log_lines >= 8);
}

#[test]
fn an_unresolvable_install_switches_nothing() {
    let mut w = World::new();
    let (mut index, mut repo) = spoiled("notes", Tamper::None);
    index.retain(|e| e.name != "ui-kit");
    let store = w.store;
    let want = [Want {
        name: "notes",
        range: None,
    }];
    let got = store.install(&mut w.cx(), &mut repo, &index, OS, &want, false);
    assert_eq!(
        refused(got),
        Refused::Unresolved(
            "Cannot install notes 2.1.0: it needs ui-kit: no package by that name.".into()
        )
    );
    assert_eq!(w.check().store.current, 1);
}

#[test]
fn a_remove_resolves_what_stays_against_the_os() {
    // notes needs kai:app, which only the OS provides: removing weather beside it
    // must still resolve.
    let mut w = World::new();
    w.install("notes").unwrap();
    w.install("weather").unwrap();
    let store = w.store;
    store.remove(&mut w.cx(), &index(), OS, "weather").unwrap();
    assert!(w.names().iter().any(|(n, e)| n == "notes" && *e));
    w.check();
}
