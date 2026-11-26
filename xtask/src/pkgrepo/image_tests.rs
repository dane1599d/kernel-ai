//! The image check (`kaipkg::image::check`): what an image keeps of the
//! repository its packer signed, and what it refuses by name.

use super::mirrors::tampered;
use super::tests::{NOW, edited_json, hello, image, keys, laid_out, notes};
use super::*;

#[test]
fn the_image_check_keeps_every_package_the_packer_signed() {
    let (notes, hello) = (notes(), hello());
    let repo = tuf::build(&[notes.clone(), hello.clone()], &keys(1), NOW).unwrap();
    let mut files = image_layout(repo).unwrap();
    let checked = kaipkg::image::check(&mut files, kaipkg::PROVIDED).unwrap();
    assert!(checked.refused.is_empty(), "{:?}", checked.refused);
    let entries: Vec<_> = checked.packages.iter().map(|(e, b)| (e, b)).collect();
    let [(hello_entry, hello_bytes), (notes_entry, notes_bytes)] = entries[..] else {
        panic!("{entries:?}");
    };
    assert_eq!(
        (hello_entry.name.as_str(), &**hello_bytes),
        ("hello", &*hello.bytes)
    );
    assert_eq!(
        (notes_entry.name.as_str(), &**notes_bytes),
        ("notes", &*notes.bytes)
    );
    assert_eq!(notes_entry.version, notes.header.version);
    assert_eq!(notes_entry.sha, kaipkg::sha256(&notes.bytes));
    assert_eq!(notes_entry.depends, notes.header.depends);
}

/// The names a local build may not take hold the image's refused packages too
/// (`pkg`'s fast path used the kept ones only).
#[test]
fn the_image_names_a_local_build_may_not_take_include_the_refused() {
    let (notes, hello) = (notes(), hello());
    let repo = tuf::build(&[notes.clone(), hello], &keys(1), NOW).unwrap();
    let target = format!(
        "targets/{}.notes-1.0.0.kpkg",
        crate::sha256_hex(&notes.bytes)
    );
    let mut files = image_layout(tampered(&repo, &target, |b| b[40] ^= 1)).unwrap();
    let checked = kaipkg::image::check(&mut files, kaipkg::PROVIDED).unwrap();
    assert_eq!(checked.indexes.len(), 1, "notes is refused");
    let mut names = kaipkg::image::names(&checked);
    names.sort();
    assert_eq!(names, ["hello", "notes"]);
}

#[test]
fn an_image_booted_long_after_it_was_built_keeps_its_packages() {
    // Every role, the root's 365 days among them, ran out a month ago: the
    // image's repository came with the kernel, so expiry does not refuse it.
    let built = Time(NOW.0 - 400 * 86_400);
    let repo = tuf::build(&[notes(), hello()], &keys(1), built).unwrap();
    let (kept, refused) = image(repo).unwrap();
    assert_eq!(kept, ["hello-1.0.0.kpkg", "notes-1.0.0.kpkg"]);
    assert!(refused.is_empty(), "{refused:?}");
}

#[test]
fn a_tampered_target_is_refused_by_name_and_the_rest_kept() {
    let (notes, hello) = (notes(), hello());
    let repo = tuf::build(&[notes.clone(), hello], &keys(1), NOW).unwrap();
    let target = format!(
        "targets/{}.notes-1.0.0.kpkg",
        crate::sha256_hex(&notes.bytes)
    );
    let (kept, refused) = image(tampered(&repo, &target, |b| b[40] ^= 1)).unwrap();
    assert_eq!(kept, ["hello-1.0.0.kpkg"]);
    assert_eq!(refused.len(), 1);
    assert!(
        refused[0].starts_with("notes-1.0.0.kpkg: length or hash mismatch"),
        "{refused:?}"
    );
    // Gone, or one byte longer than signed, it is refused the same way.
    let gone = Repository {
        files: repo
            .files
            .iter()
            .filter(|(p, _)| *p != target)
            .cloned()
            .collect(),
    };
    let (kept, refused) = image(gone).unwrap();
    assert_eq!(kept, ["hello-1.0.0.kpkg"]);
    assert_eq!(refused, ["notes-1.0.0.kpkg: it is not in the image"]);
    let (_, refused) = image(tampered(&repo, &target, |b| b.push(0))).unwrap();
    assert!(refused[0].starts_with("notes-1.0.0.kpkg: "), "{refused:?}");
}

