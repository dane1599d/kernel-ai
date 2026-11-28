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

/// A hostile mirror cannot change what signed metadata says and still pass:
/// each byte of each metadata file of the sample repository, flipped two ways
/// (every bit, and every bit but the lowest), is refused by the image check,
/// unless the file's signed part still has the same canonical bytes (a
/// re-spaced copy). Every byte, not a sample.
#[test]
fn no_flipped_byte_of_signed_metadata_passes_the_image_check() {
    let repository = harness::repository();
    let (mut refused, mut respaced) = (0usize, 0usize);
    for (path, original) in repository
        .iter()
        .filter(|(p, _)| p.starts_with("metadata/"))
    {
        assert!(
            harness::signed_bytes(path, original).is_some(),
            "{path} is read as its role"
        );
        for at in 0..original.len() {
            for mask in [0xff, 0xfe] {
                let mut bytes = original.clone();
                bytes[at] ^= mask;
                match harness::check_replaced(path, &bytes) {
                    Err(_) => refused += 1,
                    Ok(_) => {
                        // It passed: only if the signed part says the same.
                        harness::same_signed_bytes(path, &bytes);
                        respaced += 1;
                    }
                }
            }
        }
    }
    assert!(refused > 0, "nothing was refused");
    eprintln!("{refused} refused, {respaced} passed with the same signed bytes");
}

/// The flip test's allowance is live: a metadata file re-spaced (pretty
/// printed) that the image check takes says the same signed bytes. The root
/// and the targets roles are covered by signatures alone, so they pass; the
/// snapshot is pinned by the timestamp's hash of its raw bytes, so a re-spaced
/// one is refused.
#[test]
fn a_respaced_copy_passes_where_only_signatures_cover_it() {
    let repository = harness::repository();
    let mut passed = Vec::new();
    for (path, original) in repository
        .iter()
        .filter(|(p, _)| p.starts_with("metadata/"))
    {
        let value: serde_json::Value = serde_json::from_slice(original).unwrap();
        let pretty = serde_json::to_vec_pretty(&value).unwrap();
        assert_ne!(&pretty, original, "{path} was re-spaced");
        if harness::check_replaced(path, &pretty).is_ok() {
            harness::same_signed_bytes(path, &pretty);
            passed.push(path.as_str());
        }
    }
    assert!(passed.contains(&"metadata/1.root.json"), "{passed:?}");
    assert!(!passed.contains(&"metadata/1.snapshot.json"), "{passed:?}");
}

#[test]
#[should_panic(expected = "the image check took metadata whose signed part changed")]
fn the_assertion_refuses_metadata_that_says_something_else() {
    let repository = harness::repository();
    let other = &repository["metadata/1.packages.json"];
    // Another role's file has other signed bytes, though it is valid JSON.
    harness::same_signed_bytes("metadata/1.targets.json", other);
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
