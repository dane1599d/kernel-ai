//! P2 task 5's exit gate: the crash harness over namespace operations (ADR 0004;
//! P2 plan §6). A workload of file and folder operations runs on a recording
//! disk beside a model that knows only paths, bytes and attributes. Then every
//! crash state the enumerator rebuilds must mount, pass `check()` and the
//! namespace rules on the live tree and every snapshot, and hold exactly one
//! committed generation's namespace as the model had it, never older than the
//! last commit waited on.
//!
//! The operations: create, mkdir, whole-file writes from empty to 140 KiB, ranged
//! writes that leave holes, truncate, rename (of files and folders, across
//! folders, refused onto a taken name or below itself), remove (refused for a
//! full folder), set and remove an attribute, snapshot, drop, restore, commit.
//! Each runs on the filesystem and on the model, and both must agree on whether
//! it is refused. Names hash to 8 buckets, so collision chains are the rule.
//!
//! Workloads: every sequence of up to three operations, then a commit, on an
//! empty namespace, on a small one and on one of 2,000 files; and random batches
//! of 120 operations. S04-1's deliberate bugs must still each be caught here.
//!
//! Release builds run it all:
//! `cargo test --release -p kaifs --test ns_crash -- --nocapture --test-threads 2`

use kaifs::check::{Cache, check};
use kaifs::crash::{Options, Oracle, Recording, Rng, Tally, enumerate};
use kaifs::format;
use kaifs::fs::Faults;
use kaifs::ns_check::{Digest, check_tree};
use kaifs::{CsumKind, Error, FileType, Fs, Image, MemDisk, ROOT, RecordingDisk};
use std::collections::BTreeMap;
use std::time::Instant;

const UUID: [u8; 16] = *b"kaifs ns crashes";
/// Names keep 3 bits of their hash: 8 buckets a folder.
const MASK: u64 = 0x7;
const NAMES: [&str; 12] = [
    "a",
    "b",
    "c",
    "d",
    "notes",
    "photo.bin",
    "x",
    "y",
    "\u{65e5}\u{8a18}",
    "longer-name",
    "z1",
    "z2",
];

#[derive(Clone, Debug, PartialEq, Eq)]
enum Node {
    Dir,
    File(Vec<u8>),
}

/// A namespace as a reader sees it: paths, bytes, attributes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Ns {
    nodes: BTreeMap<String, Node>,
    attrs: BTreeMap<String, BTreeMap<String, Vec<u8>>>,
}

fn parent(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) => "/",
        Some(at) => &path[..at],
        None => unreachable!("paths start with /"),
    }
}

fn join(dir: &str, name: &str) -> String {
    match dir {
        "/" => format!("/{name}"),
        _ => format!("{dir}/{name}"),
    }
}

fn below(path: &str, top: &str) -> bool {
    path == top || path.starts_with(&format!("{top}/"))
}

impl Ns {
    fn digest(&self) -> Digest {
        let mut d = Digest::default();
        d.dir("/");
        for (path, node) in &self.nodes {
            match node {
                Node::Dir => d.dir(path),
                Node::File(data) => {
                    d.file(path, data.len() as u64);
                    d.bytes(path, data.len() as u64, 0, data);
                }
            }
        }
        for (path, attrs) in &self.attrs {
            for (name, value) in attrs {
                d.attr(path, name, value);
            }
        }
        d
    }

    fn is_dir(&self, path: &str) -> bool {
        path == "/" || self.nodes.get(path) == Some(&Node::Dir)
    }

    fn dirs(&self) -> Vec<String> {
        let mut out = vec!["/".to_string()];
        out.extend(
            self.nodes
                .iter()
                .filter(|(_, n)| **n == Node::Dir)
                .map(|(p, _)| p.clone()),
        );
        out
    }

    fn files(&self) -> Vec<String> {
        self.nodes
            .iter()
            .filter(|(_, n)| matches!(n, Node::File(_)))
            .map(|(p, _)| p.clone())
            .collect()
    }

