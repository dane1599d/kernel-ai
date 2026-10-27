//! What the store tests share: the store's files on kaifs, a test repository of
//! real `kai-pkg 1` objects, a ledger that writes grants and a log the way P2
//! task 12 will, and a source that serves (or spoils) the repository's bytes.

#![allow(dead_code)]

use kaifs::fs::SYSTEM;
use kaifs::{BlockDev, Error as FsError, FileType, Fs, ROOT};
use kaipkg::generation::{Generation, Time};
use kaipkg::header::Depends;
use kaipkg::resolve::Entry;
use kaipkg::store::{self, Ledger, Source, Store, Summary, Volume};
use kaipkg::version::{Range, Version};
use kaipkg::{bundle, sha256};

/// The store's files on kaifs's system volume.
pub struct KaifsVol<D: BlockDev> {
    pub fs: Fs<D>,
    /// With a store and a time, each commit checks the store on the live disk and
    /// keeps what it held, by generation, for the crash states to match.
    pub track: Option<(Store, Time)>,
    pub commits: Vec<(usize, u64, Result<State, String>)>,
    /// The disk log's length now, for the commit records (a recording disk's).
    pub log_len: fn(&Fs<D>) -> usize,
}

fn fs_err(e: FsError) -> String {
    format!("kaifs: {e:?}")
}

fn split(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

impl<D: BlockDev> KaifsVol<D> {
    pub fn new(mut fs: Fs<D>) -> Self {
        fs.set_volume(SYSTEM).expect("a system volume");
        KaifsVol {
            fs,
            track: None,
            commits: Vec::new(),
            log_len: |_| 0,
        }
    }

    fn ino(&mut self, path: &str) -> Result<Option<u64>, String> {
        match self.fs.resolve(&format!("/{path}")) {
            Ok(ino) => Ok(Some(ino)),
            Err(FsError::NotFound) => Ok(None),
            Err(e) => Err(fs_err(e)),
        }
    }

    /// The folder `dir`, made with its parents if absent.
    fn folder(&mut self, dir: &str) -> Result<u64, String> {
        let mut ino = ROOT;
        for part in dir.split('/').filter(|p| !p.is_empty()) {
            ino = match self.fs.lookup(ino, part).map_err(fs_err)? {
                Some(e) => e.ino,
                None => self.fs.mkdir(ino, part).map_err(fs_err)?,
            };
        }
        Ok(ino)
    }

    fn file(&mut self, path: &str) -> Result<u64, String> {
        let (dir, name) = split(path);
        let dir = self.folder(dir)?;
        match self.fs.lookup(dir, name).map_err(fs_err)? {
            Some(e) => Ok(e.ino),
            None => self.fs.create(dir, name).map_err(fs_err),
        }
    }
}

impl<D: BlockDev> Volume for KaifsVol<D> {
    fn begin(&mut self, action: u64) -> Result<(), String> {
        self.fs.begin(action).map_err(fs_err)
    }

    fn commit(&mut self) -> Result<(), String> {
        self.fs.commit().map_err(fs_err)?;
        if let Some((store, now)) = self.track {
            let state = state(self, &store, now);
            let at = (self.log_len)(&self.fs);
            self.commits.push((at, self.fs.committed, state));
        }
        Ok(())
    }

    fn read(&mut self, path: &str) -> Result<Option<Vec<u8>>, String> {
        let Some(ino) = self.ino(path)? else {
            return Ok(None);
        };
        if self.fs.stat(ino).map_err(fs_err)?.kind != FileType::File {
            return Err(format!("{path} is a folder"));
        }
        self.fs.read_file(ino).map(Some).map_err(fs_err)
    }

    fn list(&mut self, dir: &str) -> Result<Vec<String>, String> {
        let Some(ino) = self.ino(dir)? else {
            return Ok(Vec::new());
        };
        let mut names = Vec::new();
        let mut after = None;
        loop {
            let page = self.fs.list(ino, after.as_ref(), 64).map_err(fs_err)?;
            let Some(last) = page.last().cloned() else {
                break;
            };
            names.extend(page.into_iter().map(|l| l.entry.name));
            after = Some(last);
        }
        names.sort();
        Ok(names)
    }

    fn write(&mut self, path: &str, bytes: &[u8]) -> Result<(), String> {
        let ino = self.file(path)?;
        self.fs.write_file(ino, bytes).map_err(fs_err)
    }

    fn append(&mut self, path: &str, bytes: &[u8]) -> Result<(), String> {
        let ino = self.file(path)?;
        let size = self.fs.stat(ino).map_err(fs_err)?.size;
        self.fs.write_at(ino, size, bytes).map_err(fs_err)
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), String> {
        let ((fd, fname), (td, tname)) = (split(from), split(to));
        let (fd, td) = (self.folder(fd)?, self.folder(td)?);
        if self.fs.lookup(td, tname).map_err(fs_err)?.is_some() {
            self.fs.remove(td, tname).map_err(fs_err)?;
        }
        self.fs.rename(fd, fname, td, tname).map_err(fs_err)
    }

    fn remove(&mut self, path: &str) -> Result<(), String> {
        let (dir, name) = split(path);
        let dir = self.folder(dir)?;
        self.fs.remove(dir, name).map_err(fs_err)
    }
}

/// A test package: its header's fields and what ADR 0001 makes the user grant.
pub struct Package {
    pub name: &'static str,
    pub version: &'static str,
    pub depends: &'static [(&'static str, &'static str)],
    pub grants: &'static [&'static str],
    /// Bytes of payload after the header.
    pub size: usize,
}

