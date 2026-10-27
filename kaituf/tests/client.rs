//! The client workflow over a repository signed in the test: what it fetches,
//! what it keeps, and what it refuses. tuf-conformance's suite runs the same
//! cases and more against the CLI in CI; these keep them on every host run.

mod common;

use common::*;
use kaituf::Error;
use kaituf::updater::{Updater, check_target};
use serde_json::json;

#[test]
fn a_refresh_fetches_the_top_level_roles_in_order_and_keeps_their_bytes() {
    let repo = Repo::new();
    let mut cache = fresh(&repo);
    let (result, log) = refresh(&repo, &mut cache, NOW);
    result.unwrap();
    assert_eq!(
        log,
        [
            "2.root.json",
            "timestamp.json",
            "1.snapshot.json",
            "1.targets.json"
        ]
    );
    assert_eq!(cache.0["timestamp"], repo.files["timestamp.json"]);
    assert_eq!(cache.0["snapshot"], repo.files["1.snapshot.json"]);
    assert_eq!(cache.0["targets"], repo.files["1.targets.json"]);
    // A second refresh uses the cached snapshot and targets, which are current.
    let (result, log) = refresh(&repo, &mut cache, NOW);
    result.unwrap();
    assert_eq!(log, ["2.root.json", "timestamp.json"]);
}

#[test]
fn root_rotates_only_when_signed_by_the_old_keys_and_the_new() {
    let mut repo = Repo::new();
    let mut cache = fresh(&repo);
    let old = repo.signers["root"].clone();
    repo.signers
        .insert("root".into(), vec![Signer::ed25519(50)]);
    repo.publish_root(&old);
    repo.publish_root(&[]);
    refresh(&repo, &mut cache, NOW).0.unwrap();
    assert_eq!(cached_version(&cache, "root"), Some(3));

    // Version 4 lacks the old (v3) key's signature.
    repo.signers
        .insert("root".into(), vec![Signer::ed25519(51)]);
    repo.publish_root(&[]);
    let (result, _) = refresh(&repo, &mut cache, NOW);
    assert!(matches!(result, Err(Error::Threshold(_))), "{result:?}");
    assert_eq!(cached_version(&cache, "root"), Some(3));
}

#[test]
fn a_role_short_of_its_threshold_is_refused() {
    let mut repo = Repo::new();
    repo.signers
        .get_mut("snapshot")
        .unwrap()
        .push(Signer::ed25519(60));
    repo.thresholds.insert("snapshot".into(), 2);
    let old = repo.signers["root"].clone();
    repo.publish_root(&old);
    let extra = repo.signers.get_mut("snapshot").unwrap().pop().unwrap();
    repo.publish(&["targets"]);
    let mut cache = fresh(&repo);
    let (result, _) = refresh(&repo, &mut cache, NOW);
    assert!(
        matches!(result, Err(Error::Threshold(ref w)) if w.contains("snapshot")),
        "{result:?}"
    );
    assert_eq!(cached_version(&cache, "snapshot"), None);
    // With both keys it holds.
    repo.signers.get_mut("snapshot").unwrap().push(extra);
    repo.publish(&["targets"]);
    refresh(&repo, &mut cache, NOW).0.unwrap();
}

#[test]
fn two_signatures_by_one_key_are_refused_and_a_bad_extra_one_is_ignored() {
    let mut repo = Repo::new();
    let mut doubled: serde_json::Value =
        serde_json::from_slice(&repo.files["timestamp.json"]).unwrap();
    let sig = doubled["signatures"][0].clone();
    doubled["signatures"].as_array_mut().unwrap().push(sig);
    repo.files.insert(
        "timestamp.json".into(),
        serde_json::to_vec(&doubled).unwrap(),
    );
    let (result, _) = refresh(&repo, &mut fresh(&repo), NOW);
    assert!(
        matches!(result, Err(Error::Malformed(ref w)) if w.contains("two signatures")),
        "{result:?}"
    );

    let mut extra: serde_json::Value =
        serde_json::from_slice(&repo.files["1.targets.json"]).unwrap();
    extra["signatures"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "keyid": "ab", "sig": "" }));
    repo.files
        .insert("1.targets.json".into(), serde_json::to_vec(&extra).unwrap());
    repo.publish_timestamp(json!({ "version": 1 }));
    refresh(&repo, &mut fresh(&repo), NOW).0.unwrap();
}