    fn has_children(&self, path: &str) -> bool {
        self.nodes.keys().any(|p| p != path && below(p, path))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct State {
    live: Digest,
    snaps: BTreeMap<u64, Digest>,
}

#[derive(Clone, Debug, Default)]
struct Model {
    live: Ns,
    snaps: BTreeMap<u64, Ns>,
    next_snap: u64,
}

impl Model {
    fn state(&self) -> State {
        State {
            live: self.live.digest(),
            snaps: self
                .snaps
                .iter()
                .map(|(id, ns)| (*id, ns.digest()))
                .collect(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Create,
    Mkdir,
    WriteSmall,
    WriteBig,
    WriteAt,
    Truncate,
    Rename,
    Remove,
    SetXattr,
    RemoveXattr,
    Snapshot,
    DropSnapshot,
    Restore,
    Commit,
}

const ACTIONS: [Action; 14] = [
    Action::Create,
    Action::Mkdir,
    Action::WriteSmall,
    Action::WriteBig,
    Action::WriteAt,
    Action::Truncate,
    Action::Rename,
    Action::Remove,
    Action::SetXattr,
    Action::RemoveXattr,
    Action::Snapshot,
    Action::DropSnapshot,
    Action::Restore,
    Action::Commit,
];

/// How an operation came out, as both sides can say it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Done,
    NotFound,
    Exists,
    NotEmpty,
    Invalid,
    Other,
}

fn outcome<T>(r: Result<T, Error>) -> Outcome {
    match r {
        Ok(_) => Outcome::Done,
        Err(Error::NotFound) => Outcome::NotFound,
        Err(Error::Exists) => Outcome::Exists,
        Err(Error::NotEmpty) => Outcome::NotEmpty,
        Err(Error::Invalid(_)) => Outcome::Invalid,
        Err(_) => Outcome::Other,
    }
}

struct Run {
    fs: Fs<RecordingDisk>,
    model: Model,
    rng: Rng,
    expected: BTreeMap<u64, State>,
    waited: Vec<(usize, u64)>,
    base: MemDisk,
    faults: Faults,
}

fn content(rng: &mut Rng, len: usize) -> Vec<u8> {
    (0..len).map(|_| rng.next_u64() as u8).collect()
}

impl Run {
    fn setup(blocks: u64, files: u64, seed: u64) -> Run {
        let mut fs = Fs::mkfs(RecordingDisk::new(blocks), UUID, "ns crash gate").unwrap();
        fs.hash_mask = MASK;
        let mut run = Run {
            fs,
            model: Model {
                next_snap: 1,
                ..Model::default()
            },
            rng: Rng(seed),
            expected: BTreeMap::new(),
            waited: Vec::new(),
            base: MemDisk::default(),
            faults: Faults::default(),
        };
        // Folders of up to 40 files, two deep, so names collide in every bucket.
        let mut dir = "/".to_string();
        for n in 0..files {
            if n % 40 == 0 {
                let top = join("/", &format!("dir{}", n / 400));
                if !run.model.live.nodes.contains_key(&top) {
                    run.mkdir(&top);
                }
                dir = join(&top, &format!("sub{}", n / 40));
                run.mkdir(&dir);
            }
            let len = [0, 7, 300, 1023, 1024, 4096][run.rng.below(6) as usize];
            let data = content(&mut run.rng, len);
            run.write(&join(&dir, &format!("file{n}")), &data);
            if n % 1000 == 999 {
                run.fs.commit().unwrap();
            }
        }
        run.commit();
        run.checkpoint();
        run
    }

    fn ino(&mut self, path: &str) -> u64 {
        self.fs.resolve(path).unwrap()
    }

    fn mkdir(&mut self, path: &str) {
        let dir = self.ino(parent(path));
        self.fs
            .mkdir(dir, &path[path.rfind('/').unwrap() + 1..])
            .unwrap();
        self.model.live.nodes.insert(path.into(), Node::Dir);
    }

    fn write(&mut self, path: &str, data: &[u8]) {
        let dir = self.ino(parent(path));
        let name = &path[path.rfind('/').unwrap() + 1..];
        let ino = match self.fs.lookup(dir, name).unwrap() {
            Some(e) => e.ino,
            None => self.fs.create(dir, name).unwrap(),
        };
        self.fs.write_file(ino, data).unwrap();
        self.model
            .live
            .nodes
            .insert(path.into(), Node::File(data.to_vec()));
    }

    fn checkpoint(&mut self) {
        self.fs.io.dev.checkpoint();
        self.base = self.fs.io.dev.disk.clone();
        self.expected.clear();
        self.waited.clear();
        self.expected.insert(self.fs.committed, self.model.state());
        self.waited.push((0, self.fs.committed));
    }

    fn fork(&self, seed: u64, faults: Faults) -> Run {
        let mut fs = Fs::mount_with(self.fs.io.dev.clone(), faults).unwrap();
        fs.hash_mask = MASK;
        Run {
            fs,
            model: self.model.clone(),
            rng: Rng(seed),
            expected: self.expected.clone(),
            waited: self.waited.clone(),
            base: self.base.clone(),
            faults,
        }
    }

    fn pick(&mut self, from: &[String]) -> Option<String> {
        (!from.is_empty()).then(|| from[self.rng.below(from.len() as u64) as usize].clone())
    }

    fn name(&mut self) -> &'static str {
        NAMES[self.rng.below(NAMES.len() as u64) as usize]
    }

    fn any_node(&mut self) -> Option<String> {
        let all: Vec<String> = self.model.live.nodes.keys().cloned().collect();
        self.pick(&all)
    }

    fn commit(&mut self) {
        self.act(Action::Commit);
    }

    fn act(&mut self, action: Action) {
        if let Err(e) = self.try_act(action) {
            panic!("{action:?}: {e}");
        }
    }

    /// One action on both sides; Err if they disagree or the filesystem failed.
    fn try_act(&mut self, action: Action) -> Result<(), String> {
        let (got, want) = match action {
            Action::Create | Action::Mkdir => {
                let dirs = self.model.live.dirs();
                let dir = self.pick(&dirs).unwrap();
                let name = self.name();
                let path = join(&dir, name);
                let ino = self.ino(&dir);
                let (got, node) = match action {
                    Action::Create => (outcome(self.fs.create(ino, name)), Node::File(Vec::new())),
                    _ => (outcome(self.fs.mkdir(ino, name)), Node::Dir),
                };
                let want = match self.model.live.nodes.contains_key(&path) {
                    true => Outcome::Exists,
                    false => {
                        self.model.live.nodes.insert(path, node);
                        Outcome::Done
                    }
                };
                (got, want)
            }
            Action::WriteSmall | Action::WriteBig => {
                let files = self.model.live.files();
                let Some(path) = self.pick(&files) else {
                    return Ok(());
                };
                let len = match action {
                    Action::WriteSmall => [0, 1, 50, 200, 1023][self.rng.below(5) as usize],
                    _ => [1024, 4096, 5000, 140_000][self.rng.below(4) as usize],
                };
                let data = content(&mut self.rng, len);
                let ino = self.ino(&path);
                let got = outcome(self.fs.write_file(ino, &data));
                self.model.live.nodes.insert(path, Node::File(data));
                (got, Outcome::Done)
            }
            Action::WriteAt | Action::Truncate => {
                let files = self.model.live.files();
                let Some(path) = self.pick(&files) else {
                    return Ok(());
                };
                let Some(Node::File(bytes)) = self.model.live.nodes.get(&path).cloned() else {
                    unreachable!("a file")
                };
                let mut bytes = bytes;
                let ino = self.ino(&path);
                let got = match action {
                    Action::WriteAt => {
                        let off = self.rng.below(bytes.len() as u64 + 140_000) as usize;
                        let len = 1 + self.rng.below(6000) as usize;
                        let data = content(&mut self.rng, len);
                        if bytes.len() < off + data.len() {
                            bytes.resize(off + data.len(), 0);
                        }
                        bytes[off..off + data.len()].copy_from_slice(&data);
                        outcome(self.fs.write_at(ino, off as u64, &data))
                    }
                    _ => {
                        let size = self.rng.below(2 * bytes.len() as u64 + 1000) as usize;
                        bytes.resize(size, 0);
                        outcome(self.fs.truncate(ino, size as u64))
                    }
                };
                self.model.live.nodes.insert(path, Node::File(bytes));
                (got, Outcome::Done)
            }
            Action::Rename => {
                let Some(from) = self.any_node() else {
                    return Ok(());
                };
                let dirs = self.model.live.dirs();
                let to_dir = self.pick(&dirs).unwrap();
                let name = self.name();
                let to = join(&to_dir, name);
                let (fd, td) = (self.ino(parent(&from)), self.ino(&to_dir));
                let got =
                    outcome(
                        self.fs
                            .rename(fd, &from[from.rfind('/').unwrap() + 1..], td, name),
                    );
                let ns = &mut self.model.live;
                // The filesystem's order: a folder below itself, then a taken name.
                let want = if from == to {
                    Outcome::Done
                } else if ns.is_dir(&from) && below(&to_dir, &from) {
                    Outcome::Invalid
                } else if ns.nodes.contains_key(&to) {
                    Outcome::Exists
                } else {
                    let moved: Vec<String> = ns
                        .nodes
                        .keys()
                        .filter(|p| below(p, &from))
                        .cloned()
                        .collect();
                    for old in moved {
                        let new = format!("{to}{}", &old[from.len()..]);
                        let node = ns.nodes.remove(&old).unwrap();
                        ns.nodes.insert(new.clone(), node);
                        if let Some(attrs) = ns.attrs.remove(&old) {
                            ns.attrs.insert(new, attrs);
                        }
                    }
                    Outcome::Done
                };
                (got, want)
            }
            Action::Remove => {
                let Some(path) = self.any_node() else {
                    return Ok(());
                };
                let dir = self.ino(parent(&path));
                let got = outcome(self.fs.remove(dir, &path[path.rfind('/').unwrap() + 1..]));
                let ns = &mut self.model.live;
                let want = match ns.has_children(&path) {
                    true => Outcome::NotEmpty,
                    false => {
                        ns.nodes.remove(&path);
                        ns.attrs.remove(&path);
                        Outcome::Done
                    }
                };
                (got, want)
            }
            Action::SetXattr | Action::RemoveXattr => {
                let mut all = self.model.live.dirs();
                all.extend(self.model.live.nodes.keys().cloned());
                let path = self.pick(&all).unwrap();
                let name = ["user.a", "user.b", "user.mime"][self.rng.below(3) as usize];
                let ino = self.ino(&path);
                match action {
                    Action::SetXattr => {
                        let len = self.rng.below(60) as usize;
                        let value = content(&mut self.rng, len);
                        let got = outcome(self.fs.set_xattr(ino, name, &value));
                        let attrs = self.model.live.attrs.entry(path).or_default();
                        attrs.insert(name.into(), value);
                        (got, Outcome::Done)
                    }
                    _ => {
                        let got = self.fs.remove_xattr(ino, name);
                        let attrs = self.model.live.attrs.entry(path.clone()).or_default();
                        let had = attrs.remove(name).is_some();
                        if attrs.is_empty() {
                            self.model.live.attrs.remove(&path);
                        }
                        let got = match got {
                            Ok(true) => Outcome::Done,
                            Ok(false) => Outcome::NotFound,
                            Err(e) => outcome::<()>(Err(e)),
                        };
                        (
                            got,
                            if had {
                                Outcome::Done
                            } else {
                                Outcome::NotFound
                            },
                        )
                    }
                }
            }
            Action::Snapshot => {
                let id = self.fs.snapshot().map_err(|e| format!("{e:?}"))?;
                let want = self.model.next_snap;
                self.model.next_snap += 1;
                self.model.snaps.insert(want, self.model.live.clone());
                (
                    if id == want {
                        Outcome::Done
                    } else {
                        Outcome::Other
                    },
                    Outcome::Done,
                )
            }
            Action::DropSnapshot => {
                let Some(&id) = self.model.snaps.keys().next_back() else {
                    return Ok(());
                };
                self.model.snaps.remove(&id);
                (
                    outcome(
                        self.fs
                            .drop_snapshot(id)
                            .and_then(|b| b.then_some(()).ok_or(Error::NotFound)),
                    ),
                    Outcome::Done,
                )
            }
            Action::Restore => {
                let Some((&id, ns)) = self.model.snaps.iter().next_back() else {
                    return Ok(());
                };
                self.model.live = ns.clone();
                (
                    outcome(
                        self.fs
                            .restore(id)
                            .and_then(|b| b.then_some(()).ok_or(Error::NotFound)),
                    ),
                    Outcome::Done,
                )
            }
            Action::Commit => {
                let generation = self.fs.commit().map_err(|e| format!("commit: {e:?}"))?;
                self.expected.insert(generation, self.model.state());
                self.waited.push((self.fs.io.dev.log.len(), generation));
                (Outcome::Done, Outcome::Done)
            }
        };
        match got == want {
            true => Ok(()),
            false => Err(format!("the filesystem said {got:?}, the model {want:?}")),
        }
    }

    fn enumerate(&self, rng: &mut Rng) -> Tally {
        let blocks = self.fs.block_count();
        let slots: Vec<u64> = format::all_slots(blocks).collect();
        let tear = |addr: u64| slots.contains(&addr);
        let recording = Recording {
            base: &self.base,
            log: &self.fs.io.dev.log,
            waited: &self.waited,
            tear: &tear,
        };
        let mut oracle = NsOracle {
            expected: &self.expected,
            faults: self.faults,
            cache: Cache::default(),
        };
        enumerate(&recording, &Options::default(), &mut oracle, rng)
    }
}

/// Mounts each crash state as `Fs::mount` does, checks its blocks and its
/// namespace on every tree, through the memo, and compares the digests.
struct NsOracle<'a> {
    expected: &'a BTreeMap<u64, State>,
    faults: Faults,
    cache: Cache,
}

impl NsOracle<'_> {
    fn state(&mut self, image: &mut Image<'_>) -> Result<(u64, State), String> {
        let found = format::read_rings(image).map_err(|e| format!("rings: {e:?}"))?;
        let verifies = |image: &mut Image<'_>, ptr: &kaifs::BlockPtr| {
            ptr.is_null() || format::read_checked(image, CsumKind::Seahash64, ptr, "root").is_ok()
        };
        let sb = found
            .into_iter()
            .find(|sb| {
                !sb.volumes.is_null() && verifies(image, &sb.volumes) && verifies(image, &sb.alloc)
            })
            .ok_or("no superblock whose roots verify")?;
        let mut mounted = None;
        let report = match self.cache.mounted(&sb) {
            Some(report) => report,
            None => {
                let mut fs = Fs::mount_with(&mut *image, self.faults)
                    .map_err(|e| format!("mount: {e:?}"))?;
                fs.hash_mask = MASK;
                if fs.committed != sb.generation {
                    return Err(format!(
                        "mount took {}, not {}",
                        fs.committed, sb.generation
                    ));
                }
                let report = check(&mut fs, &mut self.cache).map_err(|e| format!("check: {e}"))?;
                mounted = Some(fs);
                report
            }
        };
        let trees: Vec<(Option<u64>, kaifs::check::TreeReport)> =
            core::iter::once((None, report.live))
                .chain(report.snaps.iter().map(|(id, t)| (Some(*id), *t)))
                .collect();
        let mut state = State {
            live: Digest::default(),
            snaps: BTreeMap::new(),
        };
        for (id, tree) in trees {
            let ns = match self
                .cache
                .namespace(&tree.root)
                .filter(|ns| ns.max_oid < tree.next_oid)
            {
                Some(ns) => ns,
                None => {
                    if mounted.is_none() {
                        let mut fs = Fs::mount_with(&mut *image, self.faults)
                            .map_err(|e| format!("mount: {e:?}"))?;
                        fs.hash_mask = MASK;
                        mounted = Some(fs);
                    }
                    let fs = mounted.as_mut().unwrap();
                    check_tree(fs, &tree.root, tree.next_oid, &mut self.cache)
                        .map_err(|e| format!("namespace of {id:?}: {e}"))?
                }
            };
            match id {
                None => state.live = ns.digest,
                Some(id) => {
                    state.snaps.insert(id, ns.digest);
                }
            }
        }
        Ok((report.generation, state))
    }
}

