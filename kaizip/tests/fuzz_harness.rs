//! The `zip_archive` fuzz target's body (`fuzz/zip_harness.rs`) over hostile
//! mutations and truncations of good archives, on the host: it returns for
//! every input and keeps its promises, before libFuzzer runs it on GitHub
//! (`.github/workflows/fuzz.yml`).

#[path = "../../testkit/hostile.rs"]
mod hostile;

#[path = "../../fuzz/zip_harness.rs"]
mod harness;

mod kit;

use kaizip::{Limits, Maker};
use kit::Raw;
use std::io::Cursor;

/// Mutations of each sample; the fuzz workflow goes much further.
const CASES: usize = 3_000;

#[test]
fn the_zip_harness_returns_for_mutated_archives() {
    let mut maker = Maker::new(Cursor::new(Vec::new()), Limits::default());
    maker.folder("docs").unwrap();
    maker
        .file("docs/a.txt", &mut &b"some text, some text, some text"[..])
        .unwrap();
    maker.file("b.bin", &mut &[0u8, 1, 2, 3, 255][..]).unwrap();
    let made = maker.finish().unwrap().into_inner();
    let raw = kit::archive(&[
        Raw::stored("x/y.txt", b"stored bytes"),
        Raw::deflated("z.txt", &b"deflated ".repeat(50)),
        Raw::folder("x/"),
    ]);
    for (seed, sample) in [(0x21_9a01, &made), (0x21_9a02, &raw)] {
        harness::zip_archive(sample);
        hostile::fuzz(sample, seed, CASES, harness::zip_archive);
        hostile::truncations(sample, |_, bytes| harness::zip_archive(bytes));
    }
    harness::zip_archive(&[]);
}
