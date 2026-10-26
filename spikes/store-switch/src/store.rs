//! The package store and its switch (ADR 0011 §5), over a [`Disk`], and the
//! invariants a crash must never break.
//!
//! Files: `pkg/store/<sha256>` objects, renamed in only after their length and hash
//! match; `pkg/partial/<sha256>` downloads in progress; `pkg/gen/<n>` in `kai-gen 1`;
//! `pkg/current` and `pkg/good`; `pkg/failed`; and, in ADR 0011's layout,
//! `grants` (`kai-grants 1` lines) and `log` (`actlog 1` lines), apart from the
//! generations. Every file is written to `<path>.tmp` and renamed over `<path>`.
//! The store keeps no state of its own: each operation reads what it needs, so a
//! store mounted after a crash simply carries on.

use crate::Disk;
use crate::repo::{self, Package};

/// Where a switch's grants and log line live.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// ADR 0011 as accepted: `grants` and `log` are files of their own, written in
    /// the same ADR 0004 transaction as the generation and `current`.
    Separate,
    /// Design C's layout: grants and the log line live inside the generation, so
    /// renaming `current` is the whole switch.
    InGeneration,
}

/// Grants that no package gave, which a switch must keep.
const BASE_GRANTS: &str = "kai-grants 1\nrule user process_stop build-*\n";
/// Bytes a download moves in one piece; each piece is its own commit.
pub const PIECE: usize = 4 << 10;