impl Oracle for NsOracle<'_> {
    fn judge(&mut self, image: &mut Image<'_>, min: u64) -> Result<(), String> {
        self.cache.taint(image.overlay.keys().copied());
        let (generation, got) = self.state(image)?;
        if generation < min {
            return Err(format!(
                "mounted generation {generation}, older than the waited-on {min}"
            ));
        }
        let want = self
            .expected
            .get(&generation)
            .ok_or(format!("generation {generation} was never committed"))?;
        match got == *want {
            true => Ok(()),
            false => Err(format!(
                "generation {generation}: {got:?}, expected {want:?}"
            )),
        }
    }

    fn durable(&mut self, written: &[u64]) {
        self.cache.forget(written);
    }
}

fn sequences(len: usize) -> Vec<Vec<Action>> {
    let mut out = vec![Vec::new()];
    let mut frontier = vec![Vec::new()];
    for _ in 0..len {
        let mut next = Vec::new();
        for seq in &frontier {
            for action in ACTIONS {
                let mut longer: Vec<Action> = seq.clone();
                longer.push(action);
                next.push(longer);
            }
        }
        out.extend(next.iter().cloned());
        frontier = next;
    }
    out
}

fn report(what: &str, tally: &Tally, start: Instant) {
    eprintln!(
        "kaifs ns crash: {what}: {} crash states, {} violations, {:.1} s",
        tally.states,
        tally.violations,
        start.elapsed().as_secs_f64()
    );
    for example in tally.examples.iter().take(3) {
        eprintln!("  violation: {example}");
    }
}