#[test]
fn versions_never_go_back() {
    let mut repo = Repo::new();
    let mut cache = fresh(&repo);
    repo.publish(&["targets"]);
    refresh(&repo, &mut cache, NOW).0.unwrap();
    assert_eq!(cached_version(&cache, "timestamp"), Some(2));

    // An older timestamp.
    let newer = repo.files["timestamp.json"].clone();
    repo.version.insert("timestamp".into(), 0);
    repo.publish_timestamp(json!({ "version": 2 }));
    let (result, _) = refresh(&repo, &mut cache, NOW);
    assert!(matches!(result, Err(Error::Version(_))), "{result:?}");
    assert_eq!(cache.0["timestamp"], newer);

    // A timestamp naming an older snapshot.
    repo.version.insert("timestamp".into(), 2);
    repo.publish_timestamp(json!({ "version": 1 }));
    assert!(matches!(
        refresh(&repo, &mut cache, NOW).0,
        Err(Error::Version(_))
    ));
    assert_eq!(cached_version(&cache, "timestamp"), Some(2));

    // A snapshot naming an older targets.
    repo.version.insert("targets".into(), 0);
    repo.version.insert("timestamp".into(), 3);
    repo.publish(&["targets"]);
    let (result, _) = refresh(&repo, &mut cache, NOW);
    assert!(
        matches!(result, Err(Error::Version(ref w)) if w.contains("older targets.json")),
        "{result:?}"
    );
    assert_eq!(cached_version(&cache, "snapshot"), Some(2));
}

#[test]
fn an_equal_timestamp_keeps_the_trusted_one() {
    let mut repo = Repo::new();
    let mut cache = fresh(&repo);
    refresh(&repo, &mut cache, NOW).0.unwrap();
    let kept = cache.0["timestamp"].clone();
    repo.version.insert("timestamp".into(), 0);
    repo.publish_timestamp(json!({ "version": 1, "length": 1 }));
    refresh(&repo, &mut cache, NOW).0.unwrap();
    assert_eq!(cache.0["timestamp"], kept);
}

#[test]
fn expired_metadata_is_refused_and_not_kept() {
    let mut repo = Repo::new();
    let mut cache = fresh(&repo);
    refresh(&repo, &mut cache, NOW).0.unwrap();
    repo.expires.insert("timestamp".into(), PAST.into());
    repo.publish(&["targets"]);
    let (result, _) = refresh(&repo, &mut cache, NOW);
    assert_eq!(result, Err(Error::Expired("timestamp".into())));
    assert_eq!(cached_version(&cache, "timestamp"), Some(1));
    // A refresh far enough ahead finds the root expired.
    let (result, _) = refresh(&repo, &mut fresh(&repo), 2_000_000_000);
    assert_eq!(result, Err(Error::Expired("root".into())));
}

#[test]
fn hashes_named_in_meta_are_checked() {
    let mut repo = Repo::new();
    repo.hashes_in_meta = true;
    repo.publish(&["targets"]);
    let mut cache = fresh(&repo);
    refresh(&repo, &mut cache, NOW).0.unwrap();
    repo.publish(&["targets"]);
    let snapshot = format!("{}.snapshot.json", repo.version["snapshot"]);
    // The same length, so the hash is what catches it.
    *repo.files.get_mut(&snapshot).unwrap().last_mut().unwrap() = b' ';
    let (result, _) = refresh(&repo, &mut cache, NOW);
    assert!(matches!(result, Err(Error::Hash(_))), "{result:?}");
    // One byte longer is past the length the timestamp names.
    repo.files.get_mut(&snapshot).unwrap().push(b' ');
    let (result, _) = refresh(&repo, &mut cache, NOW);
    assert!(matches!(result, Err(Error::TooLarge(_))), "{result:?}");
}

