//! Grants hold only for the image's own versions of a component.

use crate::grants::{FromImage, Grants, MAX_IMAGE_VERSIONS, is_boot_label};

const SETTINGS: Grants = Grants { settings: true };

#[test]
fn only_a_label_neither_an_upload_nor_a_package_is_the_images() {
    assert!(is_boot_label("1"));
    assert!(is_boot_label("v2"));
    assert!(!is_boot_label("e3"));
    assert!(!is_boot_label("p-0123456789ab"));
}

#[test]
fn an_upload_or_package_swapped_in_runs_without_its_grants() {
    let mut image = FromImage::default();
    image.fetched("1", [1; 32]);
    image.fetched("e7", [2; 32]);
    image.fetched("p-0123456789ab", [3; 32]);
    assert_eq!(image.grants_for(&[1; 32], SETTINGS), SETTINGS);
    assert_eq!(image.grants_for(&[2; 32], SETTINGS), Grants::default());
    assert_eq!(image.grants_for(&[3; 32], SETTINGS), Grants::default());
    // Never fetched at all: no grant either.
    assert_eq!(image.grants_for(&[4; 32], SETTINGS), Grants::default());
}

#[test]
fn the_same_bytes_as_the_images_version_are_that_version() {
    // An upload byte for byte equal to the image's artifact is the same code.
    let mut image = FromImage::default();
    image.fetched("1", [1; 32]);
    image.fetched("e8", [1; 32]);
    assert_eq!(image.grants_for(&[1; 32], SETTINGS), SETTINGS);
}

#[test]
fn it_remembers_the_latest_image_versions() {
    let mut image = FromImage::default();
    for n in 0..=MAX_IMAGE_VERSIONS as u8 {
        image.fetched("1", [n; 32]);
    }
    assert_eq!(image.grants_for(&[0; 32], SETTINGS), Grants::default());
    let last = [MAX_IMAGE_VERSIONS as u8; 32];
    assert_eq!(image.grants_for(&last, SETTINGS), SETTINGS);
}

#[test]
fn an_instance_says_what_it_was_granted() {
    let limits = crate::Limits {
        fuel_call: 1_000_000,
        fuel_life: 1_000_000,
        memory_bytes: 1 << 20,
    };
    let module = wat::parse_str(r#"(module (memory (export "memory") 1))"#).unwrap();
    let load = |grants| crate::load_core_granted(&module, limits, None, grants).unwrap();
    assert_eq!(load(SETTINGS).grants(), SETTINGS);
    assert_eq!(load(Grants::default()).grants(), Grants::default());
}
