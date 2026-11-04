//! The `search` service's index (ADR 0013 section 4): files in `/home/user`,
//! cut into chunks, kept in `kai-index 1` on the `index` volume, and searched.
//!
//! - [`scope`]: what is indexed. Text under `/home/user` only, never `system`
//!   or `models`, and never a folder the person excludes (question 3's
//!   default). Every path is checked whole, so `..` and the like never reach
//!   past the home folder.
//! - [`chunk`]: a file in chunks of about 400 tokens, 40 of them shared with
//!   the chunk before, cut at whitespace, never inside a character.
//! - [`format`]: `kai-index 1`, a versioned header and then checksummed frames
//!   in batches, each closed by a commit with the index's generation mark. A
//!   torn tail, which a crash leaves, is dropped and found again through
//!   `changes(since)`; an index of another format or embedder is rebuilt,
//!   never read.
//! - [`search`]: the keyword score (BM25 over each chunk's terms), merged with
//!   a vector's cosine where the index has them, filtered by time and folder.
//!   Keyword search with time filters is the ADR's fallback if S13-3 fails, so
//!   it stands on its own.
//! - [`since`]: "last week" and the like, resolved by code from the clock,
//!   never by the model.
//! - [`embed`]: the vector slot, a trait S13-3's embedder fills; vectors are
//!   kept as f16 ([`half`]).
//!
//! The index is derived data: it can always be thrown away and rebuilt. What it
//! holds is read back as hostile, and nothing here holds a handle: the service
//! checks each result's path with the caller's own `Dir` before it returns it.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod chunk;
pub mod embed;
pub mod format;
pub mod half;
pub mod index;
pub mod scope;
pub mod search;
pub mod since;
pub mod terms;

#[cfg(test)]
mod tests;

pub use chunk::{Chunk, Estimate, Tokens, chunk};
pub use embed::{Embedder, EmbedderId};
pub use format::{Batch, Header, Opened, Record, Refused, fresh, open};
pub use index::{ChunkEntry, FileEntry, Index, Prepared, prepare};
pub use scope::{Scope, Skip};
pub use search::{Hit, Query, search};