#[test]
fn p256_keys_in_pem_and_hex_verify_with_either_s() {
    for (pem, high_s) in [(true, false), (true, true), (false, false), (false, true)] {
        let mut repo = Repo::new();
        repo.signers
            .insert("timestamp".into(), vec![Signer::p256(7, pem, high_s)]);
        let old = repo.signers["root"].clone();
        repo.publish_root(&old);
        repo.publish(&["targets"]);
        refresh(&repo, &mut fresh(&repo), NOW).0.unwrap();
    }
    // A P-256 root key, then a tampered message.
    let mut repo = Repo::new();
    let old = repo.signers["root"].clone();
    repo.signers
        .insert("root".into(), vec![Signer::p256(9, true, true)]);
    repo.publish_root(&old);
    let mut cache = fresh(&repo);
    refresh(&repo, &mut cache, NOW).0.unwrap();
    assert_eq!(cached_version(&cache, "root"), Some(2));
    repo.publish_root(&[]);
    let name = format!("{}.root.json", repo.version["root"]);
    let text = String::from_utf8(repo.files[&name].clone())
        .unwrap()
        .replace("\"version\":3", "\"version\":3,\"x\":1");
    repo.files.insert(name, text.into_bytes());
    assert!(matches!(
        refresh(&repo, &mut cache, NOW).0,
        Err(Error::Threshold(_))
    ));
}

#[test]
fn a_role_that_needs_an_unchecked_key_type_is_refused_by_name() {
    let mut repo = Repo::new();
    let p384 = Signer::p384_public();
    let id = key_id(&p384);
    let mut root = repo.root_signed();
    root["version"] = json!(2);
    root["keys"][&id] = p384;
    root["roles"]["timestamp"] = json!({ "keyids": [id], "threshold": 1 });
    let signed = envelope(&root, &[&repo.signers["root"][0]]);
    repo.files.insert("2.root.json".into(), signed);
    let (result, _) = refresh(&repo, &mut fresh(&repo), NOW);
    assert!(
        matches!(result, Err(Error::UnsupportedKey(ref w)) if w.contains("ecdsa/ecdsa-sha2-nistp384")),
        "{result:?}"
    );
    assert!(
        result
            .unwrap_err()
            .to_string()
            .starts_with("unsupported key type: timestamp")
    );
}

#[test]
fn delegations_are_searched_preorder_depth_first() {
    let mut repo = Repo::new();
    repo.delegate("targets", "a", Signer::ed25519(70), &["*"], false);
    repo.delegate("targets", "b", Signer::ed25519(71), &["*"], false);
    repo.delegate("a", "c", Signer::ed25519(72), &["*"], false);
    repo.delegate("a", "d", Signer::ed25519(73), &["*"], false);
    repo.add_artifact("b", "file", b"from b");
    repo.publish(&["c", "d", "a", "b", "targets"]);
    let mut cache = fresh(&repo);
    let mut remote = Served {
        repo: &repo,
        log: Vec::new(),
    };
    let mut updater = Updater::new(&mut remote, &mut cache, NOW).unwrap();
    updater.refresh().unwrap();
    let found = updater.target("file").unwrap().unwrap();
    assert_eq!(found.length, 6);
    check_target(&found, b"from b").unwrap();
    assert!(check_target(&found, b"from c").is_err());
    let url = updater.target_url(&found);
    assert!(url.ends_with(".file") && url.len() == 64 + 5, "{url}");
    assert_eq!(updater.target("missing").unwrap(), None);
    drop(updater);
    let visited: Vec<&str> = remote.log[4..].iter().map(String::as_str).collect();
    assert_eq!(visited, ["1.a.json", "1.c.json", "1.d.json", "1.b.json"]);
}