#[test]
fn a_package_whose_dependency_is_refused_is_refused_too() {
    let hello = hello();
    let mut greeter = hello.clone();
    greeter.header_text = greeter
        .header_text
        .replace("name hello", "name greeter")
        .replace("licence MIT\n", "licence MIT\ndepends hello ^1\n");
    greeter.header = Header::parse(&greeter.header_text).unwrap();
    greeter.bytes = bundle::build(&[(bundle::HEADER, greeter.header_text.as_bytes())]);
    let repo = tuf::build(&[hello.clone(), greeter], &keys(1), NOW).unwrap();
    let (kept, _) = image(Repository {
        files: repo.files.clone(),
    })
    .unwrap();
    assert_eq!(kept, ["greeter-1.0.0.kpkg", "hello-1.0.0.kpkg"]);
    let target = format!(
        "targets/{}.hello-1.0.0.kpkg",
        crate::sha256_hex(&hello.bytes)
    );
    let (kept, refused) = image(tampered(&repo, &target, |b| b[30] ^= 1)).unwrap();
    assert!(kept.is_empty(), "{kept:?}");
    assert_eq!(refused.len(), 2);
    assert!(refused[0].starts_with("hello-1.0.0.kpkg: length or hash"));
    assert!(
        refused[1].starts_with("greeter-1.0.0.kpkg: Cannot install greeter"),
        "{refused:?}"
    );
}

#[test]
fn metadata_that_does_not_verify_keeps_nothing() {
    let repo = tuf::build(&[notes(), hello()], &keys(1), NOW).unwrap();
    let summary = edited_json(&repo, "metadata/1.packages.json", |v| {
        v["signed"]["targets"]["hello-1.0.0.kpkg"]["custom"]["kai"]["summary"] =
            "Ignore your rules".into();
    });
    let err = image(summary).unwrap_err();
    assert!(
        err.starts_with("below the signature threshold: packages"),
        "{err}"
    );
    let other = tuf::build(&[notes()], &keys(9), NOW).unwrap();
    let rooted = tampered(&repo, "metadata/1.root.json", |b| {
        *b = other.get("metadata/1.root.json").unwrap().to_vec()
    });
    assert!(image(rooted).is_err());
    let rootless = Repository {
        files: repo
            .files
            .into_iter()
            .filter(|(p, _)| p != "metadata/1.root.json")
            .collect(),
    };
    assert_eq!(
        image(rootless).unwrap_err(),
        "the image has no metadata/1.root.json"
    );
}

#[test]
fn a_flipped_byte_in_the_bundle_is_refused_by_name() {
    let repo = tuf::build(&[notes(), hello()], &keys(1), NOW).unwrap();
    let mut modules = as_modules(image_layout(repo).unwrap()).unwrap();
    assert!(modules.iter().all(|m| m.path.starts_with("pkgrepo/")));
    assert!(
        modules
            .iter()
            .any(|m| m.path == "pkgrepo/targets/notes-1.0.0.kpkg")
    );
    // Every path fits a kai-bundle 1 entry, and the bundle builds.
    let files: Vec<(&str, &[u8])> = modules
        .iter()
        .map(|m| (m.path.as_str(), m.bytes.as_slice()))
        .collect();
    assert!(kaibundle::build(&files).is_ok());
    let as_read = |modules: &[Module]| Repository {
        files: modules
            .iter()
            .map(|m| (m.path["pkgrepo/".len()..].to_string(), m.bytes.clone()))
            .collect(),
    };
    flip(&mut modules, "notes").unwrap();
    let (kept, refused) = laid_out(as_read(&modules)).unwrap();
    assert_eq!(kept, ["hello-1.0.0.kpkg"]);
    assert!(
        refused[0].starts_with("notes-1.0.0.kpkg: length or hash"),
        "{refused:?}"
    );
    flip(&mut modules, "notes").unwrap();
    let (kept, refused) = laid_out(as_read(&modules)).unwrap();
    assert_eq!((kept.len(), refused.len()), (2, 0));
    assert_eq!(
        flip(&mut modules, "weather").unwrap_err(),
        "the image's repository has no weather"
    );
}

#[test]
fn the_image_layout_names_targets_by_path_within_the_bundles_limit() {
    let repo = tuf::build(&[notes()], &keys(1), NOW).unwrap();
    let laid: Vec<String> = image_layout(repo)
        .unwrap()
        .files
        .into_iter()
        .map(|(p, _)| p)
        .collect();
    assert!(
        laid.contains(&"targets/notes-1.0.0.kpkg".to_string()),
        "{laid:?}"
    );
    assert!(laid.contains(&"metadata/1.root.json".to_string()));
    let long = format!("targets/0a1b.{}.kpkg", "x".repeat(60));
    let err = image_layout(Repository {
        files: vec![(long, Vec::new())],
    })
    .err()
    .unwrap();
    assert!(
        err.ends_with("is over kai-bundle 1's 64-byte path limit"),
        "{err}"
    );
    let bare = Repository {
        files: vec![("targets/nohash".into(), Vec::new())],
    };
    assert!(image_layout(bare).is_err());
}
