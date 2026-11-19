//! [`Search`]: the index, opened from its file, walked forward through
//! `changes(since)`, and queried.

use crate::text::{self, Reply, Request};
use crate::{Access, Changed, Files, Models, Store};
use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use semindex::format::MAX_BATCH;
use semindex::index::FileEntry;
use semindex::index::MAX_FILE;
use semindex::scope;
use semindex::search::MAX_HITS;
use semindex::{Batch, Embedder, Estimate, Header, Index, Query, Record, Refused, Scope};

/// Most files in one batch: each batch is one kaifs transaction, and a crash loses
/// at most one.
pub const BATCH_FILES: usize = 16;
/// How long the embedding model stays mapped unused: 5 minutes (ADR 0013 question
/// 2's default).
pub const IDLE_NS: u64 = 5 * 60 * 1_000_000_000;

/// What opening the index file found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Opened {
    /// There was none: a new one was written.
    New,
    /// It was read; `torn` when a batch a crash cut short was cut off its end.
    Read { torn: bool },
    /// It was of another format or embedder, or did not hold: a new one replaced it.
    Rebuilt(Refused),
}

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
        generation: u64,
    },
    /// The index embeds, and there is no room to map the model now: nothing was
    /// done, and the next walk tries again.
    NoRoom,
}

/// The `search` service's state.
pub struct Search<F, S, M> {
    files: F,
    store: S,
    models: M,
    scope: Scope,
    index: Index,
    embedder: Option<Box<dyn Embedder>>,
    /// When the model was last used, for [`Search::idle`].
    used_ns: u64,
}

impl<F: Files, S: Store, M: Models> Search<F, S, M> {
    /// Opens the index in `store` for the model `models` names, rebuilding it when
    /// it cannot be read as one.
    pub fn open(files: F, store: S, models: M, scope: Scope) -> Result<(Self, Opened), String> {
        let header = Header {
            embedder: models.id(),
        };
        let mut search = Self {
            files,
            store,
            models,
            scope,
            index: Index::new(header),
            embedder: None,
            used_ns: 0,
        };
        let opened = search.load()?;
        Ok((search, opened))
    }

    pub fn index(&self) -> &Index {
        &self.index
    }

    pub fn store(&self) -> &S {
        &self.store
    }

    pub fn store_mut(&mut self) -> &mut S {
        &mut self.store
    }

    pub fn files_mut(&mut self) -> &mut F {
        &mut self.files
    }

    pub fn models_mut(&mut self) -> &mut M {
        &mut self.models
    }

    /// Whether the embedding model is mapped.
    pub fn holds_model(&self) -> bool {
        self.embedder.is_some()
    }

    /// Reads the index file, or writes a new one.
    fn load(&mut self) -> Result<Opened, String> {
        let want = self.index.header.clone();
        let bytes = self.store.load()?;
        let read = match bytes.is_empty() {
            true => None,
            false => Some(semindex::open(&bytes, &want)),
        };
        match read {
            Some(Ok(opened)) => {
                if opened.torn {
                    self.store.truncate(opened.committed)?;
                }
                self.index = opened.index;
                Ok(Opened::Read { torn: opened.torn })
            }
            Some(Err(why)) => {
                self.fresh(want)?;
                Ok(Opened::Rebuilt(why))
            }
            None => {
                self.fresh(want)?;
                Ok(Opened::New)
            }
        }
    }

    fn fresh(&mut self, header: Header) -> Result<(), String> {
        let bytes = semindex::fresh(&header).map_err(|why| String::from(why.why()))?;
        self.store.replace(&bytes)?;
        self.index = Index::new(header);
        Ok(())
    }

    /// Maps the model if the index embeds and it is not mapped: whether it may be
    /// used now. With no room, it is not mapped.
    fn lease(&mut self, now_ns: u64) -> Result<bool, String> {
        let Some(want) = &self.index.header.embedder else {
            return Ok(true);
        };
        if self.embedder.is_none() {
            if !self.models.room() {
                return Ok(false);
            }
            let model = self.models.load()?;
            if model.id() != *want {
                return Err(String::from(
                    "the model mapped is not the one the index was made with",
                ));
            }
            self.embedder = Some(model);
        }
        self.used_ns = now_ns;
        Ok(true)
    }