#[test]
fn a_terminating_role_ends_the_search_and_paths_narrow_it() {
    let mut repo = Repo::new();
    repo.delegate("targets", "a", Signer::ed25519(70), &["*.txt"], true);
    repo.delegate("targets", "b", Signer::ed25519(71), &["*"], false);
    repo.delegate("targets", "c", Signer::ed25519(72), &["dir/*"], false);
    repo.add_artifact("b", "x.txt", b"b's");
    repo.add_artifact("c", "dir/y", b"c's");
    repo.publish(&["a", "b", "c", "targets"]);
    let mut cache = fresh(&repo);
    let mut remote = Served {
        repo: &repo,
        log: Vec::new(),
    };
    let mut updater = Updater::new(&mut remote, &mut cache, NOW).unwrap();
    updater.refresh().unwrap();
    // a is terminating for *.txt: b is never searched for it.
    assert_eq!(updater.target("x.txt").unwrap(), None);
    // `*` crosses no `/`: only c's dir/* is searched for dir/y.
    assert!(updater.target("dir/y").unwrap().is_some());
    drop(updater);
    assert_eq!(&remote.log[4..], ["1.a.json", "1.c.json"]);
}

#[test]
fn a_role_reached_through_another_delegator_is_verified_again() {
    let mut repo = Repo::new();
    repo.delegate("targets", "pa", Signer::ed25519(80), &["a/*"], false);
    repo.delegate("targets", "pb", Signer::ed25519(81), &["b/*"], false);
    repo.delegate("pa", "release", Signer::ed25519(82), &["a/*"], false);
    repo.delegate("pb", "release", Signer::ed25519(83), &["b/*"], false);
    repo.add_artifact("release", "a/x", b"trusted");
    repo.add_artifact("release", "b/x", b"not pb's");
    // release is signed by pa's key for it, not pb's.
    let release = json!({
        "_type": "targets", "spec_version": "1.0.31", "version": 1, "expires": LATER,
        "targets": repo.delegated_targets["release"],
    });
    repo.files.insert(
        "1.release.json".into(),
        envelope(&release, &[&Signer::ed25519(82)]),
    );
    repo.version.insert("release".into(), 1);
    repo.publish(&["pa", "pb", "targets"]);
    let mut cache = fresh(&repo);
    let mut remote = Served {
        repo: &repo,
        log: Vec::new(),
    };
    let mut updater = Updater::new(&mut remote, &mut cache, NOW).unwrap();
    updater.refresh().unwrap();
    assert!(updater.target("a/x").unwrap().is_some());
    assert!(matches!(updater.target("b/x"), Err(Error::Threshold(_))));
}

#[test]
fn a_delegated_role_missing_from_the_snapshot_is_not_fetched() {
    let mut repo = Repo::new();
    repo.delegate("targets", "gone", Signer::ed25519(90), &["*"], false);
    repo.publish(&["targets"]);
    let mut cache = fresh(&repo);
    let mut remote = Served {
        repo: &repo,
        log: Vec::new(),
    };
    let mut updater = Updater::new(&mut remote, &mut cache, NOW).unwrap();
    updater.refresh().unwrap();
    assert_eq!(
        updater.target("x"),
        Err(Error::NotFound("gone is not in the snapshot".into()))
    );
    drop(updater);
    assert_eq!(remote.log.len(), 4);
}

#[test]
fn without_consistent_snapshots_files_are_unversioned() {
    let mut repo = Repo::new();
    repo.consistent = false;
    let old = repo.signers["root"].clone();
    repo.publish_root(&old);
    repo.add_artifact("targets", "dir/f", b"bytes");
    repo.publish(&["targets"]);
    let mut cache = fresh(&repo);
    let mut remote = Served {
        repo: &repo,
        log: Vec::new(),
    };
    let mut updater = Updater::new(&mut remote, &mut cache, NOW).unwrap();
    updater.refresh().unwrap();
    let found = updater.target("dir/f").unwrap().unwrap();
    assert_eq!(updater.target_url(&found), "dir/f");
    drop(updater);
    assert_eq!(
        remote.log,
        [
            "2.root.json",
            "3.root.json",
            "timestamp.json",
            "snapshot.json",
            "targets.json"
        ]
    );
}
