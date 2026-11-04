//! `kai-index 1`: what is written reads back the same; a crash's torn tail is
//! dropped at any byte and the next batch appends over it; anything else wrong
//! means rebuild.

use super::{CORPUS, Topics, corpus_index};
use crate::embed::{Embedder, EmbedderId};
use crate::format::{self, Batch, Header, MAGIC, Record, Refused};
use crate::index::ChunkEntry;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

fn keyword_only() -> Header {
    Header::default()
}

fn file(object: u64, path: &str, length: u64) -> Record {
    Record::File {
        object,
        path: String::from(path),
        generation: 1,
        modified_ns: 0,
        length,
    }
}

fn chunk(object: u64, offset: u64, len: u32, term: &str) -> Record {
    Record::Chunk {
        object,
        chunk: ChunkEntry {
            offset,
            len,
            terms: vec![(String::from(term), 1)],
            vector: None,
        },
    }
}

fn batch(records: &[Record], generation: u64) -> Vec<u8> {
    let mut batch = Batch::new();
    for record in records {
        batch.push(record).unwrap();
    }
    batch.commit(generation)
}

#[test]
fn the_corpus_reads_back_as_it_was_written_with_its_last_generation() {
    let (index, bytes) = corpus_index(None);
    assert!(bytes.starts_with(MAGIC));
    assert_eq!(index.files().count(), CORPUS.len());
    assert_eq!(index.generation, 10 + CORPUS.len() as u64 - 1);
    let trip = index.files().find(|f| f.path.ends_with("trip.md")).unwrap();
    assert_eq!(trip.chunks.len(), 1);
    assert!(trip.chunks[0].count("lisbon") == 1 && trip.chunks[0].count("hotel") == 1);
    // The whitespace-only file is indexed with no chunks.
    let empty = index
        .files()
        .find(|f| f.path.ends_with("empty.txt"))
        .unwrap();
    assert!(empty.chunks.is_empty());
    let opened = format::open(&bytes, &keyword_only()).unwrap();
    assert_eq!((opened.committed, opened.torn), (bytes.len(), false));
}

#[test]
fn a_crash_at_any_byte_of_the_last_batch_loses_that_batch_alone() {
    let mut bytes = format::fresh(&keyword_only()).unwrap();
    bytes.extend(batch(
        &[file(1, "/home/user/a.txt", 10), chunk(1, 0, 10, "alpha")],
        5,
    ));
    let first = bytes.len();
    let second = batch(&[file(2, "/home/user/b.txt", 8), chunk(2, 0, 8, "beta")], 6);
    for cut in 0..second.len() {
        let mut torn = bytes.clone();
        torn.extend_from_slice(&second[..cut]);
        let opened = format::open(&torn, &keyword_only()).unwrap();
        assert_eq!(opened.committed, first, "cut at {cut}");
        assert_eq!(opened.torn, cut > 0);
        assert_eq!(opened.index.generation, 5);
        assert_eq!(opened.index.files().count(), 1);
        // The writer appends at `committed`, over the torn tail.
        torn.truncate(opened.committed);
        torn.extend_from_slice(&second);
        let healed = format::open(&torn, &keyword_only()).unwrap();
        assert_eq!(
            (healed.index.generation, healed.index.files().count()),
            (6, 2)
        );
    }
}

#[test]
fn a_flipped_byte_in_the_last_batch_is_a_torn_tail() {
    let mut bytes = format::fresh(&keyword_only()).unwrap();
    bytes.extend(batch(&[file(1, "/home/user/a.txt", 10)], 5));
    let first = bytes.len();
    bytes.extend(batch(&[file(2, "/home/user/b.txt", 8)], 6));
    for at in first..bytes.len() {
        let mut bent = bytes.clone();
        bent[at] ^= 0x40;
        let opened = format::open(&bent, &keyword_only()).unwrap();
        assert_eq!(opened.committed, first, "flip at {at}");
        assert!(opened.torn);
    }
}