    /// Unmaps the model once it has gone [`IDLE_NS`] unused: whether it was.
    pub fn idle(&mut self, now_ns: u64) -> bool {
        let idle = self.embedder.is_some() && now_ns.saturating_sub(self.used_ns) >= IDLE_NS;
        if idle {
            self.embedder = None;
        }
        idle
    }

    /// Indexes everything changed since the index's generation, in batches that
    /// each commit whole: the changed files first, then each changed folder (a
    /// folder moved in is listed and its files re-keyed to their new paths, and
    /// what is no longer under a changed folder leaves the index). The generation
    /// mark moves only once every batch has landed, so a walk cut short is done
    /// again from where it began; and never past a change it could not read, so
    /// that change is tried again on the next walk while its old entry stays.
    pub fn walk(&mut self, now_ns: u64) -> Result<Walked, String> {
        if !self.lease(now_ns)? {
            return Ok(Walked::NoRoom);
        }
        let since = self.index.generation;
        let changes = self.files.changes(since)?;
        let mut walk = Walk {
            since,
            done: Done::default(),
            pending: Pending::new(&self.index),
            failed_at: None,
        };
        for row in changes.rows.iter().filter(|row| is_file(row)) {
            match self.file_records(row, &walk.pending, &mut walk.done) {
                Ok(records) => self.push(&mut walk, records)?,
                Err(()) => walk.failed(row.stat.changed),
            }
        }
        let folders: Vec<(&str, u64)> = changes
            .rows
            .iter()
            .filter(|row| row.stat.kind == fsproto::Kind::Dir)
            .filter_map(|row| Some((row.path.as_deref()?, row.stat.changed)))
            .filter(|(path, _)| scope::clean(path) && scope::within(path, scope::HOME))
            .collect();
        for &(folder, changed) in &folders {
            if !self.moved_in(folder, &mut walk)? {
                walk.failed(changed);
            }
        }
        for &(folder, changed) in &folders {
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
            generation: self.index.generation,
        })
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

    /// A changed folder with nothing indexed under it: a folder moved or copied
    /// in, or one that was excluded. Its files are listed; one indexed at another
    /// path is re-keyed to its new one without being read again, one not indexed
    /// is read and indexed, and one the scope does not index leaves. Whether
    /// everything could be read.
    fn moved_in(&mut self, folder: &str, walk: &mut Walk) -> Result<bool, String> {
        if walk.pending.beneath(folder).next().is_some() {
            return Ok(true);
        }
        let Ok(listed) = self.files.files_under(folder) else {
            return Ok(false);
        };
        let mut read_all = true;
        for row in listed.iter().filter(|row| is_file(row)) {
            let object = row.stat.object;
            let Some(path) = row.path.as_deref() else {
                continue;
            };
            if walk.pending.path_of(object) == Some(path) {
                continue;
            }
            // Indexed as this walk has left it so far, with the entry the index
            // holds for a re-key.
            let entry = self
                .index
                .file(object)
                .filter(|_| walk.pending.indexed(object));
            let records = match (self.scope.check(path), entry) {
                (Ok(()), Some(entry)) => rekey(entry, path, &walk.pending),
                (Err(_), Some(_)) => {
                    walk.done.removed += 1;
                    Vec::from([Record::Remove { object }])
                }
                (Err(_), None) => continue,
                (Ok(()), None) => match self.file_records(row, &walk.pending, &mut walk.done) {
                    Ok(records) => records,
                    Err(()) => {
                        read_all = false;
                        continue;
                    }
                },
            };
            self.push(walk, records)?;
        }
        Ok(read_all)
    }