fn sweep(files: u64, blocks: u64, len: usize) -> Tally {
    let start = Instant::now();
    let setup = Run::setup(blocks, files, 0x5EED ^ files);
    let mut tally = Tally::default();
    let mut rng = Rng(42);
    for (i, seq) in sequences(len).iter().enumerate() {
        let mut run = setup.fork(1000 + i as u64, Faults::default());
        for action in seq {
            run.act(*action);
        }
        run.act(Action::Commit);
        tally.add(&run.enumerate(&mut rng));
    }
    report(
        &format!("{files:5} files, every sequence of {len}"),
        &tally,
        start,
    );
    tally
}

fn batch_action(pick: &mut Rng) -> Action {
    match pick.below(100) {
        0..=11 => Action::Create,
        12..=17 => Action::Mkdir,
        18..=35 => Action::WriteSmall,
        36..=43 => Action::WriteBig,
        44..=53 => Action::WriteAt,
        54..=59 => Action::Truncate,
        60..=69 => Action::Rename,
        70..=77 => Action::Remove,
        78..=82 => Action::SetXattr,
        83..=84 => Action::RemoveXattr,
        85..=86 => Action::Snapshot,
        87 => Action::DropSnapshot,
        88 => Action::Restore,
        _ => Action::Commit,
    }
}

