//! The `search` service's loop (ADR 0013 section 4), with no OS in it. The OS half
//! (`user/src/bin/search`, the built-in agent `index`) supplies four parts behind
//! traits, and this crate decides everything else:
//!
//! - [`Files`]: what changed in `/home/user` since a generation, from kaifs's
//!   `changes(since)`, each changed object with the path it now has, and a file's
//!   bytes.
//! - [`Store`]: the index file on the `index` volume, `kai-index 1`. Each append is
//!   one kaifs transaction.
//! - [`Models`]: the embedding model, which takes 639 MB while it is mapped: whether
//!   there is room for it, and mapping it.
//! - [`Access`]: the caller's own `Dir`, asked about each result's path before the
//!   result is returned.
//!
//! What [`Search`] does with them:
//!
//! - **Open.** The index file is read with `semindex::open`. A torn tail is cut off
//!   and found again by the next walk; a file of another format or another embedder
//!   is rebuilt, never read ([`Opened`]).
//! - **Walk.** Every file `changes(since)` reports is read and indexed, in batches of
//!   at most [`BATCH_FILES`] files. A batch commits whole or not at all. Only the
//!   walk's last batch moves the index's generation mark on, so a crash in any batch
//!   leaves the mark where the walk began, and the next walk does those files again.
//!   Indexing a file again replaces it, so doing it twice is harmless. A changed
//!   folder with nothing indexed under it is listed, so a moved folder's files are
//!   re-keyed to their new paths without being read again; and for every changed
//!   folder, the indexed files under a child of it that no longer exists leave.
//!   A file that cannot be read now keeps its old entry, and the mark stays below
//!   its change, so the next walk tries it again.
//! - **Model.** A walk that would embed waits while there is no room for the model
//!   ([`Walked::NoRoom`]): indexing never pushes out the person's own requests. The
//!   model is dropped after [`IDLE_NS`] unused ([`Search::idle`]).
//! - **Query.** One request text in, one reply text out ([`text`]). Results come
//!   from `semindex::search` over the files the caller's `Dir` may read and no
//!   others: a file it may not read is never returned, takes no place under the
//!   limit, and plays no part in any score.
//!
//! The index is derived data: losing it loses nothing but time.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

mod search;
pub mod text;

#[cfg(test)]
mod tests;

pub use search::{BATCH_FILES, IDLE_NS, Opened, Search, Walked};

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use semindex::{Embedder, EmbedderId};

/// One object `changes(since)` reported, or a folder listed: its stat, as kaifs's
/// `Reply::Changes` row gives it, and its path now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Changed {
    pub stat: fsproto::Stat,
    /// Its absolute path now, or `None` when it is no longer under `/home/user`:
    /// it leaves the index.
    pub path: Option<String>,
}

/// What changed since a generation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// Every object changed after the generation asked about, files and
    /// folders. `fs` answers at most 64 rows a call, in `(changed, object)` order;
    /// the OS half pages with `ChangesAfter` until it has them all (#1148). A
    /// trashed or deleted object is in no row: the home folder's handle no longer
    /// sees it, and the change to its folder is how the walk learns of it.
    pub rows: Vec<Changed>,
    /// The generation this answer is complete up to: the next walk starts here.
    pub generation: u64,
}

/// The files of `/home/user`, as the `index` agent's `Dir` sees them. An `Err` is
/// a failure to read now: what it was about is kept and tried again.
pub trait Files {
    fn changes(&mut self, since: u64) -> Result<Changes, String>;
    /// The bytes of the file at `path`, at most `max` of them; more is an error.
    fn read(&mut self, path: &str, max: u64) -> Result<Vec<u8>, String>;
    /// Every file under `folder`, at any depth, with its stat and path: a folder
    /// moved in is listed so its files are re-keyed to their new paths.
    fn files_under(&mut self, folder: &str) -> Result<Vec<Changed>, String>;
    /// Whether anything, file or folder, is at `path` now.
    fn present(&mut self, path: &str) -> Result<bool, String>;
}

/// The index file on the `index` volume.
pub trait Store {
    /// The whole file, or nothing when there is none yet.
    fn load(&mut self) -> Result<Vec<u8>, String>;
    /// Cuts the file to `len` bytes: a torn tail goes.
    fn truncate(&mut self, len: usize) -> Result<(), String>;
    /// Appends `bytes` in one transaction: all of them land, or none.
    fn append(&mut self, bytes: &[u8]) -> Result<(), String>;
    /// Replaces the whole file with `bytes`: a rebuild.
    fn replace(&mut self, bytes: &[u8]) -> Result<(), String>;
}

/// The embedding model.
pub trait Models {
    /// The model the index's vectors should come from, read from its pin without
    /// mapping it; `None` for an index searched by keyword alone.
    fn id(&self) -> Option<EmbedderId>;
    /// Whether there is room to map the model now, beside the person's own.
    fn room(&self) -> bool;
    /// Maps the model. It is unmapped when the box is dropped.
    fn load(&mut self) -> Result<Box<dyn Embedder>, String>;
}

/// The caller's own `Dir`: whether it may read `path`.
pub trait Access {
    fn may_read(&mut self, path: &str) -> bool;
}
