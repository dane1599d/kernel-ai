//! The `models` role (ADR 0008 §6; P4 task 10) as the packer signs it and as the
//! OS reads it back with `kaipkg::models`.

use super::mirrors::tampered;
use super::models::{self, Model};
use super::tests::{NOW, edited_json, keys, notes};
use super::tuf::{self, Key, envelope};
use super::{Repository, image_layout};
use serde_json::{Value, json};

/// `kaipkg::models::check` of `repo`, laid out as the image holds it.
fn read(repo: Repository) -> Result<kaipkg::models::Checked, String> {
    kaipkg::models::check(&mut image_layout(repo)?)
}

/// `path`'s `signed` with `edit` applied, signed again by `key`.
fn resigned(repo: &Repository, path: &str, key: &Key, edit: impl Fn(&mut Value)) -> Repository {
    tampered(repo, path, |bytes| {
        let mut value: Value = serde_json::from_slice(bytes).unwrap();
        edit(&mut value["signed"]);
        *bytes = envelope(&value["signed"], key).unwrap();
    })
}

fn as_signed(model: &Model) -> kaipkg::models::Model {
    kaipkg::models::Model {
        id: model.id.clone(),
        name: model.name.clone(),
        length: model.length,
        sha256: kaipkg::unhex(&model.sha256).unwrap(),
        licence: model.licence.clone(),
        source: model.source.clone(),
    }
}

#[test]
fn the_os_reads_every_model_the_role_signs() {
    let signed = models::for_image().unwrap();
    let repo = tuf::build_with_models(&[notes()], &signed, &keys(1), NOW).unwrap();
    let checked = read(repo).unwrap();
    assert_eq!(checked.refused, []);
    let want: Vec<_> = signed.iter().map(as_signed).collect();
    let mut got = checked.models;
    got.sort_by(|a, b| a.id.cmp(&b.id));
    let mut want = want;
    want.sort_by(|a, b| a.id.cmp(&b.id));
    assert_eq!(got, want);
}

#[test]
fn the_packages_still_verify_beside_the_models() {
    let signed = models::for_image().unwrap();
    let repo = tuf::build_with_models(&[notes()], &signed, &keys(1), NOW).unwrap();
    let mut files = image_layout(repo).unwrap();
    let checked = kaipkg::image::check(&mut files, kaipkg::PROVIDED).unwrap();
    let kept: Vec<&str> = checked.packages.iter().map(|(e, _)| &*e.name).collect();
    assert_eq!((kept, checked.refused.len()), (vec!["notes"], 0));
}

#[test]
fn the_catalog_is_signed_as_it_is_pinned() {
    let catalog = models::catalog().unwrap();
    let qwen = catalog.iter().find(|m| m.id == "qwen3-4b-2507").unwrap();
    assert_eq!(qwen.length, 2_497_281_120);
    assert_eq!(
        qwen.sha256,
        "3605803b982cb64aead44f6c1b2ae36e3acdb41d8e46c8a94c6533bc4c67e597"
    );
    assert_eq!(qwen.licence, "Apache-2.0");
    assert!(qwen.source.starts_with(
        "https://huggingface.co/unsloth/Qwen3-4B-Instruct-2507-GGUF/resolve/a06e946bb6b655725eafa393f4a9745d460374c9/"
    ));
    assert!(catalog.iter().all(|m| m.id != models::FIXTURE));
    let fixture = models::fixture();
    let bytes = crate::disk::Fixture::Gguf.bytes();
    assert_eq!(fixture.length, bytes.len() as u64);
    assert_eq!(fixture.sha256, crate::sha256_hex(&bytes));
    assert!(bytes.starts_with(b"GGUF"));
    // Every field reads back within the OS's limits.
    let repo = tuf::build_with_models(&[], &models::for_image().unwrap(), &keys(1), NOW);
    assert_eq!(read(repo.unwrap()).unwrap().refused, []);
}

#[test]
fn a_tampered_role_keeps_no_model() {
    let signed = models::for_image().unwrap();
    let repo = tuf::build_with_models(&[], &signed, &keys(1), NOW).unwrap();
    // Each edit leaves the role's own signature, or its delegator's, unmet.
    let cases = [
        edited_json(&repo, "metadata/1.models.json", |v| {
            v["signed"]["targets"]["fixture.gguf"]["hashes"]["sha256"] = "00".repeat(32).into();
        }),
        resigned(&repo, "metadata/1.models.json", &keys(1).packages, |_| {}),
        resigned(
            &repo,
            "metadata/1.models.json",
            &Key::from_seed([9; 32]),
            |_| {},
        ),
        edited_json(&repo, "metadata/1.targets.json", |v| {
            v["signed"]["delegations"]["roles"][1]["paths"] = json!(["*"]);
        }),
    ];
    for (i, repo) in cases.into_iter().enumerate() {
        let err = read(repo)
            .err()
            .unwrap_or_else(|| panic!("case {i} kept models"));
        assert!(!err.is_empty(), "case {i}");
    }
}

#[test]
fn neither_role_signs_for_the_others_paths() {
    let keys = keys(1);
    let repo = tuf::build_with_models(&[notes()], &models::for_image().unwrap(), &keys, NOW);
    let repo = repo.unwrap();
    let package = |v: &mut Value| {
        let entry = v["targets"]["fixture.gguf"].clone();
        v["targets"]["evil-1.0.0.kpkg"] = entry;
    };
    let model = |v: &mut Value| {
        let entry = v["targets"]["notes-1.0.0.kpkg"].clone();
        v["targets"]["evil.gguf"] = entry;
    };
    let repo = resigned(&repo, "metadata/1.models.json", &keys.models, package);
    let repo = resigned(&repo, "metadata/1.packages.json", &keys.packages, model);
    let checked = read(Repository {
        files: repo.files.clone(),
    })
    .unwrap();
    assert!(checked.models.iter().all(|m| m.id != "evil"));
    assert_eq!(checked.refused.len(), 1, "{:?}", checked.refused);
    assert_eq!(checked.refused[0].0, "evil-1.0.0.kpkg");
    let mut files = image_layout(repo).unwrap();
    let packages = kaipkg::image::check(&mut files, kaipkg::PROVIDED).unwrap();
    assert!(packages.packages.iter().all(|(e, _)| e.name == "notes"));
    assert!(packages.refused.iter().any(|(path, _)| path == "evil.gguf"));
}

#[test]
fn a_malformed_target_is_refused_by_path_and_the_rest_kept() {
    let keys = keys(1);
    let repo = tuf::build_with_models(&[], &models::for_image().unwrap(), &keys, NOW).unwrap();
    let repo = resigned(&repo, "metadata/1.models.json", &keys.models, |v| {
        v["targets"]["fixture.gguf"]["custom"]["kai"]["run"] = "this".into();
    });
    let checked = read(repo).unwrap();
    assert_eq!(checked.refused.len(), 1);
    assert_eq!(checked.refused[0].0, "fixture.gguf");
    assert_eq!(checked.models.len(), models::catalog().unwrap().len());
}

#[test]
fn a_model_twice_is_refused() {
    let fixture = models::fixture();
    let err = tuf::build_with_models(&[], &[fixture.clone(), fixture], &keys(1), NOW);
    assert!(err.err().unwrap().contains("twice"));
}
