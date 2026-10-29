//! The package store and its generations (ADR 0011 §5, §7), over a [`Volume`]:
//! the system volume's `pkg` folder.
//!
//! - `pkg/store/<sha256>.kpkg`: objects, immutable, named by their SHA-256. A
//!   download lands in `pkg/store/.partial/<sha256>`, a piece per commit, and is
//!   renamed in only once its length, its hash and its own header match the
//!   index entry.
//! - `pkg/gen/<n>`: generations in `kai-gen 1`. `pkg/current` and `pkg/good` hold
//!   generation numbers; `pkg/failed` the hashes of packages that failed their
//!   probation, one a line, never installed again without `force`.
//! - A switch writes the new generation, `current`, and through the [`Ledger`] its
//!   grants and its log line, in **one** transaction (S11-2's verdict). History
//!   only grows: a rollback writes a new generation copying an old one.
//! - GC keeps the newest [`Policy::keep_last`] generations, every one newer than
//!   [`Policy::keep_secs`], `current` and `good`, then removes the objects no kept
//!   generation names, in one transaction, and logs it.
//!
//! The store keeps no state of its own: each operation reads what it needs, so a
//! store mounted after a crash carries on.

use crate::bundle::{self, HEADER};
use crate::effects;
use crate::generation::{Generation, Installed, Time};
use crate::header::Header;
use crate::resolve::{self, Entry, Want};
use crate::version::Version;
use crate::{Sha, hex, sha256, unhex};
use alloc::string::String;
use alloc::vec::Vec;

/// What the store needs of a filesystem: kaifs's, through the `fs` server in the
/// OS and directly in the host tests. Paths are relative to the system volume's
/// root and use `/`. Everything between [`Volume::begin`] and [`Volume::commit`]
/// is one transaction; changes made without `begin` are committed by the next
/// `commit` as a plain one.
pub trait Volume {
    /// Opens the action `action` (ADR 0004 §4).
    fn begin(&mut self, action: u64) -> Result<(), String>;
    fn commit(&mut self) -> Result<(), String>;
    fn read(&mut self, path: &str) -> Result<Option<Vec<u8>>, String>;
    /// The names directly in folder `dir`; none when it does not exist.
    fn list(&mut self, dir: &str) -> Result<Vec<String>, String>;
    /// Replaces the whole file, making its folders.
    fn write(&mut self, path: &str, bytes: &[u8]) -> Result<(), String>;
    /// Adds `bytes` at the file's end, making it and its folders.
    fn append(&mut self, path: &str, bytes: &[u8]) -> Result<(), String>;
    /// Moves a file over `to`, replacing what was there.
    fn rename(&mut self, from: &str, to: &str) -> Result<(), String>;
    fn remove(&mut self, path: &str) -> Result<(), String>;
}

/// What a switch writes besides the store's own files, inside its transaction:
/// the grants of the generation becoming current (`kai-grants 1`) and the log
/// line (`actlog 1`), both P2 task 12's.
pub trait Ledger {
    fn switched(&mut self, vol: &mut dyn Volume, n: u64, next: &Generation) -> Result<(), String>;
    /// GC's log line, in GC's transaction.
    fn collected(
        &mut self,
        vol: &mut dyn Volume,
        gens: &[u64],
        objects: usize,
    ) -> Result<(), String>;
}

/// Where downloads come from: `Range` GETs of an index entry's object.
pub trait Source {
    /// Up to `len` bytes of `entry`'s object from `offset`; empty at its end.
    fn fetch(&mut self, entry: &Entry, offset: u64, len: usize) -> Result<Vec<u8>, String>;
}

/// An object already in memory, as the image's packages are.
struct Local<'a>(&'a [u8]);

impl Source for Local<'_> {
    fn fetch(&mut self, _: &Entry, offset: u64, len: usize) -> Result<Vec<u8>, String> {
        let start = (offset as usize).min(self.0.len());
        let end = start.saturating_add(len).min(self.0.len());
        Ok(self.0[start..end].to_vec())
    }
}