/// Six packages with dependencies and grants, each fetched in more than one
/// piece when the source serves small ones.
pub const REPOSITORY: &[Package] = &[
    Package {
        name: "notes",
        version: "2.1.0",
        depends: &[
            ("spell-en", "^1.2"),
            ("ui-kit", "^0.1"),
            ("kai:app", "^0.1"),
        ],
        grants: &[],
        size: 9 << 10,
    },
    Package {
        name: "spell-en",
        version: "1.2.3",
        depends: &[],
        grants: &[],
        size: 12 << 10,
    },
    Package {
        name: "ui-kit",
        version: "0.1.0",
        depends: &[],
        grants: &[],
        size: 1 << 10,
    },
    Package {
        name: "weather",
        version: "0.1.0",
        depends: &[("ui-kit", "^0.1")],
        grants: &["network"],
        size: 5 << 10,
    },
    Package {
        name: "camera",
        version: "0.2.0",
        depends: &[],
        grants: &["camera"],
        size: 3 << 10,
    },
    Package {
        name: "maps",
        version: "1.0.0",
        depends: &[("weather", "^0.1")],
        grants: &["location", "network"],
        size: 7 << 10,
    },
];

pub const OS: &[(&str, Version)] = &[("kai:app", Version::new(0, 1, 0))];

pub fn find(name: &str) -> &'static Package {
    REPOSITORY
        .iter()
        .find(|p| p.name == name)
        .expect("a test package")
}

/// The header text, with `version` in place of the package's own.
pub fn header(p: &Package, version: &str) -> String {
    let mut text = format!(
        "kai-package 1\nname {}\nversion {version}\nkind data\nsummary Test package {}\n\
         licence MPL-2.0\n",
        p.name, p.name
    );
    for (name, range) in p.depends {
        text += &format!("depends {name} {range}\n");
    }
    text
}