fn batches(files: u64, blocks: u64, workloads: u64) -> Tally {
    let start = Instant::now();
    let setup = Run::setup(blocks, files, 0xBA7C4 ^ files);
    let mut tally = Tally::default();
    let mut rng = Rng(7);
    for w in 0..workloads {
        let mut run = setup.fork(5000 + w, Faults::default());
        let mut pick = Rng(77 + w);
        for _ in 0..120 {
            run.act(batch_action(&mut pick));
        }
        run.act(Action::Commit);
        tally.add(&run.enumerate(&mut rng));
    }
    report(
        &format!("{files:5} files, {workloads} batches of 120"),
        &tally,
        start,
    );
    tally
}

/// The exit gate: zero violations over the namespace operations' crash states.
#[test]
fn namespace_operations_survive_every_crash_state() {
    let debug = cfg!(debug_assertions);
    let mut total = Tally::default();
    total.add(&sweep(0, 4096, if debug { 2 } else { 3 }));
    total.add(&sweep(60, 8192, if debug { 1 } else { 3 }));
    if !debug {
        total.add(&sweep(2000, 16384, 2));
    }
    total.add(&batches(60, 16384, if debug { 2 } else { 60 }));
    if !debug {
        total.add(&batches(2000, 32768, 20));
    }
    eprintln!(
        "kaifs ns crash total: {} crash states, {} violations",
        total.states, total.violations
    );
    assert_eq!(total.violations, 0, "{:#?}", total.examples);
    if !debug {
        assert!(total.states >= 1_000_000, "only {} states", total.states);
    }
}

