//! A hostile `kai-grants 1` file, as one read back from disk, and hostile grant
//! references, as the kernel reads them: the `kai_grants` fuzz target's body
//! (`fuzz/grants_harness.rs`), run here over truncations and mutations so a
//! harness that cannot build, or that panics on its own, fails the host tests
//! without libFuzzer. A file cut, mutated or bent at every byte is read or
//! refused with an error, never a panic; what is read writes back as the same
//! text; and each call allocates no more than a small multiple of its input.

#[path = "../../testkit/hostile.rs"]
mod hostile;

#[path = "../../fuzz/grants_harness.rs"]
mod harness;

use harness::SAMPLE;
use kaigrants::parse;

/// The tree is a fixed 64 KiB or so; the rest grows with the input.
const ALLOC_PER_BYTE: usize = 16;
const ALLOC_FLOOR: usize = 256 * 1024;
/// `parse` alone holds no tree, so it is held to the bound it had before the
/// harness: 16 KiB, and 16 B a byte.
const PARSE_FLOOR: usize = 16 * 1024;

fn bounded(bytes: &[u8]) {
    if let Ok(text) = core::str::from_utf8(bytes) {
        let (_, peak) = hostile::peak_alloc(|| parse(text));
        let cap = ALLOC_PER_BYTE * bytes.len() + PARSE_FLOOR;
        assert!(
            peak <= cap,
            "parse: {peak} bytes allocated for {}",
            bytes.len()
        );
    }
    let ((), peak) = hostile::peak_alloc(|| harness::grants(bytes));
    let cap = ALLOC_PER_BYTE * bytes.len() + ALLOC_FLOOR;
    assert!(peak <= cap, "{peak} bytes allocated for {}", bytes.len());
}

#[test]
fn the_sample_reads() {
    let grants = parse(SAMPLE).unwrap();
    assert_eq!((grants.grants.len(), grants.rules.len()), (2, 2));
    // Its one persistent grant is made again.
    assert_eq!(harness::file(SAMPLE), Some(1));
    harness::grants(SAMPLE.as_bytes());
}

#[test]
fn mutated_files_are_read_or_refused() {
    hostile::fuzz(SAMPLE.as_bytes(), 8, 30_000, bounded);
    hostile::truncations(SAMPLE.as_bytes(), |_, cut| bounded(cut));
}

#[test]
fn hostile_grant_references_are_refused_cleanly() {
    let steps = harness::sample_steps();
    harness::steps(&steps);
    hostile::fuzz(&steps, 0x6772_616e, 20_000, bounded);
    hostile::truncations(&steps, |_, cut| bounded(cut));
    hostile::sweep_fields(&steps, 0..steps.len(), |_, bytes| bounded(bytes));
}

/// A full tree: a file of more persistent grants than the tree has nodes is
/// made again until it is full, and refused cleanly past that.
#[test]
fn more_grants_than_nodes_fill_the_tree_and_stop() {
    let mut text = String::from("kai-grants 1\n");
    for id in 0..kaigrants::MAX_RECORDS {
        text.push_str(&format!(
            "grant {id} app=notes kind=dir object=/n/{id} rights=read from=picker life=persistent signer=- made=2026-11-02T10:14:00Z\n"
        ));
    }
    let made = harness::file(&text).expect("the file reads");
    assert_eq!(made, captable::grant::NODES - 3);
}

/// Writes the `kai_grants` target's seeds, one file each, to
/// `$FUZZ_SEED_DIR/kai_grants/`.
#[test]
#[ignore = "writes the fuzz corpus; run by the fuzz workflow"]
fn write_fuzz_seeds() {
    let dir = std::env::var("FUZZ_SEED_DIR").expect("FUZZ_SEED_DIR names the corpus folder");
    let folder = std::path::Path::new(&dir).join("kai_grants");
    std::fs::create_dir_all(&folder).unwrap();
    for (n, bytes) in [SAMPLE.as_bytes().to_vec(), harness::sample_steps()]
        .into_iter()
        .enumerate()
    {
        std::fs::write(folder.join(format!("seed-{n}")), bytes).unwrap();
    }
}
