//! A hostile `kai-index 1` file, as one read back from the `index` volume: cut,
//! mutated or bent at every byte, it is read or refused, never a panic, and
//! what is read is no more than the file's bytes could hold.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use semindex::format::{Batch, Header, Record, fresh, open};
use semindex::index::ChunkEntry;

fn sample() -> Vec<u8> {
    let mut bytes = fresh(&Header::default()).unwrap();
    let mut batch = Batch::new();
    let records = [
        Record::File {
            object: 7,
            path: String::from("/home/user/notes/trip.md"),
            generation: 3,
            modified_ns: 1_700_000_000_000_000_000,
            length: 64,
        },
        Record::Chunk {
            object: 7,
            chunk: ChunkEntry {
                offset: 0,
                len: 64,
                terms: vec![(String::from("hotel"), 1), (String::from("lisbon"), 2)],
                vector: None,
            },
        },
        Record::Remove { object: 9 },
    ];
    for record in &records {
        batch.push(record).unwrap();
    }
    bytes.extend(batch.commit(3));
    bytes
}

fn read(bytes: &[u8]) {
    let (opened, peak) = hostile::peak_alloc(|| open(bytes, &Header::default()));
    if let Ok(opened) = opened {
        assert!(opened.committed <= bytes.len());
        assert!(opened.index.chunks() <= bytes.len());
    }
    assert!(
        peak <= 16 * 1024 + 16 * bytes.len(),
        "{peak} bytes for {} bytes",
        bytes.len()
    );
}

#[test]
fn the_sample_reads() {
    let opened = open(&sample(), &Header::default()).unwrap();
    assert_eq!(opened.index.files().count(), 1);
    assert_eq!(opened.index.chunks(), 1);
    assert!(!opened.torn);
}

#[test]
fn mutated_files_are_read_or_refused() {
    let sample = sample();
    hostile::fuzz(&sample, 13, 30_000, read);
    hostile::truncations(&sample, |_, cut| read(cut));
}