/// The largest piece a download asks for: a 192 KiB body keeps the whole
/// response under llm's 256 KiB limit (ADR 0011 §8).
pub const PIECE: usize = 192 << 10;

pub const CURRENT: &str = "pkg/current";
pub const GOOD: &str = "pkg/good";
pub const FAILED: &str = "pkg/failed";
pub const GENS: &str = "pkg/gen";
pub const OBJECTS: &str = "pkg/store";
pub const PARTIAL: &str = "pkg/store/.partial";

pub fn object_path(sha: &Sha) -> String {
    alloc::format!("{OBJECTS}/{}.kpkg", hex(sha))
}

fn partial_path(sha: &Sha) -> String {
    alloc::format!("{PARTIAL}/{}", hex(sha))
}

/// Why nothing switched, in code's words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    AlreadyInstalled(String),
    NotInstalled(String),
    /// Resolution's one sentence.
    Unresolved(String),
    /// The download's length or SHA-256 is not its index entry's.
    BadHash(String),
    /// The object's own header differs from its index entry (cargo's rule).
    HeaderDiffers(String),
    /// The app's artifact declares other effects than its index entry lists.
    EffectsDiffer(String),
    /// The package failed a probation before; `force` installs it anyway.
    Failed(String),
    NoGeneration(u64),
    NothingToDo,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Refused(Refused),
    /// The volume failed, or holds what the store never writes.
    Store(String),
}

impl From<String> for Error {
    fn from(why: String) -> Self {
        Error::Store(why)
    }
}

impl From<&str> for Error {
    fn from(why: &str) -> Self {
        Error::Store(why.into())
    }
}

fn refuse<T>(why: Refused) -> Result<T, Error> {
    Err(Error::Refused(why))
}

/// What GC keeps (§5).
#[derive(Clone, Copy, Debug)]
pub struct Policy {
    pub keep_last: usize,
    pub keep_secs: u64,
}

impl Default for Policy {
    /// The last 10 generations, and every one from the last 30 days.
    fn default() -> Self {
        Policy {
            keep_last: 10,
            keep_secs: 30 * 86_400,
        }
    }
}

/// What one operation needs besides the volume.
pub struct Ctx<'a> {
    pub vol: &'a mut dyn Volume,
    pub ledger: &'a mut dyn Ledger,
    pub now: Time,
    /// The action this operation's switch is (ADR 0002's log id).
    pub action: u64,
}

fn text(vol: &mut dyn Volume, path: &str) -> Result<Option<String>, String> {
    vol.read(path)?
        .map(|b| String::from_utf8(b).map_err(|_| alloc::format!("{path} is not text")))
        .transpose()
}

fn number(vol: &mut dyn Volume, path: &str) -> Result<u64, String> {
    text(vol, path)?
        .ok_or_else(|| alloc::format!("no {path}"))?
        .trim_end()
        .parse()
        .map_err(|_| alloc::format!("{path} holds no generation number"))
}

/// The generations present, oldest first.
pub fn generations(vol: &mut dyn Volume) -> Result<Vec<u64>, String> {
    let mut gens: Vec<u64> = Vec::new();
    for name in vol.list(GENS)? {
        let n: u64 = name
            .parse()
            .map_err(|_| alloc::format!("{GENS} holds {name:?}"))?;
        gens.push(n);
    }
    gens.sort_unstable();
    Ok(gens)
}

pub fn generation(vol: &mut dyn Volume, n: u64) -> Result<Generation, String> {
    let path = alloc::format!("{GENS}/{n}");
    Generation::parse(&text(vol, &path)?.ok_or_else(|| alloc::format!("no {path}"))?)
}

pub fn current(vol: &mut dyn Volume) -> Result<u64, String> {
    number(vol, CURRENT)
}

