//! The index as the service holds it: each indexed file, by its kaifs object
//! id, with its path, generation, modification time and chunks. A file is
//! changed by replacing it whole, so its old chunks never mix with new ones.
//!
//! [`prepare`] turns a file into the records a batch carries, and
//! [`Index::apply`] takes each record, the same whether it comes from a batch
//! just written or from the file read back at start, so the two never differ.

use crate::chunk::{self, Tokens};
use crate::embed::{self, Embedder};
use crate::format::{Header, Record};
use crate::scope::{self, Scope, Skip};
use crate::{half, terms};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// The largest file indexed, in bytes.
pub const MAX_FILE: u64 = 4 << 20;

/// One chunk of a file: where it is, its terms and, with an embedder, its
/// vector as unit-length f16 bits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChunkEntry {
    pub offset: u64,
    pub len: u32,
    /// Its distinct terms with their counts, sorted by term.
    pub terms: Vec<(String, u16)>,
    pub vector: Option<Vec<u16>>,
}

impl ChunkEntry {
    /// The chunk's length in terms, for the keyword score.
    pub fn term_count(&self) -> u32 {
        self.terms.iter().map(|(_, n)| u32::from(*n)).sum()
    }

    /// How often `term` occurs in the chunk.
    pub fn count(&self, term: &str) -> u16 {
        self.terms
            .binary_search_by(|(t, _)| t.as_str().cmp(term))
            .map_or(0, |at| self.terms[at].1)
    }
}

/// One indexed file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEntry {
    /// Its kaifs object id: a rename keeps it, so a renamed file is found by
    /// its new path once `changes(since)` says it moved.
    pub object: u64,
    pub path: String,
    /// The kaifs generation the file was read at.
    pub generation: u64,
    /// When it was last changed, in nanoseconds since the Unix epoch.
    pub modified_ns: u64,
    pub length: u64,
    pub chunks: Vec<ChunkEntry>,
}

/// The files indexed, and the generation mark of the last batch committed.
#[derive(Clone, Debug, PartialEq)]
pub struct Index {
    pub header: Header,
    files: BTreeMap<u64, FileEntry>,
    /// Each indexed path's object: one path is one file.
    paths: BTreeMap<String, u64>,
    /// The kaifs generation the index is complete up to: the next
    /// `changes(since)` starts here.
    pub generation: u64,
}

impl Index {
    pub fn new(header: Header) -> Self {
        Self {
            header,
            files: BTreeMap::new(),
            paths: BTreeMap::new(),
            generation: 0,
        }
    }

    pub fn files(&self) -> impl Iterator<Item = &FileEntry> {
        self.files.values()
    }

    pub fn file(&self, object: u64) -> Option<&FileEntry> {
        self.files.get(&object)
    }

    /// Chunks in all files.
    pub fn chunks(&self) -> usize {
        self.files.values().map(|f| f.chunks.len()).sum()
    }

    /// Takes one record, or says why it does not hold: a path that is not
    /// clean or that another file holds, a chunk of a file not indexed, one
    /// past the file's end, or a vector the header does not promise or with a
    /// value that is not finite. A file moved to a path another file held
    /// comes after that file's `Remove`, as `changes(since)` reports them.
    pub fn apply(&mut self, record: Record) -> Result<(), &'static str> {
        match record {
            Record::File {
                object,
                path,
                generation,
                modified_ns,
                length,
            } => {
                if !scope::clean(&path) {
                    return Err("a file's path is not clean");
                }
                if self.paths.get(&path).is_some_and(|o| *o != object) {
                    return Err("two files with one path");
                }
                if let Some(old) = self.files.get(&object) {
                    self.paths.remove(&old.path);
                }
                self.paths.insert(path.clone(), object);
                let entry = FileEntry {
                    object,
                    path,
                    generation,
                    modified_ns,
                    length,
                    chunks: Vec::new(),
                };
                self.files.insert(object, entry);
            }
            Record::Chunk { object, chunk } => {
                let dims = self.header.embedder.as_ref().map_or(0, |e| e.dims);
                let file = self
                    .files
                    .get_mut(&object)
                    .ok_or("a chunk of a file not in the index")?;
                let end = chunk.offset.checked_add(u64::from(chunk.len));
                if end.is_none_or(|end| end > file.length) {
                    return Err("a chunk past its file's end");
                }
                match &chunk.vector {
                    Some(v) if v.len() != usize::from(dims) || dims == 0 => {
                        return Err("a vector the header does not promise");
                    }
                    // An f16 with every exponent bit set is an infinity or NaN.
                    Some(v) if v.iter().any(|h| h & 0x7c00 == 0x7c00) => {
                        return Err("a vector with a value that is not finite");
                    }
                    _ => {}
                }
                file.chunks.push(chunk);
            }
            Record::Remove { object } => {
                if let Some(old) = self.files.remove(&object) {
                    self.paths.remove(&old.path);
                }
            }
        }
        Ok(())
    }
}

/// A file made ready to index: the records its batch carries, and how many of
/// its chunks the embedder gave no vector for (those are found by keyword).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepared {
    pub records: Vec<Record>,
    pub unembedded: usize,
}

/// The records for file `object` at `path`, read at `generation` with
/// `bytes` as its contents: the file, then each chunk with its terms and, with
/// `embedder`, its vector. A path [`Scope`] does not index, a file over
/// [`MAX_FILE`] or one that is not text is skipped, and the caller removes any
/// older entry for it.
#[allow(clippy::too_many_arguments)]
pub fn prepare(
    scope: &Scope,
    path: &str,
    object: u64,
    generation: u64,
    modified_ns: u64,
    bytes: &[u8],
    tokens: &dyn Tokens,
    mut embedder: Option<&mut (dyn Embedder + '_)>,
) -> Result<Prepared, Skip> {
    scope.check(path)?;
    if bytes.len() as u64 > MAX_FILE {
        return Err(Skip::TooLarge);
    }
    let text = scope::text(bytes)?;
    let mut records = alloc::vec![Record::File {
        object,
        path: String::from(path),
        generation,
        modified_ns,
        length: bytes.len() as u64,
    }];
    let dims = embedder.as_ref().map_or(0, |e| usize::from(e.id().dims));
    let mut out = alloc::vec![0.0f32; dims];
    let mut unembedded = 0;
    for c in chunk::chunk(text, tokens) {
        let words = c.of(text);
        let vector = match embedder.as_deref_mut() {
            Some(e) if dims > 0 => {
                out.fill(0.0);
                e.embed_document(words, &mut out)
                    .ok()
                    .and_then(|()| embed::unit(&out))
                    .map(|v| v.into_iter().map(half::narrow).collect())
            }
            _ => None,
        };
        if embedder.is_some() && vector.is_none() {
            unembedded += 1;
        }
        records.push(Record::Chunk {
            object,
            chunk: ChunkEntry {
                offset: c.offset as u64,
                len: c.len as u32,
                terms: terms::counted(words),
                vector,
            },
        });
    }
    Ok(Prepared {
        records,
        unembedded,
    })
}
