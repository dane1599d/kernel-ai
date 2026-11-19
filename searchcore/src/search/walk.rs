//! The walk: everything changed since the index's generation, in batches that
//! each commit whole.
//!
//! 1. Each changed file is read and indexed again.
//! 2. Each changed folder whose path is not the one it had when it was last
//!    listed whole (moved, made, or not yet seen since the agent started) is
//!    listed and reconciled: a file indexed at another path is re-keyed to its
//!    new one with the chunks it has, one changed since it was indexed or not
//!    indexed is read, one the scope does not index leaves, and what the index
//!    holds under the folder that the listing does not, leaves. A walk whose
//!    `fs` fell back since the last full one (and the first walk after every
//!    start) reconciles the whole home folder this way.
//! 3. For each changed folder, everything under a child of it that no longer
//!    exists leaves: a trashed or deleted object has no row of its own, and its
//!    folder's change is how the walk learns of it.
//!
//! The mark moves only once every batch has landed, so a walk cut short is done
//! again from where it began, and never past a change that could not be read
//! now: its old entry stays and the next walk tries again. After [`MAX_TRIES`]
//! failed reads a file is given up (its entry leaves and it is reported), so one
//! file that never reads cannot hold every later change back.

use super::{BATCH_FILES, Search, entry_records};
use crate::{Changed, Files, Models, Store};
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use semindex::format::MAX_BATCH;
use semindex::index::MAX_FILE;
use semindex::scope::{self, HOME};
use semindex::{Estimate, Index, Record};

/// Failed reads of one file before it is given up.
pub const MAX_TRIES: u32 = 3;
/// Folders whose listed paths are kept; past it the list starts again, so a
/// folder may be listed once more than it needs to be, never less.
const MAX_FOLDERS: usize = 4096;

/// What a walk did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Walked {
    /// The changes were indexed: the generation the index is now complete up to.
    Indexed {
        files: usize,
        removed: usize,
        /// Files not indexed: outside the scope, too large or not text.
        skipped: usize,
        /// Changes that could not be read now: their entries are kept as they
        /// were, and the mark stays below them, so the next walk tries again.
        failed: usize,
        /// Files given up after [`MAX_TRIES`] failed reads: out of the index until
        /// they change again.
        gave_up: Vec<String>,
        generation: u64,
    },
    /// The index embeds, and there is no room to map the model now: nothing was
    /// done, and the next walk tries again.
    NoRoom,
}

impl<F: Files, S: Store, M: Models> Search<F, S, M> {
    /// Indexes everything changed since the index's generation (see the module's
    /// docs).
    pub fn walk(&mut self, now_ns: u64) -> Result<Walked, String> {
        if !self.lease(now_ns)? {
            return Ok(Walked::NoRoom);
        }
        let since = self.index.generation;
        let changes = self.files.changes(since)?;
        let mut walk = Walk::new(since, &self.index);
        for row in changes.rows.iter().filter(|row| is_file(row)) {
            self.file(row, &mut walk)?;
        }
        let folders: Vec<(u64, &str, u64)> = changes
            .rows
            .iter()
            .filter(|row| row.stat.kind == fsproto::Kind::Dir)
            .filter_map(|row| Some((row.stat.object, row.path.as_deref()?, row.stat.changed)))
            .filter(|(_, path, _)| scope::clean(path) && scope::within(path, HOME))
            .collect();
        if self.fallbacks != Some(changes.fallbacks) {
            match self.reconcile(HOME, &mut walk)? {
                true => {
                    self.fallbacks = Some(changes.fallbacks);
                    for &(object, path, _) in &folders {
                        self.listed(object, path);
                    }
                }
                false => walk.failed(since + 1),
            }
        }
        for &(object, folder, changed) in &folders {
            if self.folders.get(&object).map(String::as_str) == Some(folder) {
                continue;
            }
            match self.reconcile(folder, &mut walk)? {
                true => self.listed(object, folder),
                false => walk.failed(changed),
            }
        }
        for &(_, folder, changed) in &folders {
            if !self.gone_from(folder, &mut walk)? {
                walk.failed(changed);
            }
        }
        // The last batch, closed at the mark: every change at or before it is in.
        // A change that could not be read holds the mark just below it.
        let top = changes.generation.max(since);
        let mark = match walk.failed_at {
            Some(first) => first.saturating_sub(1).clamp(since, top),
            None => top,
        };
        if !walk.pending.records.is_empty() || mark > since {
            self.commit(walk.pending.records, mark)?;
        }
        if self.index.header.embedder.is_some() {
            self.used_ns = now_ns;
        }
        Ok(Walked::Indexed {
            files: walk.done.files,
            removed: walk.done.removed,
            skipped: walk.done.skipped,
            failed: walk.done.failed,
            gave_up: walk.gave_up,
            generation: self.index.generation,
        })
    }

    /// Folder `object` is at `path`, listed whole.
    fn listed(&mut self, object: u64, path: &str) {
        if self.folders.len() >= MAX_FOLDERS {
            self.folders.clear();
        }
        self.folders.insert(object, String::from(path));
    }