    /// A changed folder: every indexed file under a child of it that no longer
    /// exists leaves the index. A trashed or deleted file or folder is reported by
    /// no row of its own (the home folder's handle no longer sees it); its
    /// folder's change is how the walk learns of it. Whether every child could be
    /// looked at.
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
                        .beneath_or_at(&child)
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
        // A file that took another's path (that one deleted, its new one made in
        // its place) puts the old one out first: one path is one file.
        let mut records = Vec::new();
        if let Some(holder) = pending.holder(path).filter(|holder| *holder != object) {
            records.push(Record::Remove { object: holder });
        }
        records.extend(prepared.records);
        Ok(records)
    }

    /// Appends `records` as one batch closed at `generation`, then applies them.
    /// Records that the index would not take are a bug; the index is then read
    /// back from its file, which rebuilds it if it does not hold.
    fn commit(&mut self, records: Vec<Record>, generation: u64) -> Result<(), String> {
        let mut batch = Batch::new();
        for record in &records {
            batch.push(record).map_err(|why| String::from(why.why()))?;
        }
        self.store.append(&batch.commit(generation))?;
        for record in records {
            if let Err(why) = self.index.apply(record) {
                self.load()?;
                return Err(format!("the index did not take a batch: {why}"));
            }
        }
        self.index.generation = generation;
        Ok(())
    }

    /// Answers one request with one reply, both as [`text`]. Only the files the
    /// caller's `Dir` may read are searched: one it may not read is never
    /// returned, takes no place under the limit, and plays no part in any score.
    pub fn query(&mut self, request: &str, now_ns: u64, access: &mut dyn Access) -> String {
        let Some(request) = Request::parse(request) else {
            return Reply::Err(String::from("not a search 1 request")).encode();
        };
        // The model embeds the query if it can be mapped; else it is by keyword.
        if let Err(why) = self.lease(now_ns) {
            return Reply::Err(why).encode();
        }
        let query = Query {
            text: &request.query,
            since_ns: request.since_ns,
            folder: request.folder.as_deref(),
            limit: request.limit.unwrap_or(MAX_HITS),
        };
        // Only what the caller may read is ranked: a file it may not read takes
        // no place among the results and no part in any score.
        let visible = visible(&self.index, access);
        let found = semindex::search(
            visible.as_ref().unwrap_or(&self.index),
            &self.scope,
            &query,
            self.embedder.as_deref_mut(),
        );
        let hits = match found {
            Ok(hits) => hits.iter().map(text::Hit::of).collect(),
            Err(why) => return Reply::Err(String::from(why.why())).encode(),
        };
        Reply::Hits(hits).encode()
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

/// A walk in progress: where it began, what it did, the batch being built, and
/// the earliest change it could not read.
struct Walk {
    since: u64,
    done: Done,
    pending: Pending,
    failed_at: Option<u64>,
}

impl Walk {
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

/// An indexed file at its new path, with the chunks it has: a move, not a change,
/// so nothing is read or embedded again.
fn rekey(entry: &FileEntry, path: &str, pending: &Pending) -> Vec<Record> {
    let object = entry.object;
    let mut records = Vec::new();
    if let Some(holder) = pending.holder(path).filter(|holder| *holder != object) {
        records.push(Record::Remove { object: holder });
    }
    records.push(Record::File {
        object,
        path: String::from(path),
        generation: entry.generation,
        modified_ns: entry.modified_ns,
        length: entry.length,
    });
    records.extend(entry.chunks.iter().map(|chunk| Record::Chunk {
        object,
        chunk: chunk.clone(),
    }));
    records
}

/// The part of `index` the caller may read, or `None` when that is all of it.
fn visible(index: &Index, access: &mut dyn Access) -> Option<Index> {
    let readable: Vec<&FileEntry> = index.files().filter(|f| access.may_read(&f.path)).collect();
    if readable.len() == index.files().count() {
        return None;
    }
    let mut part = Index::new(index.header.clone());
    for entry in readable {
        for record in rekey(entry, &entry.path, &Pending::empty()) {
            // Records of an index that holds them already: none is refused.
            let _ = part.apply(record);
        }
    }
    part.generation = index.generation;
    Some(part)
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

    fn empty() -> Self {
        Self {
            records: Vec::new(),
            files: 0,
            paths: BTreeMap::new(),
            objects: BTreeSet::new(),
        }
    }

    fn indexed(&self, object: u64) -> bool {
        self.objects.contains(&object)
    }

    /// The indexed paths under `folder`, not `folder` itself, with their objects.
    fn beneath<'a>(&'a self, folder: &'a str) -> impl Iterator<Item = (&'a str, u64)> + 'a {
        self.paths
            .iter()
            .filter(move |(path, _)| path.as_str() != folder && scope::within(path, folder))
            .map(|(path, object)| (path.as_str(), *object))
    }

    /// The indexed paths at `path` or under it, with their objects.
    fn beneath_or_at<'a>(&'a self, path: &'a str) -> impl Iterator<Item = (&'a str, u64)> + 'a {
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

    fn holder(&self, path: &str) -> Option<u64> {
        self.paths.get(path).copied()
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