/// The object as the repository serves it, its header claiming `version`.
pub fn object_as(p: &Package, version: &str) -> Vec<u8> {
    let mut x = 0x9E37_79B9_7F4A_7C15u64 ^ p.size as u64;
    for byte in p.name.bytes() {
        x = (x ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    let payload: Vec<u8> = (0..p.size)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect();
    let header = header(p, version);
    bundle::build(&[(bundle::HEADER, header.as_bytes()), ("payload", &payload)])
}

pub fn object(p: &Package) -> Vec<u8> {
    object_as(p, p.version)
}

pub fn entry_for(p: &Package, bytes: &[u8]) -> Entry {
    Entry {
        name: p.name.into(),
        version: Version::parse(p.version).unwrap(),
        sha: sha256(bytes),
        size: bytes.len() as u64,
        depends: p
            .depends
            .iter()
            .map(|(name, range)| Depends {
                name: (*name).into(),
                range: Range::parse(range).unwrap(),
            })
            .collect(),
        grants: p.grants.iter().map(|g| g.to_string()).collect(),
        yanked: false,
    }
}

pub fn index() -> Vec<Entry> {
    REPOSITORY
        .iter()
        .map(|p| entry_for(p, &object(p)))
        .collect()
}

/// How the served bytes are spoiled, for the tamper cases.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tamper {
    None,
    /// One byte of the named package's object flipped: its hash no longer matches.
    FlipByte,
    /// The named package's entry pins an object whose own header says 9.9.9.
    WrongHeader,
}

/// The index and the bytes behind it, with `tamper` applied to `name`.
pub fn spoiled(name: &str, tamper: Tamper) -> (Vec<Entry>, Repo) {
    let mut index = index();
    let mut repo = Repo::default();
    for (p, e) in REPOSITORY.iter().zip(index.iter_mut()) {
        let mut bytes = object(p);
        if p.name == name {
            match tamper {
                Tamper::None => {}
                Tamper::FlipByte => *bytes.last_mut().unwrap() ^= 1,
                Tamper::WrongHeader => {
                    bytes = object_as(p, "9.9.9");
                    e.sha = sha256(&bytes);
                    e.size = bytes.len() as u64;
                }
            }
        }
        repo.objects.push((e.sha, bytes));
    }
    (index, repo)
}

/// Serves objects in pieces of at most `piece` bytes.
#[derive(Default)]
pub struct Repo {
    pub objects: Vec<([u8; 32], Vec<u8>)>,
    pub piece: usize,
    pub fetched: usize,
}

impl Source for Repo {
    fn fetch(&mut self, entry: &Entry, offset: u64, len: usize) -> Result<Vec<u8>, String> {
        let bytes = &self
            .objects
            .iter()
            .find(|(sha, _)| *sha == entry.sha)
            .ok_or("no such object")?
            .1;
        let len = if self.piece > 0 {
            len.min(self.piece)
        } else {
            len
        };
        let from = (offset as usize).min(bytes.len());
        let to = (from + len).min(bytes.len());
        self.fetched += to - from;
        Ok(bytes[from..to].to_vec())
    }
}

/// Grants no package gave, which a switch must keep.
pub const BASE_GRANTS: &str = "kai-grants 1\nrule user process_stop build-*\n";

/// The grants a generation's packages hold, as `kai-grants 1` lines.
pub fn grants_of(g: &Generation) -> String {
    let mut lines: Vec<String> = g
        .packages
        .iter()
        .flat_map(|p| {
            p.grants
                .iter()
                .map(move |gr| format!("pkg {} {gr}\n", p.name))
        })
        .collect();
    lines.sort();
    BASE_GRANTS.to_string() + &lines.concat()
}

/// P2 task 12's stand-in: `grants` and `log` files written in the switch's
/// transaction.
pub struct TestLedger;

pub const GRANTS: &str = "grants";
pub const LOG: &str = "log";

fn append_log(vol: &mut dyn Volume, line: &str) -> Result<(), String> {
    let id = vol
        .read(LOG)?
        .map_or(0, |b| b.iter().filter(|&&c| c == b'\n').count())
        + 1;
    vol.append(LOG, format!("{id} {line}\n").as_bytes())
}

impl Ledger for TestLedger {
    fn switched(&mut self, vol: &mut dyn Volume, n: u64, next: &Generation) -> Result<(), String> {
        vol.write(GRANTS, grants_of(next).as_bytes())?;
        append_log(vol, &format!("{} current={n}", next.why))
    }

    fn collected(
        &mut self,
        vol: &mut dyn Volume,
        gens: &[u64],
        objects: usize,
    ) -> Result<(), String> {
        append_log(
            vol,
            &format!("gc removed {} generations, {objects} objects", gens.len()),
        )
    }
}

/// What a crash state must equal: the store's summary, the grants and the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub store: Summary,
    pub grants: String,
    pub log_lines: usize,
}

/// The store's invariants, and the switch's own: the grants are `current`'s and
/// the newest switch line names `current`.
pub fn state(vol: &mut dyn Volume, store: &Store, now: Time) -> Result<State, String> {
    let summary = store::check(vol, store, now)?;
    let live = store::generation(vol, summary.current)?;
    let grants = String::from_utf8(vol.read(GRANTS)?.ok_or("no grants")?).map_err(|_| "grants")?;
    if grants != grants_of(&live) {
        return Err(format!("the grants are not current {}'s", summary.current));
    }
    let log = String::from_utf8(vol.read(LOG)?.ok_or("no log")?).map_err(|_| "log")?;
    let newest = log
        .lines()
        .rev()
        .find_map(|l| l.rsplit_once(" current=").map(|(_, n)| n));
    let want = summary.current.to_string();
    // `good` moving (mark-good) writes no log line; every switch does.
    if newest != Some(want.as_str()) {
        return Err(format!(
            "the newest switch line says current={}, but current is {want}",
            newest.unwrap_or("none")
        ));
    }
    Ok(State {
        store: summary,
        grants,
        log_lines: log.lines().count(),
    })
}
