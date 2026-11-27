//! The `kpkg` and `tuf_metadata` fuzz targets' bodies (`fuzz/pkg_harness.rs`),
//! run here over hostile mutations of their samples, so a harness that cannot
//! build, or that panics on its own, fails the host tests without libFuzzer.
//! Each returns for every input and allocates no more than a small multiple
//! of it.

#[path = "../../testkit/hostile.rs"]
mod hostile;

#[path = "../../fuzz/pkg_harness.rs"]
mod harness;

const ALLOC_PER_BYTE: usize = 64;
const ALLOC_FLOOR: usize = 1 << 20;

fn bounded(bytes: &[u8], f: fn(&[u8])) {
    let ((), peak) = hostile::peak_alloc(|| f(bytes));
    let cap = ALLOC_PER_BYTE * bytes.len() + ALLOC_FLOOR;
    assert!(peak <= cap, "{peak} bytes allocated for {}", bytes.len());
}

#[test]
fn the_samples_are_what_the_targets_start_from() {
    let app = &harness::packages()[1];
    let files = kaipkg::bundle::files(app).unwrap();
    let header = kaipkg::header::Header::parse(core::str::from_utf8(files[0].1).unwrap()).unwrap();
    // The app sample reaches its artifact's parser.
    assert_eq!(kaipkg::app::service(&header, app), Ok(Some("notes".into())));
    let checked = harness::check_sample();
    assert_eq!(checked.packages.len(), 2, "{:?}", checked.refused);
    assert!(checked.refused.is_empty(), "{:?}", checked.refused);
}

#[test]
fn the_kpkg_harness_returns_for_hostile_packages() {
    for (i, package) in harness::packages().iter().enumerate() {
        harness::kpkg(package);
        hostile::truncations(package, |_, cut| bounded(cut, harness::kpkg));
        hostile::fuzz(package, 0x6b70_6b67 + i as u64, 3000, |bytes| {
            bounded(bytes, harness::kpkg)
        });
    }
    for header in [harness::DATA, harness::APP] {
        hostile::fuzz(header.as_bytes(), 0x6864_7200, 3000, |bytes| {
            bounded(bytes, harness::kpkg)
        });
    }
}

#[test]
fn the_tuf_harness_returns_for_hostile_metadata() {
    let repository = harness::repository();
    for (i, (path, bytes)) in repository.iter().enumerate() {
        if !path.starts_with("metadata/") {
            continue;
        }
        // The first byte names the file the rest replaces.
        let mut input = vec![i as u8];
        input.extend_from_slice(bytes);
        harness::tuf_metadata(&input);
        hostile::fuzz(&input, 0x7475_6600 + i as u64, 150, |bytes| {
            bounded(bytes, harness::tuf_metadata)
        });
        hostile::truncations(bytes, |_, cut| harness::tuf_metadata(cut));
    }
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
