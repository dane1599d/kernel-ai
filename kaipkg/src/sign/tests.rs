//! `kaipkg::sign`'s tests.

use super::*;

/// RFC 8032 §7.1, test 1: the key and the signature of the empty message.
#[test]
fn keys_sign_as_rfc_8032_says() {
    let seed = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
    let seed: Vec<u8> = (0..64)
        .step_by(2)
        .map(|i| u8::from_str_radix(&seed[i..i + 2], 16).unwrap())
        .collect();
    let key = Key::from_seed(seed.try_into().unwrap());
    assert_eq!(
        key.public()["keyval"]["public"],
        "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"
    );
    assert_eq!(
        hex_bytes(key.pair.sk.sign(b"", None).as_ref()),
        "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065\
         224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b"
    );
    assert_eq!(key.id(), hex(&sha256(&canonical(&key.public()).unwrap())));
}

fn model() -> Model {
    Model {
        path: "tiny.gguf".into(),
        length: 4096,
        sha256: [7; 32],
        kai: json!({ "name": "Tiny" }),
        chunks: None,
    }
}

fn paths(files: &[(String, Vec<u8>)]) -> Vec<&str> {
    files.iter().map(|(p, _)| p.as_str()).collect()
}

#[test]
fn a_devices_own_repository_signs_no_models() {
    let key = Key::from_seed([1; Seed::BYTES]);
    let local = build(&[], &Keys::one(&key), Time(0), ADR_0008).unwrap();
    assert!(paths(&local).iter().all(|p| !p.contains(MODELS)));
    let with = build_with_models(&[], &[model()], &Keys::one(&key), Time(0), ADR_0008);
    assert!(with.is_err(), "no models key, no models");
}

#[test]
fn a_models_key_signs_each_model_without_its_bytes() {
    let key = Key::from_seed([1; Seed::BYTES]);
    let models_key = Key::from_seed([2; Seed::BYTES]);
    let keys = Keys {
        models: Some(&models_key),
        ..Keys::one(&key)
    };
    let files = build_with_models(&[], &[model()], &keys, Time(0), ADR_0008).unwrap();
    assert!(paths(&files).iter().all(|p| !p.starts_with("targets/")));
    let (_, role) = files
        .iter()
        .find(|(p, _)| p == "metadata/1.models.json")
        .unwrap();
    let role: Value = serde_json::from_slice(role).unwrap();
    let target = &role["signed"]["targets"]["tiny.gguf"];
    assert_eq!(target["length"], 4096);
    assert_eq!(target["hashes"]["sha256"], hex(&[7; 32]));
    assert_eq!(role["signatures"][0]["keyid"], models_key.id());
    let not_a_model = Model {
        path: "tiny.kpkg".into(),
        ..model()
    };
    assert!(build_with_models(&[], &[not_a_model], &keys, Time(0), ADR_0008).is_err());
}

#[test]
fn a_models_chunk_list_is_signed_beside_it_with_its_bytes() {
    let key = Key::from_seed([1; Seed::BYTES]);
    let models_key = Key::from_seed([2; Seed::BYTES]);
    let keys = Keys {
        models: Some(&models_key),
        ..Keys::one(&key)
    };
    let list = b"kai-chunks 1\n".to_vec();
    let with = Model {
        chunks: Some(list.clone()),
        ..model()
    };
    let files = build_with_models(&[], &[with], &keys, Time(0), ADR_0008).unwrap();
    let file = |path: &str| files.iter().find(|(p, _)| p == path).map(|(_, b)| b);
    let role: Value = serde_json::from_slice(file("metadata/1.models.json").unwrap()).unwrap();
    let name = format!("{}.chunks", hex(&[7; 32]));
    let target = &role["signed"]["targets"][&name];
    let sha = hex(&sha256(&list));
    assert_eq!(target["length"], list.len());
    assert_eq!(target["hashes"]["sha256"], sha);
    assert!(target.get("custom").is_none());
    assert_eq!(file(&format!("targets/{sha}.{name}")), Some(&list));
    // The model itself still has no bytes.
    assert!(role["signed"]["targets"]["tiny.gguf"].is_object());
    assert_eq!(
        paths(&files)
            .iter()
            .filter(|p| p.starts_with("targets/"))
            .count(),
        1
    );
    // The role is delegated both kinds of path.
    let top: Value = serde_json::from_slice(file("metadata/1.targets.json").unwrap()).unwrap();
    let delegated = &top["signed"]["delegations"]["roles"][1];
    assert_eq!(delegated["name"], MODELS);
    assert_eq!(delegated["paths"], json!(["*.gguf", "*.chunks"]));
    // Two models with one GGUF would sign one list twice.
    let twin = || Model {
        chunks: Some(list.clone()),
        ..model()
    };
    let second = Model {
        path: "twin.gguf".into(),
        ..twin()
    };
    let both = build_with_models(&[], &[twin(), second], &keys, Time(0), ADR_0008);
    assert!(both.unwrap_err().contains("twice"));
}