    /// Adds one file's records to the batch, committing the batch first if they
    /// would not fit.
    fn push(&mut self, walk: &mut Walk, records: Vec<Record>) -> Result<(), String> {
        if walk.pending.full(records.len()) {
            self.commit(core::mem::take(&mut walk.pending.records), walk.since)?;
            walk.pending = Pending::new(&self.index);
        }
        walk.pending.add(records);
        Ok(())
    }

    /// Brings one changed file's entry up to date: whether it could be read (a
    /// file given up counts as read, so the mark may pass it).
    fn file(&mut self, row: &Changed, walk: &mut Walk) -> Result<bool, String> {
        let object = row.stat.object;
        walk.seen.insert(object);
        let records = match self.file_records(row, &walk.pending, &mut walk.done) {
            Ok(records) => {
                self.tries.remove(&object);
                records
            }
            Err(()) => {
                let tries = self.tries.entry(object).or_default();
                *tries += 1;
                if *tries < MAX_TRIES {
                    walk.failed(row.stat.changed);
                    return Ok(false);
                }
                self.tries.remove(&object);
                walk.gave_up.push(row.path.clone().unwrap_or_default());
                match walk.pending.indexed(object) {
                    true => {
                        walk.done.removed += 1;
                        Vec::from([Record::Remove { object }])
                    }
                    false => Vec::new(),
                }
            }
        };
        self.push(walk, records)?;
        Ok(true)
    }

    /// Lists `folder` and brings what the index holds under it in line with what
    /// is there: whether all of it could be read.
    fn reconcile(&mut self, folder: &str, walk: &mut Walk) -> Result<bool, String> {
        let Ok(listed) = self.files.files_under(folder) else {
            return Ok(false);
        };
        let mut read_all = true;
        let mut present = BTreeSet::new();
        let mut folders = Vec::new();
        for row in &listed {
            let object = row.stat.object;
            let Some(path) = row.path.as_deref() else {
                continue;
            };
            if !is_file(row) {
                folders.push((object, path));
                continue;
            }
            present.insert(object);
            if walk.seen.contains(&object) {
                continue;
            }
            // Indexed as this walk has left it so far, with the entry the index
            // holds for a re-key.
            let entry = self
                .index
                .file(object)
                .filter(|_| walk.pending.indexed(object));
            let records = match (self.scope.check(path), entry) {
                (Err(_), None) => continue,
                (Err(_), Some(_)) => {
                    walk.seen.insert(object);
                    walk.done.removed += 1;
                    Vec::from([Record::Remove { object }])
                }
                (Ok(()), Some(entry)) if entry.generation >= row.stat.changed => {
                    walk.seen.insert(object);
                    if walk.pending.path_of(object) == Some(path) {
                        continue;
                    }
                    let mut records = walk.pending.make_room(path, object);
                    records.extend(entry_records(entry, path));
                    records
                }
                (Ok(()), _) => {
                    read_all &= self.file(row, walk)?;
                    continue;
                }
            };
            self.push(walk, records)?;
        }
        // What the index holds under the folder and the folder does not.
        let gone: Vec<u64> = walk
            .pending
            .beneath(folder)
            .map(|(_, object)| object)
            .filter(|object| !present.contains(object))
            .collect();
        for object in gone {
            walk.done.removed += 1;
            self.push(walk, Vec::from([Record::Remove { object }]))?;
        }
        if read_all {
            for (object, path) in folders {
                self.listed(object, path);
            }
        }
        Ok(read_all)
    }

    /// Everything indexed under a child of `folder` that no longer exists leaves:
    /// whether every child could be looked at.
    fn gone_from(&mut self, folder: &str, walk: &mut Walk) -> Result<bool, String> {
        let children: BTreeSet<String> = walk
            .pending
            .beneath(folder)
            .filter_map(|(path, _)| child(folder, path))
            .collect();
        let mut looked = true;
        for child in children {
            match self.files.present(&child) {
                Ok(true) => {}
                Ok(false) => {
                    let gone: Vec<u64> = walk
                        .pending
                        .at_or_beneath(&child)
                        .map(|(_, object)| object)
                        .collect();
                    for object in gone {
                        walk.done.removed += 1;
                        self.push(walk, Vec::from([Record::Remove { object }]))?;
                    }
                }
                Err(_) => looked = false,
            }
        }
        Ok(looked)
    }

