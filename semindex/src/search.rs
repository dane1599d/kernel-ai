//! A query over the index (ADR 0013 section 4): the keyword score, BM25 over
//! each chunk's terms, merged with a vector's cosine where the index and the
//! embedder have them, filtered by time ("last week" resolved by code,
//! [`crate::since`]) and by folder. Without vectors, keyword and time alone
//! decide: the ADR's fallback if S13-3 fails.
//!
//! A file is one result, at its best chunk, whose place in the file is the
//! snippet the service reads with the caller's own `Dir`: results are only
//! paths and places, and the service checks each against what the caller may
//! read before it returns it. A file in a folder excluded since it was
//! indexed is never a result.

use crate::embed::{self, Embedder};
use crate::half;
use crate::index::{ChunkEntry, Index};
use crate::scope::{self, HOME, Scope, Skip};
use crate::terms;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// Most results a query returns.
pub const MAX_HITS: usize = 20;
/// BM25's term-frequency saturation and length normalisation.
const K1: f32 = 1.2;
const B: f32 = 0.75;
/// How much of a merged score the keyword part is; the cosine is the rest.
pub const KEYWORD_WEIGHT: f32 = 0.5;
/// The least cosine a chunk with no keyword in it needs to be a result.
pub const MIN_COSINE: f32 = 0.3;

/// What is asked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Query<'a> {
    pub text: &'a str,
    /// Only files changed at or after this, in nanoseconds since the Unix
    /// epoch.
    pub since_ns: Option<u64>,
    /// Only files under this folder, which must be a clean path under
    /// `/home/user`.
    pub folder: Option<&'a str>,
    /// Most results, at most [`MAX_HITS`]; 0 means [`MAX_HITS`].
    pub limit: usize,
}

/// One file found, at its best chunk.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub object: u64,
    pub path: String,
    /// The best chunk's place in the file, in bytes.
    pub offset: u64,
    pub len: u32,
    pub modified_ns: u64,
    /// Higher is better, from 0 to 1.
    pub score: f32,
}

/// What the query brings to each chunk.
struct Asked {
    /// Its distinct terms, each with how many chunks hold it.
    terms: Vec<(String, usize)>,
    vector: Option<Vec<f32>>,
}

/// The files that best answer `query`, best first, ties by path. A folder that
/// is not clean or not under `/home/user` is refused. `embedder` is used only
/// if the index's vectors are its own; if it cannot embed the query, the
/// search is by keyword alone.
pub fn search(
    index: &Index,
    scope: &Scope,
    query: &Query<'_>,
    embedder: Option<&dyn Embedder>,
) -> Result<Vec<Hit>, Skip> {
    if let Some(folder) = query.folder {
        if !scope::clean(folder) {
            return Err(Skip::Unclean);
        }
        if !scope::within(folder, HOME) {
            return Err(Skip::Outside);
        }
    }
    let wanted = |path: &str, modified_ns: u64| {
        scope.check(path).is_ok()
            && query.folder.is_none_or(|f| scope::within(path, f))
            && query.since_ns.is_none_or(|since| modified_ns >= since)
    };
    let files: Vec<_> = index
        .files()
        .filter(|f| wanted(&f.path, f.modified_ns))
        .collect();
    let chunks = files.iter().map(|f| f.chunks.len()).sum::<usize>();
    if chunks == 0 {
        return Ok(Vec::new());
    }
    let asked = asked(
        index,
        query.text,
        embedder,
        files.iter().flat_map(|f| &f.chunks),
    );
    let average = files
        .iter()
        .flat_map(|f| &f.chunks)
        .map(|c| c.term_count() as f32)
        .sum::<f32>()
        / chunks as f32;
    // Each file's best chunk: its index in the file and its two scores.
    let mut best: Vec<(usize, usize, f32, Option<f32>)> = Vec::new();
    for (fi, file) in files.iter().enumerate() {
        let mut top: Option<(usize, f32, Option<f32>)> = None;
        for (ci, chunk) in file.chunks.iter().enumerate() {
            let keyword = bm25(chunk, &asked.terms, chunks, average);
            let cosine = cosine(chunk, asked.vector.as_deref());
            let found = keyword > 0.0 || cosine.is_some_and(|c| c >= MIN_COSINE);
            let better = top.is_none_or(|(_, k, c)| (keyword, cosine) > (k, c));
            if found && better {
                top = Some((ci, keyword, cosine));
            }
        }
        if let Some((ci, keyword, cosine)) = top {
            best.push((fi, ci, keyword, cosine));
        }
    }
    let most = best.iter().map(|b| b.2).fold(0.0f32, f32::max);
    let mut hits: Vec<Hit> = best
        .into_iter()
        .map(|(fi, ci, keyword, cosine)| {
            let file = files[fi];
            let chunk = &file.chunks[ci];
            let keyword = if most > 0.0 { keyword / most } else { 0.0 };
            let score = match cosine {
                Some(c) => KEYWORD_WEIGHT * keyword + (1.0 - KEYWORD_WEIGHT) * (c + 1.0) / 2.0,
                None => keyword,
            };
            Hit {
                object: file.object,
                path: file.path.clone(),
                offset: chunk.offset,
                len: chunk.len,
                modified_ns: file.modified_ns,
                score,
            }
        })
        .collect();
    hits.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.path.cmp(&b.path))
    });
    let limit = match query.limit {
        0 => MAX_HITS,
        n => n.min(MAX_HITS),
    };
    hits.truncate(limit);
    Ok(hits)
}

/// The query's terms with their chunk counts over `chunks`, and its vector if
/// `embedder` made the index's vectors and can embed it.
fn asked<'a>(
    index: &Index,
    text: &str,
    embedder: Option<&dyn Embedder>,
    chunks: impl Iterator<Item = &'a ChunkEntry>,
) -> Asked {
    let mut held: BTreeMap<String, usize> = terms::counted(text)
        .into_iter()
        .map(|(t, _)| (t, 0))
        .collect();
    for chunk in chunks {
        for (term, n) in held.iter_mut() {
            if chunk.count(term) > 0 {
                *n += 1;
            }
        }
    }
    let vector = embedder
        .filter(|e| index.header.embedder.as_ref() == Some(&e.id()))
        .and_then(|e| e.query(text).ok())
        .filter(|v| Some(v.len()) == index.header.embedder.as_ref().map(|e| usize::from(e.dims)))
        .and_then(|v| embed::unit(&v));
    Asked {
        terms: held.into_iter().collect(),
        vector,
    }
}

/// BM25 of `chunk` for the query's terms; `chunks` is how many are searched,
/// `average` their mean length in terms.
fn bm25(chunk: &ChunkEntry, terms: &[(String, usize)], chunks: usize, average: f32) -> f32 {
    let length = chunk.term_count() as f32;
    let norm = K1 * (1.0 - B + B * length / average.max(1.0));
    terms
        .iter()
        .filter(|(_, holding)| *holding > 0)
        .map(|(term, holding)| {
            let tf = f32::from(chunk.count(term));
            let n = *holding as f32;
            let idf = libm::logf(1.0 + (chunks as f32 - n + 0.5) / (n + 0.5));
            idf * tf * (K1 + 1.0) / (tf + norm)
        })
        .sum()
}

/// The cosine of `chunk`'s vector and the query's, when both have one.
fn cosine(chunk: &ChunkEntry, query: Option<&[f32]>) -> Option<f32> {
    let query = query?;
    let vector: Vec<f32> = chunk
        .vector
        .as_ref()?
        .iter()
        .map(|h| half::widen(*h))
        .collect();
    Some(embed::dot(&vector, query))
}
