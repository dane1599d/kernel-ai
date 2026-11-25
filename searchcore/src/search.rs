//! [`Search`]: the index, opened from its file, walked forward through
//! `changes(since)` ([`walk`]), and queried.

mod walk;

pub use walk::{MAX_TRIES, Walked};

use crate::text::{self, Reply, Request};
use crate::{Access, Files, Models, Store};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use semindex::format::MAX_BATCH;
use semindex::index::FileEntry;
use semindex::search::MAX_HITS;
use semindex::{Embedder, Header, Index, Query, Record, Refused, Scope};

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
    /// Each folder's path as last listed whole: a changed folder still at its
    /// path was not moved, and is not listed again.
    folders: BTreeMap<u64, String>,
    /// Reads that failed, by object: after [`MAX_TRIES`] the file is given up.
    tries: BTreeMap<u64, u32>,
    /// `fs`'s fallback count at the last full reconcile; `None` before the first,
    /// which every start does.
    fallbacks: Option<u64>,
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
            folders: BTreeMap::new(),
            tries: BTreeMap::new(),
            fallbacks: None,
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

    /// Appends `records` as one batch closed at `generation`, then applies them.
    /// Records that the index would not take are a bug; the index is then read
    /// back from its file, which rebuilds it if it does not hold.
    fn commit(&mut self, records: Vec<Record>, generation: u64) -> Result<(), String> {
        let mut batch = semindex::Batch::new();
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

    /// Writes the index file anew from the entries it holds, closed at
    /// `generation`: the one way its mark goes back, after `fs` fell back to an
    /// earlier commit. Nothing is read or embedded again.
    fn rewrite(&mut self, generation: u64) -> Result<(), String> {
        let why = |why: Refused| String::from(why.why());
        let mut bytes = semindex::fresh(&self.index.header).map_err(why)?;
        let mut batch = semindex::Batch::new();
        for entry in self.index.files() {
            let records = entry_records(entry, &entry.path);
            if batch.len() + records.len() >= MAX_BATCH {
                bytes.extend(core::mem::take(&mut batch).commit(generation));
            }
            for record in &records {
                batch.push(record).map_err(why)?;
            }
        }
        bytes.extend(batch.commit(generation));
        self.store.replace(&bytes)?;
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

/// An indexed file's records at `path`, with the chunks it has: what a move
/// re-keys, with nothing read or embedded again.
fn entry_records(entry: &FileEntry, path: &str) -> Vec<Record> {
    let object = entry.object;
    let mut records = Vec::from([Record::File {
        object,
        path: String::from(path),
        generation: entry.generation,
        modified_ns: entry.modified_ns,
        length: entry.length,
    }]);
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
        for record in entry_records(entry, &entry.path) {
            // Records of an index that holds them already: none is refused.
            let _ = part.apply(record);
        }
    }
    part.generation = index.generation;
    Some(part)
}