    /// The records that bring one changed file's entry up to date, or `Err` when
    /// its bytes could not be read: its entry is then kept as it is.
    fn file_records(
        &mut self,
        row: &Changed,
        pending: &Pending,
        done: &mut Done,
    ) -> Result<Vec<Record>, ()> {
        let object = row.stat.object;
        let indexed = pending.indexed(object);
        let remove = |done: &mut Done| match indexed {
            true => {
                done.removed += 1;
                Vec::from([Record::Remove { object }])
            }
            false => Vec::new(),
        };
        let Some(path) = &row.path else {
            return Ok(remove(done));
        };
        if self.scope.check(path).is_err() || row.stat.size > MAX_FILE {
            done.skipped += 1;
            return Ok(remove(done));
        }
        let Ok(bytes) = self.files.read(path, MAX_FILE) else {
            done.failed += 1;
            return Err(());
        };
        let prepared = semindex::prepare(
            &self.scope,
            path,
            object,
            row.stat.changed,
            row.stat.mtime,
            &bytes,
            &Estimate,
            self.embedder.as_deref_mut(),
        );
        let Ok(prepared) = prepared else {
            done.skipped += 1;
            return Ok(remove(done));
        };
        done.files += 1;
        let mut records = pending.make_room(path, object);
        records.extend(prepared.records);
        Ok(records)
    }
}

/// What a walk counted.
#[derive(Default)]
struct Done {
    files: usize,
    removed: usize,
    skipped: usize,
    failed: usize,
}

/// A walk in progress: where it began, what it did, the batch being built, the
/// objects it has dealt with, and the earliest change it could not read.
struct Walk {
    since: u64,
    done: Done,
    pending: Pending,
    seen: BTreeSet<u64>,
    gave_up: Vec<String>,
    failed_at: Option<u64>,
}

impl Walk {
    fn new(since: u64, index: &Index) -> Self {
        Self {
            since,
            done: Done::default(),
            pending: Pending::new(index),
            seen: BTreeSet::new(),
            gave_up: Vec::new(),
            failed_at: None,
        }
    }

    fn failed(&mut self, changed: u64) {
        self.failed_at = Some(self.failed_at.map_or(changed, |at| at.min(changed)));
    }
}

fn is_file(row: &Changed) -> bool {
    row.stat.kind == fsproto::Kind::File
}

/// `path`'s child of `folder`: `/home/user/a` for `/home/user/a/b/c.txt` under
/// `/home/user`; `None` for `folder` itself or a path not under it.
fn child(folder: &str, path: &str) -> Option<String> {
    let rest = path.strip_prefix(folder)?.strip_prefix('/')?;
    let name = rest.split('/').next()?;
    Some(format!("{folder}/{name}"))
}

/// A batch being built, and which path each object holds once it lands.
struct Pending {
    records: Vec<Record>,
    files: usize,
    /// Each path's object, as the index holds them with this batch applied.
    paths: BTreeMap<String, u64>,
    /// Objects the index holds, with this batch applied.
    objects: BTreeSet<u64>,
}

impl Pending {
    fn new(index: &Index) -> Self {
        Self {
            records: Vec::new(),
            files: 0,
            paths: index.files().map(|f| (f.path.clone(), f.object)).collect(),
            objects: index.files().map(|f| f.object).collect(),
        }
    }

    fn indexed(&self, object: u64) -> bool {
        self.objects.contains(&object)
    }

    /// The `Remove` of another file holding `path`, before `object` takes it (the
    /// old one deleted, a new one made in its place): one path is one file.
    fn make_room(&self, path: &str, object: u64) -> Vec<Record> {
        match self.paths.get(path) {
            Some(&holder) if holder != object => Vec::from([Record::Remove { object: holder }]),
            _ => Vec::new(),
        }
    }

    /// The indexed paths under `folder`, not `folder` itself, with their objects.
    fn beneath<'a>(&'a self, folder: &'a str) -> impl Iterator<Item = (&'a str, u64)> + 'a {
        self.paths
            .iter()
            .filter(move |(path, _)| path.as_str() != folder && scope::within(path, folder))
            .map(|(path, object)| (path.as_str(), *object))
    }

    /// The indexed paths at `path` or under it, with their objects.
    fn at_or_beneath<'a>(&'a self, path: &'a str) -> impl Iterator<Item = (&'a str, u64)> + 'a {
        self.paths
            .iter()
            .filter(move |(held, _)| scope::within(held, path))
            .map(|(held, object)| (held.as_str(), *object))
    }

    fn path_of(&self, object: u64) -> Option<&str> {
        self.paths
            .iter()
            .find(|(_, held)| **held == object)
            .map(|(path, _)| path.as_str())
    }

    /// Whether one more file of `frames` records would not fit this batch.
    fn full(&self, frames: usize) -> bool {
        self.files > 0 && (self.files >= BATCH_FILES || self.records.len() + frames >= MAX_BATCH)
    }

    fn add(&mut self, records: Vec<Record>) {
        for record in &records {
            match record {
                Record::File { object, path, .. } => {
                    self.paths.retain(|_, held| held != object);
                    self.paths.insert(path.clone(), *object);
                    self.objects.insert(*object);
                }
                Record::Remove { object } => {
                    self.paths.retain(|_, held| held != object);
                    self.objects.remove(object);
                }
                Record::Chunk { .. } => {}
            }
        }
        if !records.is_empty() {
            self.files += 1;
        }
        self.records.extend(records);
    }
}