/// How the repository's bytes are spoiled, for the tamper cases.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tamper {
    None,
    /// One object byte flipped: the hash no longer matches the index.
    FlipByte,
    /// The object's header names another version than its index entry.
    WrongHeader,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Install(&'static str, Tamper),
    Remove(&'static str),
    /// Back to the `n`th generation present (counted modulo how many there are).
    Rollback(usize),
    Gc,
    /// Probation ended well: `good` moves to `current`.
    MarkGood,
    /// A trap during probation: back to `good`, the new hashes go to `failed`.
    FailProbation,
}

/// Why nothing switched, in code's words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    AlreadyInstalled,
    NotInstalled,
    /// The downloaded object's SHA-256 is not its index entry's.
    BadHash,
    /// The object's own header differs from its index entry (cargo's rule).
    HeaderDiffers,
    NothingToDo,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub name: String,
    pub version: String,
    pub sha: String,
    pub explicit: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Generation {
    pub parent: u64,
    pub why: String,
    pub installed: Vec<Installed>,
    pub grants: Vec<String>,
}

impl Generation {
    fn render(&self, layout: Layout, log_line: &str) -> String {
        let mut text = format!("kai-gen 1\nparent {}\nwhy {}\n", self.parent, self.why);
        for p in &self.installed {
            let kind = if p.explicit { "explicit" } else { "dep" };
            text += &format!("pkg {} {} {} {kind}\n", p.name, p.version, p.sha);
        }
        if layout == Layout::InGeneration {
            for g in &self.grants {
                text += &format!("grant {g}\n");
            }
            text += &format!("log {log_line}\n");
        }
        text
    }

    fn parse(text: &str) -> Result<Generation, String> {
        let mut lines = text.lines();
        if lines.next() != Some("kai-gen 1") {
            return Err("a generation without its kai-gen 1 line".into());
        }
        let mut generation = Generation {
            parent: 0,
            why: String::new(),
            installed: Vec::new(),
            grants: Vec::new(),
        };
        for line in lines {
            let words: Vec<&str> = line.split(' ').collect();
            match words.as_slice() {
                ["parent", n] => generation.parent = n.parse().map_err(|_| "a bad parent")?,
                ["why", ..] => generation.why = line[4..].to_string(),
                ["pkg", name, version, sha, kind] => generation.installed.push(Installed {
                    name: name.to_string(),
                    version: version.to_string(),
                    sha: sha.to_string(),
                    explicit: *kind == "explicit",
                }),
                ["grant", ..] => generation.grants.push(line[6..].to_string()),
                ["log", ..] => {}
                _ => return Err(format!("a generation line that does not parse: {line:?}")),
            }
        }
        Ok(generation)
    }
}

fn package_grants(installed: &[Installed]) -> Vec<String> {
    let mut grants: Vec<String> = installed
        .iter()
        .filter_map(|p| repo::find(&p.name))
        .flat_map(|p| p.grants.iter().map(move |g| format!("pkg {} {g}", p.name)))
        .collect();
    grants.sort();
    grants
}

fn render_grants(installed: &[Installed]) -> String {
    let mut text = BASE_GRANTS.to_string();
    for g in package_grants(installed) {
        text += &g;
        text.push('\n');
    }
    text
}

fn as_installed(packages: &[&'static Package], explicit: &[&str]) -> Vec<Installed> {
    packages
        .iter()
        .map(|p| Installed {
            name: p.name.to_string(),
            version: p.version.to_string(),
            sha: repo::index_sha(p),
            explicit: explicit.contains(&p.name),
        })
        .collect()
}

#[derive(Clone, Copy, Debug)]
pub struct Store {
    pub layout: Layout,
    /// Generations GC keeps besides `current` and `good` (ADR 0011: the last 10;
    /// fewer here, so short sequences reach a GC that deletes).
    pub keep_last: usize,
    /// A deliberate fault, for the harness to catch: commit the generation, grants
    /// and log line, then `current` in a second transaction.
    pub split_switch: bool,
}

fn text(disk: &mut dyn Disk, path: &str) -> Result<Option<String>, String> {
    disk.read(path)?
        .map(|bytes| String::from_utf8(bytes).map_err(|_| format!("{path} is not text")))
        .transpose()
}

fn number(disk: &mut dyn Disk, path: &str) -> Result<u64, String> {
    text(disk, path)?
        .ok_or(format!("no {path}"))?
        .trim()
        .parse()
        .map_err(|_| format!("{path} holds no generation number"))
}

/// Writes `bytes` to `path` the safe way: a temporary file, renamed over it.
fn put(disk: &mut dyn Disk, path: &str, bytes: &[u8]) {
    let tmp = format!("{path}.tmp");
    disk.write(&tmp, bytes);
    disk.rename(&tmp, path);
}

pub fn generations(disk: &mut dyn Disk) -> Result<Vec<u64>, String> {
    let mut gens: Vec<u64> = disk
        .list("pkg/gen")?
        .iter()
        .filter_map(|name| name.parse().ok())
        .collect();
    gens.sort_unstable();
    Ok(gens)
}

pub fn generation(disk: &mut dyn Disk, n: u64) -> Result<Generation, String> {
    let text = text(disk, &format!("pkg/gen/{n}"))?.ok_or(format!("no generation {n}"))?;
    Generation::parse(&text)
}

impl Store {
    /// Generation 1, the image, with nothing installed.
    pub fn setup(&self, disk: &mut dyn Disk) {
        disk.begin();
        let first = Generation {
            parent: 0,
            why: "image".into(),
            installed: Vec::new(),
            grants: Vec::new(),
        };
        let line = "E 1 image current=1";
        put(
            disk,
            "pkg/gen/1",
            first.render(self.layout, line).as_bytes(),
        );
        if self.layout == Layout::Separate {
            put(disk, "grants", render_grants(&[]).as_bytes());
            put(disk, "log", format!("{line}\n").as_bytes());
        }
        put(disk, "pkg/current", b"1");
        put(disk, "pkg/good", b"1");
        disk.commit();
    }

    fn next_log_id(&self, disk: &mut dyn Disk) -> Result<usize, String> {
        Ok(match self.layout {
            Layout::Separate => text(disk, "log")?.unwrap_or_default().lines().count() + 1,
            Layout::InGeneration => generations(disk)?.last().copied().unwrap_or(0) as usize + 1,
        })
    }

    /// One switch: the new generation, its grants and log line, and `current`, in
    /// one transaction (begin to commit). On design C's primitives these are five
    /// renames (ADR 0011's layout) or two (design C's).
    fn switch(
        &self,
        disk: &mut dyn Disk,
        installed: Vec<Installed>,
        why: String,
        failed: &[String],
    ) -> Result<u64, String> {
        let current = number(disk, "pkg/current")?;
        let n = generations(disk)?.last().copied().unwrap_or(0) + 1;
        let line = format!("E {} {why} current={n}", self.next_log_id(disk)?);
        let new = Generation {
            parent: current,
            why,
            grants: package_grants(&installed),
            installed,
        };
        let failed_text = (!failed.is_empty())
            .then(|| -> Result<String, String> {
                let mut text = text(disk, "pkg/failed")?.unwrap_or_default();
                for sha in failed {
                    text += sha;
                    text.push('\n');
                }
                Ok(text)
            })
            .transpose()?;
        let log = match self.layout {
            Layout::Separate => Some(text(disk, "log")?.unwrap_or_default() + &line + "\n"),
            Layout::InGeneration => None,
        };
        disk.begin();
        put(
            disk,
            &format!("pkg/gen/{n}"),
            new.render(self.layout, &line).as_bytes(),
        );
        if let Some(log) = log {
            put(disk, "grants", render_grants(&new.installed).as_bytes());
            put(disk, "log", log.as_bytes());
        }
        if self.split_switch {
            // The deliberate fault: the switch as two transactions.
            disk.commit();
            disk.begin();
        }
        if let Some(failed) = failed_text {
            put(disk, "pkg/failed", failed.as_bytes());
        }
        put(disk, "pkg/current", n.to_string().as_bytes());
        disk.commit();
        Ok(n)
    }

    /// Fetches `package`'s object into the store unless it is there: piece by
    /// piece into `pkg/partial`, then checked and renamed in.
    fn fetch(&self, disk: &mut dyn Disk, package: &Package, tamper: Tamper) -> Result<(), Refused> {
        let mut sha = repo::index_sha(package);
        let path = format!("pkg/store/{sha}");
        if disk.verified(&path, &sha).unwrap_or(false) {
            return Ok(());
        }
        let mut bytes = repo::object(package);
        match tamper {
            Tamper::None => {}
            Tamper::FlipByte => *bytes.last_mut().expect("objects are not empty") ^= 1,
            Tamper::WrongHeader => {
                // An index entry whose hash matches the object it serves, but
                // whose name and version are not the object's own: the header
                // check, not the hash, must refuse it.
                let wrong = format!("kpkg 1 {} 9.9.9\n", package.name);
                bytes = [wrong.as_bytes(), &bytes[repo::header(package).len()..]].concat();
                sha = repo::sha(&bytes);
            }
        }
        let partial = format!("pkg/partial/{sha}");
        for end in (PIECE..bytes.len()).step_by(PIECE).chain([bytes.len()]) {
            disk.begin();
            disk.write(&partial, &bytes[..end]);
            disk.commit();
        }
        let got = disk.read(&partial).ok().flatten().unwrap_or_default();
        let refused = if repo::sha(&got) != sha {
            Some(Refused::BadHash)
        } else if !got.starts_with(repo::header(package).as_bytes()) {
            Some(Refused::HeaderDiffers)
        } else {
            None
        };
        disk.begin();
        if let Some(why) = refused {
            disk.remove(&partial);
            let failed = text(disk, "pkg/failed").ok().flatten().unwrap_or_default();
            put(
                disk,
                "pkg/failed",
                format!("{failed}{}\n", repo::sha(&got)).as_bytes(),
            );
            disk.commit();
            return Err(why);
        }
        disk.rename(&partial, &path);
        disk.commit();
        Ok(())
    }

    /// Runs one operation; Ok when it switched or collected, the refusal otherwise.
    pub fn run(&self, disk: &mut dyn Disk, op: Op) -> Result<Result<(), Refused>, String> {
        let current = number(disk, "pkg/current")?;
        let now = generation(disk, current)?;
        let explicit: Vec<String> = now
            .installed
            .iter()
            .filter(|p| p.explicit)
            .map(|p| p.name.clone())
            .collect();
        match op {
            Op::Install(name, tamper) => {
                if explicit.iter().any(|e| e == name) {
                    return Ok(Err(Refused::AlreadyInstalled));
                }
                let mut want: Vec<&str> = explicit.iter().map(String::as_str).collect();
                want.push(name);
                let packages = repo::closure(&want);
                for package in &packages {
                    let spoiled = if package.name == name {
                        tamper
                    } else {
                        Tamper::None
                    };
                    if let Err(why) = self.fetch(disk, package, spoiled) {
                        return Ok(Err(why));
                    }
                }
                let installed = as_installed(&packages, &want);
                self.switch(disk, installed, format!("pkg_install {name}"), &[])?;
            }
            Op::Remove(name) => {
                if !explicit.iter().any(|e| e == name) {
                    return Ok(Err(Refused::NotInstalled));
                }
                let want: Vec<&str> = explicit
                    .iter()
                    .map(String::as_str)
                    .filter(|e| *e != name)
                    .collect();
                let installed = as_installed(&repo::closure(&want), &want);
                self.switch(disk, installed, format!("pkg_remove {name}"), &[])?;
            }
            Op::Rollback(index) => {
                let kept = self.kept(disk)?;
                let choices: Vec<u64> = kept.into_iter().filter(|g| *g != current).collect();
                let Some(&to) = choices.get(index % choices.len().max(1)) else {
                    return Ok(Err(Refused::NothingToDo));
                };
                let installed = generation(disk, to)?.installed;
                self.switch(disk, installed, format!("pkg_rollback {to}"), &[])?;
            }
            Op::MarkGood => {
                if number(disk, "pkg/good")? == current {
                    return Ok(Err(Refused::NothingToDo));
                }
                disk.begin();
                put(disk, "pkg/good", current.to_string().as_bytes());
                disk.commit();
            }
            Op::FailProbation => {
                let good = number(disk, "pkg/good")?;
                if good == current {
                    return Ok(Err(Refused::NothingToDo));
                }
                let back = generation(disk, good)?.installed;
                let failed: Vec<String> = now
                    .installed
                    .iter()
                    .filter(|p| !back.iter().any(|b| b.sha == p.sha))
                    .map(|p| p.sha.clone())
                    .collect();
                self.switch(
                    disk,
                    back,
                    format!("probation failed, back to {good}"),
                    &failed,
                )?;
            }
            Op::Gc => {
                return self
                    .gc(disk)
                    .map(|removed| removed.then_some(()).ok_or(Refused::NothingToDo));
            }
        }
        Ok(Ok(()))
    }

    /// The generations GC keeps: the newest `keep_last`, `current` and `good`.
    pub fn kept(&self, disk: &mut dyn Disk) -> Result<Vec<u64>, String> {
        let gens = generations(disk)?;
        let mut kept: Vec<u64> = gens.iter().rev().take(self.keep_last).copied().collect();
        for path in ["pkg/current", "pkg/good"] {
            let n = number(disk, path)?;
            if !kept.contains(&n) {
                kept.push(n);
            }
        }
        kept.sort_unstable();
        Ok(kept)
    }

    /// Removes the generations GC does not keep, oldest first, then the objects no
    /// kept generation names, in one transaction; logs it. Whether it removed
    /// anything.
    fn gc(&self, disk: &mut dyn Disk) -> Result<bool, String> {
        let kept = self.kept(disk)?;
        let mut names: Vec<String> = Vec::new();
        for g in &kept {
            names.extend(generation(disk, *g)?.installed.into_iter().map(|p| p.sha));
        }
        let old: Vec<u64> = generations(disk)?
            .into_iter()
            .filter(|g| !kept.contains(g))
            .collect();
        let objects: Vec<String> = disk
            .list("pkg/store")?
            .into_iter()
            .filter(|o| !o.ends_with(".tmp") && !names.contains(o))
            .collect();
        if old.is_empty() && objects.is_empty() {
            return Ok(false);
        }
        let log = match self.layout {
            Layout::Separate => {
                let id = self.next_log_id(disk)?;
                let line = format!(
                    "S {id} gc removed {} generations, {} objects",
                    old.len(),
                    objects.len()
                );
                Some(text(disk, "log")?.unwrap_or_default() + &line + "\n")
            }
            Layout::InGeneration => None,
        };
        disk.begin();
        for g in &old {
            disk.remove(&format!("pkg/gen/{g}"));
        }
        for o in &objects {
            disk.remove(&format!("pkg/store/{o}"));
        }
        if let Some(log) = log {
            put(disk, "log", log.as_bytes());
        }
        disk.commit();
        Ok(true)
    }
}

/// What a state holds, to compare a crash state with the committed one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    pub current: u64,
    pub good: u64,
    pub generations: Vec<u64>,
    pub objects: Vec<String>,
    pub grants: String,
    pub log_lines: usize,
    pub failed: usize,
}

/// ADR 0011's invariants, as S11-2 states them, plus the switch's own: `current`
/// names a fully present, verified closure; no object reachable from a kept
/// generation is gone; no half-written object is visible; and the grants and the
/// newest log line agree with `current`.
pub fn check(disk: &mut dyn Disk, store: &Store) -> Result<Summary, String> {
    let current = number(disk, "pkg/current")?;
    let good = number(disk, "pkg/good")?;
    let now = generation(disk, current)?;
    for p in &now.installed {
        let package = repo::find(&p.name).ok_or(format!("current names unknown {}", p.name))?;
        for dep in package.depends {
            if !now.installed.iter().any(|q| q.name == *dep) {
                return Err(format!(
                    "current {current}: {} without its dependency {dep}",
                    p.name
                ));
            }
        }
    }
    for g in store.kept(disk)? {
        for p in generation(disk, g)?.installed {
            if !disk.verified(&format!("pkg/store/{}", p.sha), &p.sha)? {
                return Err(format!(
                    "kept generation {g} names {}, which is missing or wrong",
                    p.name
                ));
            }
        }
    }
    let mut objects = disk.list("pkg/store")?;
    objects.retain(|o| !o.ends_with(".tmp"));
    for o in &objects {
        if !disk.verified(&format!("pkg/store/{o}"), o)? {
            return Err(format!(
                "object {} is visible but does not hash to its name",
                &o[..12]
            ));
        }
    }
    let grants = match store.layout {
        Layout::Separate => {
            let on_disk = text(disk, "grants")?.ok_or("no grants")?;
            if on_disk != render_grants(&now.installed) {
                return Err(format!("the grants are not current {current}'s"));
            }
            on_disk
        }
        Layout::InGeneration => {
            if now.grants != package_grants(&now.installed) {
                return Err(format!(
                    "generation {current}'s grants are not its packages'"
                ));
            }
            now.grants.join("\n")
        }
    };
    let log_lines = match store.layout {
        Layout::Separate => {
            let log = text(disk, "log")?.ok_or("no log")?;
            let newest = log.lines().rev().find_map(|l| l.split(" current=").nth(1));
            if newest != Some(current.to_string().as_str()) {
                return Err(format!(
                    "the newest log line says current={}, but current is {current}",
                    newest.unwrap_or("none")
                ));
            }
            log.lines().count()
        }
        Layout::InGeneration => 0,
    };
    let failed = text(disk, "pkg/failed")?.unwrap_or_default();
    if !failed
        .lines()
        .all(|l| l.len() == 64 && l.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err("pkg/failed holds a line that is not a hash".into());
    }
    Ok(Summary {
        current,
        good,
        generations: generations(disk)?,
        objects,
        grants,
        log_lines,
        failed: failed.lines().count(),
    })
}
