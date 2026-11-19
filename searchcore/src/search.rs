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
use semindex::index::MAX_FILE;
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
        /// Files not indexed: outside the scope, too large, not text, unreadable.
        skipped: usize,
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
    /// each commit whole. The generation mark moves only once every batch has
    /// landed, so a walk cut short is done again from where it began.
    pub fn walk(&mut self, now_ns: u64) -> Result<Walked, String> {
        if !self.lease(now_ns)? {
            return Ok(Walked::NoRoom);
        }
        let since = self.index.generation;
        let changes = self.files.changes(since)?;
        let rows: Vec<&Changed> = changes
            .rows
            .iter()
            .filter(|row| row.stat.kind == fsproto::Kind::File)
            .collect();
        let mut done = Done::default();
        let mut pending = Pending::new(&self.index);
        for row in rows {
            let records = self.records(row, &pending, &mut done);
            if pending.full(records.len()) {
                self.commit(core::mem::take(&mut pending.records), since)?;
                pending = Pending::new(&self.index);
            }
            pending.add(records);
        }
        // The last batch, closed at the walk's generation: the mark that says every
        // change before it is in. A walk with nothing new writes nothing.
        if !pending.records.is_empty() || changes.generation > since {
            self.commit(pending.records, changes.generation.max(since))?;
        }
        if self.index.header.embedder.is_some() {
            self.used_ns = now_ns;
        }
        Ok(Walked::Indexed {
            files: done.files,
            removed: done.removed,
            skipped: done.skipped,
            generation: self.index.generation,
        })
    }

    /// The records that bring one changed object's entry up to date.
    fn records(&mut self, row: &Changed, pending: &Pending, done: &mut Done) -> Vec<Record> {
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
            return remove(done);
        };
        let ready = self.scope.check(path).is_ok() && row.stat.size <= MAX_FILE;
        let bytes = match ready {
            true => self.files.read(path, MAX_FILE).ok(),
            false => None,
        };
        let prepared = bytes.and_then(|bytes| {
            semindex::prepare(
                &self.scope,
                path,
                object,
                row.stat.changed,
                row.stat.mtime,
                &bytes,
                &Estimate,
                self.embedder.as_deref_mut(),
            )
            .ok()
        });
        let Some(prepared) = prepared else {
            done.skipped += 1;
            return remove(done);
        };
        done.files += 1;
        // A file that took another's path (that one deleted, its new one made in
        // its place) puts the old one out first: one path is one file.
        let mut records = Vec::new();
        if let Some(holder) = pending.holder(path).filter(|holder| *holder != object) {
            records.push(Record::Remove { object: holder });
        }
        records.extend(prepared.records);
        records
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

    /// Answers one request with one reply, both as [`text`]. A result the caller's
    /// `Dir` may not read is never returned, nor counted against the limit.
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
            limit: MAX_HITS,
        };
        let found = semindex::search(
            &self.index,
            &self.scope,
            &query,
            self.embedder.as_deref_mut(),
        );
        let hits = match found {
            Ok(hits) => hits,
            Err(why) => return Reply::Err(String::from(why.why())).encode(),
        };
        let limit = request.limit.unwrap_or(MAX_HITS);
        let hits = hits
            .iter()
            .filter(|hit| access.may_read(&hit.path))
            .take(limit)
            .map(text::Hit::of)
            .collect();
        Reply::Hits(hits).encode()
    }
}

/// What a walk counted.
#[derive(Default)]
struct Done {
    files: usize,
    removed: usize,
    skipped: usize,
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
