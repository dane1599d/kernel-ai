//! The `fsproto_request` and `fsproto_reply` fuzz targets' bodies
//! (`fuzz/fsproto_harness.rs`), run here over hostile mutations of their
//! samples, so a harness that cannot build, or that panics on its own, fails
//! the host tests without libFuzzer. Each returns for every input and
//! allocates no more than a small multiple of it.

#[path = "../../testkit/hostile.rs"]
mod hostile;

#[path = "../../fuzz/fsproto_harness.rs"]
mod harness;

const ALLOC_PER_BYTE: usize = 16;
const ALLOC_FLOOR: usize = 64 * 1024;

fn bounded(bytes: &[u8], f: fn(&[u8])) {
    let ((), peak) = hostile::peak_alloc(|| f(bytes));
    let cap = ALLOC_PER_BYTE * bytes.len() + ALLOC_FLOOR;
    assert!(peak <= cap, "{peak} bytes allocated for {}", bytes.len());
}

fn run(samples: Vec<Vec<u8>>, seed: u64, f: fn(&[u8])) {
    for (i, sample) in samples.iter().enumerate() {
        // Every sample decodes, so the round trip is checked on it whole.
        f(sample);
        hostile::truncations(sample, |_, cut| bounded(cut, f));
        hostile::fuzz(sample, seed + i as u64, 3000, |bytes| bounded(bytes, f));
    }
}

#[test]
fn the_request_harness_returns_for_hostile_requests() {
    let samples = harness::requests();
    for sample in &samples {
        assert!(fsproto::decode_request(sample).is_ok());
    }
    run(samples, 0x6673_7271, harness::request);
}

#[test]
fn the_reply_harness_returns_for_hostile_replies() {
    let samples = harness::replies();
    for sample in &samples {
        assert!(fsproto::decode_reply(sample).is_ok());
    }
    run(samples, 0x6673_7270, harness::reply);
}

/// The targets' corpora, for `.github/workflows/fuzz.yml`: each seed in
/// `$FUZZ_SEED_DIR/<target>/`.
#[test]
#[ignore = "writes the fuzz corpora; run by the fuzz workflow"]
fn write_fuzz_seeds() {
    let dir = std::env::var("FUZZ_SEED_DIR").expect("FUZZ_SEED_DIR names the corpus folder");
    for (n, (target, bytes)) in harness::seeds().into_iter().enumerate() {
        let folder = std::path::Path::new(&dir).join(target);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join(format!("seed-{n}")), bytes).unwrap();
    }
}