pub fn good(vol: &mut dyn Volume) -> Result<u64, String> {
    number(vol, GOOD)
}

fn failed(vol: &mut dyn Volume) -> Result<Vec<Sha>, String> {
    text(vol, FAILED)?
        .unwrap_or_default()
        .lines()
        .map(|l| unhex(l).ok_or_else(|| alloc::format!("{FAILED} holds {l:?}")))
        .collect()
}

/// Whether the object `sha` is in the store whole: its bytes hash to its name.
pub fn present(vol: &mut dyn Volume, sha: &Sha) -> Result<bool, String> {
    Ok(vol
        .read(&object_path(sha))?
        .is_some_and(|b| sha256(&b) == *sha))
}

/// An object's own header.
fn object_header(bytes: &[u8]) -> Result<Header, String> {
    let files = bundle::files(bytes)?;
    let (_, header) = files
        .first()
        .filter(|(p, _)| *p == HEADER)
        .ok_or("no header")?;
    let header = core::str::from_utf8(header).map_err(|_| "a header is not text")?;
    Header::parse(header)
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Store {
    pub policy: Policy,
}

impl Store {
    /// Generation 1, the image's packages, if the store holds no generation yet.
    /// `image` names objects already in the store.
    pub fn setup(&self, cx: &mut Ctx<'_>, image: Vec<Installed>) -> Result<bool, Error> {
        if cx.vol.read(CURRENT)?.is_some() {
            return Ok(false);
        }
        let first = Generation {
            parent: 0,
            time: cx.now,
            why: "image".into(),
            packages: image,
        };
        cx.vol.begin(cx.action)?;
        cx.vol
            .write(&alloc::format!("{GENS}/1"), first.format().as_bytes())?;
        cx.vol.write(CURRENT, b"1\n")?;
        cx.vol.write(GOOD, b"1\n")?;
        cx.ledger.switched(cx.vol, 1, &first)?;
        cx.vol.commit()?;
        Ok(true)
    }

    /// [`Store::setup`] from the image's checked packages ([`crate::image`]):
    /// each object is stored, checked against its entry as a download is, then
    /// generation 1 names them all, explicit, with their grants. Ok(false) when
    /// the store already has a generation; then nothing is stored.
    pub fn seed(&self, cx: &mut Ctx<'_>, image: &[(Entry, Vec<u8>)]) -> Result<bool, Error> {
        if cx.vol.read(CURRENT)?.is_some() {
            return Ok(false);
        }
        for (entry, bytes) in image {
            self.fetch(cx, &mut Local(bytes), entry)?;
        }
        let packages = image
            .iter()
            .map(|(entry, _)| Installed {
                name: entry.name.clone(),
                version: entry.version,
                sha: entry.sha,
                explicit: true,
                grants: entry.grants.clone(),
            })
            .collect();
        self.setup(cx, packages)
    }

    /// The one transaction of a switch (S11-2): the new generation, `current`,
    /// the grants and the log line, and with `failed` the hashes that failed.
    fn switch(
        &self,
        cx: &mut Ctx<'_>,
        packages: Vec<Installed>,
        why: String,
        failed_now: &[Sha],
    ) -> Result<u64, Error> {
        let parent = current(cx.vol)?;
        let n = generations(cx.vol)?.last().copied().unwrap_or(0) + 1;
        let next = Generation {
            parent,
            time: cx.now,
            why,
            packages,
        };
        let mut failed_text = text(cx.vol, FAILED)?.unwrap_or_default();
        for sha in failed_now {
            failed_text.push_str(&hex(sha));
            failed_text.push('\n');
        }
        cx.vol.begin(cx.action)?;
        cx.vol
            .write(&alloc::format!("{GENS}/{n}"), next.format().as_bytes())?;
        if !failed_now.is_empty() {
            cx.vol.write(FAILED, failed_text.as_bytes())?;
        }
        cx.vol.write(CURRENT, alloc::format!("{n}\n").as_bytes())?;
        cx.ledger.switched(cx.vol, n, &next)?;
        cx.vol.commit()?;
        Ok(n)
    }

    /// Fetches `entry`'s object unless it is stored: piece by piece into
    /// `.partial` (resuming what is there), then checked and renamed in.
    fn fetch(&self, cx: &mut Ctx<'_>, source: &mut dyn Source, entry: &Entry) -> Result<(), Error> {
        if present(cx.vol, &entry.sha)? {
            return Ok(());
        }
        let partial = partial_path(&entry.sha);
        let mut have = cx.vol.read(&partial)?.map_or(0, |b| b.len() as u64);
        while have < entry.size {
            let want = PIECE.min((entry.size - have) as usize);
            let piece = source.fetch(entry, have, want)?;
            if piece.is_empty() || piece.len() > want {
                break;
            }
            cx.vol.append(&partial, &piece)?;
            cx.vol.commit()?;
            have += piece.len() as u64;
        }
        let got = cx.vol.read(&partial)?.unwrap_or_default();
        let why = if got.len() as u64 != entry.size || sha256(&got) != entry.sha {
            Some(Refused::BadHash(entry.name.clone()))
        } else {
            match object_header(&got) {
                Ok(h)
                    if h.name == entry.name
                        && h.version == entry.version
                        && h.depends == entry.depends =>
                {
                    effects::check(&h, &got, &entry.effects)
                        .is_err()
                        .then(|| Refused::EffectsDiffer(entry.name.clone()))
                }
                _ => Some(Refused::HeaderDiffers(entry.name.clone())),
            }
        };
        if let Some(why) = why {
            cx.vol.remove(&partial)?;
            cx.vol.commit()?;
            return refuse(why);
        }
        cx.vol.rename(&partial, &object_path(&entry.sha))?;
        cx.vol.commit()?;
        Ok(())
    }

    /// Installs `wants` (with what is installed) from `index`, fetching what the
    /// store lacks, and switches. Ok is the new generation.
    #[allow(clippy::too_many_arguments)]
    pub fn install(
        &self,
        cx: &mut Ctx<'_>,
        source: &mut dyn Source,
        index: &[Entry],
        provided: &[(&str, Version)],
        wants: &[Want<'_>],
        force: bool,
    ) -> Result<u64, Error> {
        let cur = current(cx.vol)?;
        let now = generation(cx.vol, cur)?;
        let fresh: Vec<&Want<'_>> = wants
            .iter()
            .filter(|w| {
                !now.packages.iter().any(|p| {
                    p.explicit && p.name == w.name && w.range.is_none_or(|r| r.matches(&p.version))
                })
            })
            .collect();
        if fresh.is_empty() {
            let name = wants.first().map_or("", |w| w.name);
            return refuse(Refused::AlreadyInstalled(name.into()));
        }
        let mut all: Vec<Want<'_>> = now
            .packages
            .iter()
            .filter(|p| p.explicit && !fresh.iter().any(|w| w.name == p.name))
            .map(|p| Want {
                name: &p.name,
                range: None,
            })
            .collect();
        all.extend(fresh.iter().map(|w| Want {
            name: w.name,
            range: w.range,
        }));
        let picks = resolve::resolve(index, provided, &now.packages, &all)
            .map_err(|why| Error::Refused(Refused::Unresolved(why)))?;
        let bad = failed(cx.vol)?;
        if let Some(p) = picks
            .iter()
            .find(|p| !force && p.entry.is_some() && bad.contains(&p.sha))
        {
            return refuse(Refused::Failed(p.name.clone()));
        }
        for entry in picks.iter().filter_map(|p| p.entry.as_ref()) {
            self.fetch(cx, source, entry)?;
        }
        let packages = picks
            .into_iter()
            .map(|p| Installed {
                name: p.name,
                version: p.version,
                sha: p.sha,
                explicit: p.explicit,
                grants: p.grants,
            })
            .collect();
        let names: Vec<&str> = wants.iter().map(|w| w.name).collect();
        self.switch(
            cx,
            packages,
            alloc::format!("pkg_install {}", names.join(" ")),
            &[],
        )
    }

    /// Removes `name` and the dependencies nothing else needs, and switches.
    /// `provided` are the OS's `kai:*` packages, as for [`Store::install`].
    pub fn remove(
        &self,
        cx: &mut Ctx<'_>,
        index: &[Entry],
        provided: &[(&str, Version)],
        name: &str,
    ) -> Result<u64, Error> {
        let cur = current(cx.vol)?;
        let now = generation(cx.vol, cur)?;
        if !now.packages.iter().any(|p| p.explicit && p.name == name) {
            return refuse(Refused::NotInstalled(name.into()));
        }
        let keep: Vec<Want<'_>> = now
            .packages
            .iter()
            .filter(|p| p.explicit && p.name != name)
            .map(|p| Want {
                name: &p.name,
                range: None,
            })
            .collect();
        let picks = resolve::resolve(index, provided, &now.packages, &keep)
            .map_err(|why| Error::Refused(Refused::Unresolved(why)))?;
        // Only what was installed stays: removing never fetches.
        let packages = now
            .packages
            .iter()
            .filter(|p| picks.iter().any(|k| k.sha == p.sha))
            .cloned()
            .collect();
        self.switch(cx, packages, alloc::format!("pkg_remove {name}"), &[])
    }

    /// Writes generation n+1 as a copy of `to`'s packages: undoing it is another
    /// switch.
    pub fn rollback(&self, cx: &mut Ctx<'_>, to: u64) -> Result<u64, Error> {
        if to == current(cx.vol)? {
            return refuse(Refused::NothingToDo);
        }
        if !generations(cx.vol)?.contains(&to) {
            return refuse(Refused::NoGeneration(to));
        }
        let packages = generation(cx.vol, to)?.packages;
        self.switch(cx, packages, alloc::format!("pkg_rollback {to}"), &[])
    }

    /// Probation ended well: `good` moves to `current`.
    pub fn mark_good(&self, cx: &mut Ctx<'_>) -> Result<u64, Error> {
        let now = current(cx.vol)?;
        if good(cx.vol)? == now {
            return refuse(Refused::NothingToDo);
        }
        cx.vol.write(GOOD, alloc::format!("{now}\n").as_bytes())?;
        cx.vol.commit()?;
        Ok(now)
    }

    /// A trap, self-test or health check failed in probation: back to `good`,
    /// and the hashes `good` lacks go to `failed`.
    pub fn fail_probation(&self, cx: &mut Ctx<'_>) -> Result<u64, Error> {
        let (now, back) = (current(cx.vol)?, good(cx.vol)?);
        if back == now {
            return refuse(Refused::NothingToDo);
        }
        let back_packages = generation(cx.vol, back)?.packages;
        let failed_now: Vec<Sha> = generation(cx.vol, now)?
            .packages
            .iter()
            .filter(|p| !back_packages.iter().any(|b| b.sha == p.sha))
            .map(|p| p.sha)
            .collect();
        let why = alloc::format!("probation failed, back to {back}");
        self.switch(cx, back_packages, why, &failed_now)
    }

    /// The generations GC keeps at `now`.
    pub fn kept(&self, vol: &mut dyn Volume, now: Time) -> Result<Vec<u64>, String> {
        let gens = generations(vol)?;
        let mut kept: Vec<u64> = gens
            .iter()
            .rev()
            .take(self.policy.keep_last)
            .copied()
            .collect();
        for &g in &gens {
            if generation(vol, g)?
                .time
                .0
                .saturating_add(self.policy.keep_secs)
                >= now.0
            {
                kept.push(g);
            }
        }
        kept.push(current(vol)?);
        kept.push(good(vol)?);
        kept.sort_unstable();
        kept.dedup();
        Ok(kept)
    }

    /// Removes what GC does not keep, in one transaction, and logs it. Ok is how
    /// many generations and objects went.
    pub fn gc(&self, cx: &mut Ctx<'_>) -> Result<(usize, usize), Error> {
        let kept = self.kept(cx.vol, cx.now)?;
        let mut named: Vec<Sha> = Vec::new();
        for &g in &kept {
            named.extend(generation(cx.vol, g)?.packages.iter().map(|p| p.sha));
        }
        let old: Vec<u64> = generations(cx.vol)?
            .into_iter()
            .filter(|g| !kept.contains(g))
            .collect();
        let mut objects: Vec<String> = Vec::new();
        for name in cx.vol.list(OBJECTS)? {
            let sha = name.strip_suffix(".kpkg").and_then(unhex);
            if sha.is_some_and(|s| !named.contains(&s)) {
                objects.push(name);
            }
        }
        if old.is_empty() && objects.is_empty() {
            return refuse(Refused::NothingToDo);
        }
        cx.vol.begin(cx.action)?;
        for g in &old {
            cx.vol.remove(&alloc::format!("{GENS}/{g}"))?;
        }
        for o in &objects {
            cx.vol.remove(&alloc::format!("{OBJECTS}/{o}"))?;
        }
        cx.ledger.collected(cx.vol, &old, objects.len())?;
        cx.vol.commit()?;
        Ok((old.len(), objects.len()))
    }
}

/// What a store holds, to compare a crash state with the commit it names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    pub current: u64,
    pub good: u64,
    pub generations: Vec<u64>,
    pub objects: Vec<String>,
    pub failed: usize,
}

/// ADR 0011's invariants (S11-2): `current` names a closure whose objects are all
/// present and verified and whose dependencies (by their own headers) are in it;
/// no object a kept generation names is gone; no object in the store fails to
/// hash to its name; `pkg/failed` holds only hashes.
pub fn check(vol: &mut dyn Volume, store: &Store, now: Time) -> Result<Summary, String> {
    let (cur, good_n) = (current(vol)?, good(vol)?);
    let gens = generations(vol)?;
    for n in [cur, good_n] {
        if !gens.contains(&n) {
            return Err(alloc::format!("generation {n} is named but absent"));
        }
    }
    let live = generation(vol, cur)?;
    for p in &live.packages {
        let bytes = vol
            .read(&object_path(&p.sha))?
            .ok_or_else(|| alloc::format!("current {cur} names {}, which is not stored", p.name))?;
        let header = object_header(&bytes)?;
        for d in header
            .depends
            .iter()
            .filter(|d| !d.name.starts_with("kai:"))
        {
            if !live
                .packages
                .iter()
                .any(|q| q.name == d.name && d.range.matches(&q.version))
            {
                return Err(alloc::format!(
                    "current {cur}: {} without {}",
                    p.name,
                    d.name
                ));
            }
        }
    }
    for g in store.kept(vol, now)? {
        for p in generation(vol, g)?.packages {
            if !present(vol, &p.sha)? {
                return Err(alloc::format!(
                    "kept generation {g} names {}, which is gone",
                    p.name
                ));
            }
        }
    }
    let mut objects: Vec<String> = Vec::new();
    for name in vol.list(OBJECTS)? {
        if name == ".partial" {
            continue;
        }
        let sha = name
            .strip_suffix(".kpkg")
            .and_then(unhex)
            .ok_or_else(|| alloc::format!("{OBJECTS} holds {name:?}"))?;
        if !present(vol, &sha)? {
            return Err(alloc::format!(
                "object {} is visible but does not hash to its name",
                &name[..12]
            ));
        }
        objects.push(name);
    }
    objects.sort();
    Ok(Summary {
        current: cur,
        good: good_n,
        generations: gens,
        objects,
        failed: failed(vol)?.len(),
    })
}