/// S04-1's deliberate bugs, under namespace operations.
#[test]
fn the_deliberate_bugs_are_caught_under_namespace_operations() {
    use Action::*;
    let seqs = [
        vec![WriteBig, Commit, Rename, Commit, Remove, Commit],
        vec![
            Snapshot, Commit, WriteBig, WriteAt, Commit, Truncate, Commit,
        ],
        vec![
            Snapshot,
            Commit,
            WriteBig,
            Rename,
            Restore,
            DropSnapshot,
            Commit,
            WriteBig,
            Create,
            Commit,
            WriteBig,
            Commit,
        ],
    ];
    for faults in [
        Faults {
            skip_data_flush: true,
            ..Faults::default()
        },
        Faults {
            reuse_now: true,
            ..Faults::default()
        },
        Faults {
            ignore_snapshots: true,
            ..Faults::default()
        },
        Faults {
            decide_at_commit: true,
            ..Faults::default()
        },
    ] {
        let start = Instant::now();
        let setup = Run::setup(8192, 60, 3);
        let mut tally = Tally::default();
        let mut rng = Rng(42);
        for (i, seq) in seqs.iter().enumerate() {
            let mut run = setup.fork(1 + i as u64, faults);
            for action in seq {
                if let Err(e) = run.try_act(*action) {
                    tally.violations += 1;
                    tally
                        .examples
                        .push(format!("the workload failed at {action:?}: {e}"));
                    break;
                }
            }
            tally.add(&run.enumerate(&mut rng));
        }
        report(&format!("{faults:?}"), &tally, start);
        assert!(tally.violations > 0, "{faults:?} went unnoticed");
    }
}

#[test]
fn a_folder_tree_of_the_setup_is_what_the_model_says() {
    let mut run = Run::setup(8192, 60, 9);
    let mut cache = Cache::default();
    check(&mut run.fs, &mut cache).unwrap();
    let ns = kaifs::ns_check::check_namespace(&mut run.fs, &mut cache).unwrap();
    assert_eq!(ns.live.digest, run.model.live.digest());
    assert_eq!(ns.live.files, 60);
    assert_eq!(run.fs.stat(ROOT).unwrap().kind, FileType::Dir);
}