#[test]
fn a_file_replaced_or_removed_takes_its_old_chunks_with_it() {
    let mut bytes = format::fresh(&keyword_only()).unwrap();
    bytes.extend(batch(
        &[file(1, "/home/user/a.txt", 10), chunk(1, 0, 10, "alpha")],
        1,
    ));
    bytes.extend(batch(
        &[file(1, "/home/user/a.txt", 4), chunk(1, 0, 4, "gamma")],
        2,
    ));
    let index = format::open(&bytes, &keyword_only()).unwrap().index;
    let a = index.file(1).unwrap();
    assert_eq!(a.chunks.len(), 1);
    assert_eq!(
        (a.chunks[0].count("alpha"), a.chunks[0].count("gamma")),
        (0, 1)
    );
    bytes.extend(batch(&[Record::Remove { object: 1 }], 3));
    let index = format::open(&bytes, &keyword_only()).unwrap().index;
    assert!(index.file(1).is_none());
    assert_eq!(index.generation, 3);
}

#[test]
fn another_format_or_embedder_means_rebuild() {
    let (_, bytes) = corpus_index(None);
    let mut other = bytes.clone();
    other[MAGIC.len() - 2] = b'2';
    assert_eq!(format::open(&other, &keyword_only()), Err(Refused::Magic));
    assert_eq!(format::open(&[], &keyword_only()), Err(Refused::Magic));
    assert_eq!(format::open(MAGIC, &keyword_only()), Err(Refused::Header));
    let topics = Header {
        embedder: Some(Topics.id()),
    };
    assert_eq!(format::open(&bytes, &topics), Err(Refused::Embedder));
    let (_, with_vectors) = corpus_index(Some(&Topics));
    assert!(format::open(&with_vectors, &topics).is_ok());
    let newer = Header {
        embedder: Some(EmbedderId {
            sha256: [8; 32],
            ..Topics.id()
        }),
    };
    assert_eq!(format::open(&with_vectors, &newer), Err(Refused::Embedder));
}

#[test]
fn a_committed_record_that_does_not_hold_means_rebuild() {
    let base = format::fresh(&keyword_only()).unwrap();
    let cases: [(&[Record], &str); 3] = [
        (
            &[chunk(9, 0, 1, "alpha")],
            "a chunk of a file not in the index",
        ),
        (
            &[file(1, "/home/user/a.txt", 4), chunk(1, 2, 3, "alpha")],
            "a chunk past its file's end",
        ),
        (
            &[
                file(1, "/home/user/a.txt", 4),
                Record::Chunk {
                    object: 1,
                    chunk: ChunkEntry {
                        offset: 0,
                        len: 4,
                        terms: vec![],
                        vector: Some(vec![0; 4]),
                    },
                },
            ],
            "a vector the header does not promise",
        ),
    ];
    for (records, why) in cases {
        let mut bytes = base.clone();
        bytes.extend(batch(records, 1));
        assert_eq!(
            format::open(&bytes, &keyword_only()),
            Err(Refused::Corrupt(why))
        );
    }
    // A commit that goes back in generation.
    let mut bytes = base.clone();
    bytes.extend(batch(&[file(1, "/home/user/a.txt", 4)], 5));
    bytes.extend(batch(&[file(2, "/home/user/b.txt", 4)], 4));
    let refused = format::open(&bytes, &keyword_only());
    assert_eq!(
        refused,
        Err(Refused::Corrupt("a commit that goes back in generation"))
    );
}

#[test]
fn the_writer_refuses_what_the_reader_would_not_read() {
    let mut batch = Batch::new();
    for path in ["/home/user/../x", "relative.txt", "/home/user/a/"] {
        assert!(batch.push(&file(1, path, 1)).is_err(), "{path}");
    }
    let unsorted = Record::Chunk {
        object: 1,
        chunk: ChunkEntry {
            offset: 0,
            len: 1,
            terms: vec![(String::from("zeta"), 1), (String::from("alpha"), 1)],
            vector: None,
        },
    };
    assert!(batch.push(&unsorted).is_err());
    let empty_vector = Record::Chunk {
        object: 1,
        chunk: ChunkEntry {
            offset: 0,
            len: 1,
            terms: vec![],
            vector: Some(vec![]),
        },
    };
    assert!(batch.push(&empty_vector).is_err());
    assert!(batch.is_empty());
    let named = |name: &str, dims| Header {
        embedder: Some(EmbedderId {
            name: String::from(name),
            sha256: [1; 32],
            dims,
        }),
    };
    assert_eq!(format::fresh(&named("", 4)), Err(Refused::Header));
    assert_eq!(
        format::fresh(&named(&"n".repeat(65), 4)),
        Err(Refused::Header)
    );
    assert_eq!(format::fresh(&named("e", 0)), Err(Refused::Header));
    assert!(format::fresh(&named("e", 256)).is_ok());
}
