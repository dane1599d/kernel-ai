//! The TUF specification's attack list (1.0.37 §1.5.2, "Goals to protect
//! against specific attacks"), one test per attack, named for it (ADR 0008's
//! security review of `kaituf`, first part). A test the client workflow tests
//! already hold is linked, not copied:
//!
//! | attack | here | also |
//! |---|---|---|
//! | arbitrary installation | [`arbitrary_installation`] | `hostile.rs`: `a_refresh_over_one_mutated_file_fails_cleanly` |
//! | endless data | [`endless_data`] | |
//! | fast-forward | [`fast_forward_timestamp`], [`fast_forward_snapshot`] | |
//! | indefinite freeze | [`indefinite_freeze`] | `client.rs`: `expired_metadata_is_refused_and_not_kept` |
//! | malicious mirrors | [`malicious_mirrors`] | |
//! | mix-and-match | [`mix_and_match`] | `client.rs`: `hashes_named_in_meta_are_checked` |
//! | rollback | [`rollback`] | `client.rs`: `versions_never_go_back` (timestamp, snapshot, targets) |
//! | slow retrieval | does not apply here | see below |
//! | vulnerability to key compromise | [`threshold_of_keys_compromised`] | `client.rs`: `a_role_short_of_its_threshold_is_refused`, `two_signatures_by_one_key_are_refused_and_a_bad_extra_one_is_ignored`, `root_rotates_only_when_signed_by_the_old_keys_and_the_new` |
//! | wrong software | [`wrong_software`] | `client.rs`: `own_targets_are_the_roles_own_entries_its_delegation_covers`, `a_top_level_target_is_never_a_delegations` |
//!
//! **Slow retrieval does not apply to `kaituf`.** It has no transport: every
//! byte comes through the caller's [`Remote`], which is given a size bound per
//! file ([`endless_data`] checks the bounds). A server that sends slowly, or
//! not at all, is the transport's to cut off: `updatenet`'s HTTP timeout
//! (`updatenet/src/stage.rs`, `HttpError::Timeout`).
//!
//! Mirrors are the caller's too: `kaituf` refreshes over one [`Remote`]. What
//! it owes [`malicious_mirrors`] is that a bad mirror's files leave nothing
//! trusted behind, so the next mirror's refresh starts from the same state.

mod common;

use common::*;
use kaituf::Error;
use kaituf::updater::{
    ROOT_MAX, Remote, SNAPSHOT_MAX, TARGETS_MAX, TIMESTAMP_MAX, Updater, check_target,
};
use serde_json::{Value, json};

/// The client's view of `repo` after a refresh at `NOW`, for a search.
fn updater_over<'a>(remote: &'a mut Served<'_>, cache: &'a mut Memory) -> Updater<'a> {
    let mut updater = Updater::new(remote, cache, NOW).unwrap();
    updater.refresh().unwrap();
    updater
}

/// The `signed` part of a file `repo` published.
fn signed(repo: &Repo, name: &str) -> Value {
    let file: Value = serde_json::from_slice(&repo.files[name]).unwrap();
    file["signed"].clone()
}

/// Arbitrary installation: an attacker without the keys serves targets
/// metadata of its own, or bytes of its own for a listed target. Neither is
/// trusted, and a path no role lists is not found.
#[test]
fn arbitrary_installation() {
    let mut repo = Repo::new();
    repo.add_artifact("targets", "kernel.img", b"the real kernel");
    repo.publish(&["targets"]);
    let mut cache = fresh(&repo);
    refresh(&repo, &mut cache, NOW).0.unwrap();
    let kept = cache.clone();

    // Targets metadata at the version the snapshot names, listing the
    // attacker's kernel, signed by the attacker's key.
    let attacker = Signer::ed25519(200);
    repo.add_artifact("targets", "kernel.img", b"the attacker's");
    repo.publish(&["targets"]);
    let name = format!("{}.targets.json", repo.version["targets"]);
    let evil = envelope(&signed(&repo, &name), &[&attacker]);
    repo.files.insert(name, evil);
    let (result, _) = refresh(&repo, &mut cache, NOW);
    assert!(
        matches!(result, Err(Error::Threshold(ref w)) if w.starts_with("targets")),
        "{result:?}"
    );
    assert_eq!(cache.0["targets"], kept.0["targets"]);

    // Trusted metadata, and other bytes under the target's name.
    let mut good = Repo::new();
    good.add_artifact("targets", "kernel.img", b"the real kernel");
    good.publish(&["targets"]);
    let mut remote = Served {
        repo: &good,
        log: Vec::new(),
    };
    let mut cache = fresh(&good);
    let mut updater = updater_over(&mut remote, &mut cache);
    let found = updater.target("kernel.img").unwrap().unwrap();
    check_target(&found, b"the real kernel").unwrap();
    let swapped = check_target(&found, b"the attacker's!");
    assert!(matches!(swapped, Err(Error::Hash(_))), "{swapped:?}");
    assert_eq!(updater.target("backdoor.img").unwrap(), None);
}

/// A remote that serves `endless` as more bytes than it was asked for, and
/// logs the bound each fetch was given.
struct Endless<'a> {
    repo: &'a Repo,
    endless: &'a str,
    asked: Vec<(String, u64)>,
}

impl Remote for Endless<'_> {
    fn fetch(&mut self, name: &str, max: u64) -> Result<Option<Vec<u8>>, Error> {
        self.asked.push((name.into(), max));
        match name == self.endless {
            true => Ok(Some(vec![b' '; max as usize + 1])),
            false => Ok(self.repo.files.get(name).cloned()),
        }
    }
}

/// Endless data: every file is fetched with a bound, the length its
/// metadata names where one does and the role's limit where none does, and
/// a file past its bound is refused before it is parsed. A target's bytes
/// are checked against its length.
#[test]
fn endless_data() {
    for hashes_in_meta in [false, true] {
        let mut repo = Repo::new();
        repo.hashes_in_meta = hashes_in_meta;
        repo.publish(&["targets"]);
        let named = |name: &str, limit: u64| match hashes_in_meta {
            true => repo.files[name].len() as u64,
            false => limit,
        };
        let expected = [
            ("2.root.json".to_string(), ROOT_MAX),
            ("timestamp.json".to_string(), TIMESTAMP_MAX),
            (
                "2.snapshot.json".to_string(),
                named("2.snapshot.json", SNAPSHOT_MAX),
            ),
            (
                "2.targets.json".to_string(),
                named("2.targets.json", TARGETS_MAX),
            ),
        ];
        // Each bound as a refresh gives it.
        let mut remote = Endless {
            repo: &repo,
            endless: "",
            asked: Vec::new(),
        };
        let mut cache = fresh(&repo);
        Updater::new(&mut remote, &mut cache, NOW)
            .and_then(|mut u| u.refresh())
            .unwrap();
        assert_eq!(remote.asked, expected);

        // Each file served one byte past its bound.
        for (name, _) in &expected {
            let mut remote = Endless {
                repo: &repo,
                endless: name,
                asked: Vec::new(),
            };
            let mut cache = fresh(&repo);
            let result = Updater::new(&mut remote, &mut cache, NOW).and_then(|mut u| u.refresh());
            assert_eq!(result, Err(Error::TooLarge(name.clone())), "{name}");
            // What came before it verified and is kept; it is not.
            match name.split('.').rev().nth(1).unwrap() {
                "root" => assert_eq!(cached_version(&cache, "root"), Some(1)),
                role => assert!(!cache.0.contains_key(role), "{name} was kept"),
            }
        }
    }

    let mut repo = Repo::new();
    repo.add_artifact("targets", "model.gguf", b"weights");
    repo.publish(&["targets"]);
    let mut remote = Served {
        repo: &repo,
        log: Vec::new(),
    };
    let mut cache = fresh(&repo);
    let mut updater = updater_over(&mut remote, &mut cache);
    let found = updater.target("model.gguf").unwrap().unwrap();
    let longer = check_target(&found, b"weights and more");
    assert!(matches!(longer, Err(Error::Hash(_))), "{longer:?}");
}

/// Fast-forward, through the timestamp key: a compromised key signs a
/// timestamp at a far version, which the client takes and keeps. The
/// repository recovers by rotating the timestamp key in root: the kept copy
/// no longer verifies, so it no longer sets the floor (TUF 1.0.37 §5.3.11).
#[test]
fn fast_forward_timestamp() {
    let mut repo = Repo::new();
    let mut cache = fresh(&repo);
    refresh(&repo, &mut cache, NOW).0.unwrap();
    let snapshot = repo.version["snapshot"];
    let real = repo.version["timestamp"];
    repo.version.insert("timestamp".into(), 999);
    repo.publish_timestamp(json!({ "version": snapshot }));
    refresh(&repo, &mut cache, NOW).0.unwrap();
    assert_eq!(cached_version(&cache, "timestamp"), Some(1000));

    // Without a rotation, the real next timestamp is refused.
    repo.version.insert("timestamp".into(), real);
    repo.publish(&["targets"]);
    let (result, _) = refresh(&repo, &mut cache, NOW);
    assert!(matches!(result, Err(Error::Version(_))), "{result:?}");
    assert_eq!(cached_version(&cache, "timestamp"), Some(1000));

    // With the timestamp key rotated, it is taken.
    let old = repo.signers["root"].clone();
    repo.signers
        .insert("timestamp".into(), vec![Signer::ed25519(201)]);
    repo.publish_root(&old);
    repo.version.insert("timestamp".into(), real);
    repo.publish(&["targets"]);
    refresh(&repo, &mut cache, NOW).0.unwrap();
    assert_eq!(cached_version(&cache, "timestamp"), Some(real + 1));
}

/// Fast-forward, through the snapshot key: a compromised key signs a
/// snapshot naming targets at a far version, which the client keeps. Rotating
/// the snapshot key in root recovers, as above.
#[test]
fn fast_forward_snapshot() {
    let mut repo = Repo::new();
    let mut cache = fresh(&repo);
    // The attacker's snapshot, at the version the real timestamp names.
    let name = format!("{}.snapshot.json", repo.version["snapshot"]);
    let mut evil = signed(&repo, &name);
    evil["meta"]["targets.json"] = json!({ "version": 999 });
    let real = repo.files[&name].clone();
    let evil = envelope(&evil, &[&repo.signers["snapshot"][0]]);
    repo.files.insert(name.clone(), evil);
    let (result, _) = refresh(&repo, &mut cache, NOW);
    assert_eq!(result, Err(Error::NotFound("999.targets.json".into())));
    let kept: Value = serde_json::from_slice(&cache.0["snapshot"]).unwrap();
    assert_eq!(kept["signed"]["meta"]["targets.json"]["version"], 999);
    repo.files.insert(name, real);

    // Without a rotation, the real next snapshot is refused.
    repo.publish(&["targets"]);
    let (result, _) = refresh(&repo, &mut cache, NOW);
    assert!(
        matches!(result, Err(Error::Version(ref w)) if w.contains("older targets.json")),
        "{result:?}"
    );

    // With the snapshot key rotated, it is taken.
    let old = repo.signers["root"].clone();
    repo.signers
        .insert("snapshot".into(), vec![Signer::ed25519(202)]);
    repo.publish_root(&old);
    repo.publish(&["targets"]);
    refresh(&repo, &mut cache, NOW).0.unwrap();
    assert_eq!(
        cached_version(&cache, "targets"),
        Some(repo.version["targets"])
    );
}

/// Indefinite freeze: a mirror that keeps serving the same files, every one
/// validly signed, holds the client only until the first of them expires.
/// The cached copies expire with them.
#[test]
fn indefinite_freeze() {
    // The timestamp expires first, at 2027-02-01: NOW is 2027-01-15.
    let thaw = 1_801_440_000;
    let mut repo = Repo::new();
    repo.expires
        .insert("timestamp".into(), "2027-02-01T00:00:00Z".into());
    repo.publish(&["targets"]);
    let mut cache = fresh(&repo);
    refresh(&repo, &mut cache, NOW).0.unwrap();
    refresh(&repo, &mut cache, thaw - 1).0.unwrap();
    let kept = cache.clone();
    // The same files, the moment the timestamp expires and after.
    for now in [thaw, thaw + 86_400 * 365] {
        let (result, _) = refresh(&repo, &mut cache, now);
        assert_eq!(result, Err(Error::Expired("timestamp".into())), "at {now}");
        assert!(cache.0 == kept.0, "at {now}: the cache changed");
    }

    // Each role on its own: snapshot, targets, then root.
    for role in ["snapshot", "targets", "root"] {
        let mut repo = Repo::new();
        repo.expires
            .insert(role.into(), "2027-02-01T00:00:00Z".into());
        if role == "root" {
            repo.publish_root(&[]);
        }
        repo.publish(&["targets"]);
        let mut cache = fresh(&repo);
        refresh(&repo, &mut cache, NOW).0.unwrap();
        let (result, _) = refresh(&repo, &mut cache, thaw);
        assert_eq!(result, Err(Error::Expired(role.into())), "{role}");
    }
}

/// What a bad mirror does to the files it serves.
type Mirror<'a> = Box<dyn Fn(&mut Repo) + 'a>;

/// Malicious mirrors: a mirror that serves files of its own making, or old
/// ones, gets a refresh refused. All it leaves in the cache is what the
/// repository itself signed (a real newer timestamp it passed on, say), so a
/// good mirror's refresh then succeeds from it.
#[test]
fn malicious_mirrors() {
    let mut repo = Repo::new();
    repo.add_artifact("targets", "app.kpkg", b"v1");
    repo.publish(&["targets"]);
    let mut cache = fresh(&repo);
    refresh(&repo, &mut cache, NOW).0.unwrap();
    let old_files = repo.files.clone();
    repo.add_artifact("targets", "app.kpkg", b"v2");
    repo.publish(&["targets"]);
    let good = repo.files.clone();

    let attacker = Signer::ed25519(203);
    let bad_mirrors: Vec<(&str, Mirror)> = vec![
        // Its own timestamp.
        (
            "forged timestamp",
            Box::new(|r: &mut Repo| {
                let forged = envelope(&signed(r, "timestamp.json"), &[&attacker]);
                r.files.insert("timestamp.json".into(), forged);
            }),
        ),
        // The real files of an older release (the version the client has).
        (
            "older release",
            Box::new(|r: &mut Repo| r.files = old_files.clone()),
        ),
        // Truncated snapshot.
        (
            "truncated snapshot",
            Box::new(|r: &mut Repo| {
                let name = format!("{}.snapshot.json", r.version["snapshot"]);
                r.files.get_mut(&name).unwrap().truncate(10);
            }),
        ),
        // Nothing but the root.
        (
            "no metadata",
            Box::new(|r: &mut Repo| r.files.retain(|name, _| name.ends_with("root.json"))),
        ),
    ];
    for (what, make) in &bad_mirrors {
        let kept = cache.clone();
        make(&mut repo);
        let (result, _) = refresh(&repo, &mut cache, NOW);
        // The older release's files verify; they only fail to move the
        // client on, which `rollback` and `indefinite_freeze` bound.
        if *what != "older release" {
            assert!(result.is_err(), "{what}: {result:?}");
        }
        for (role, bytes) in &cache.0 {
            let real = good.values().any(|g| g == bytes) || kept.0.get(role) == Some(bytes);
            assert!(real, "{what}: the cached {role} is not the repository's");
        }
        assert!(cache.0["root"] == kept.0["root"], "{what}: root moved");
        repo.files = good.clone();
    }
    refresh(&repo, &mut cache, NOW).0.unwrap();
    assert_eq!(
        cached_version(&cache, "targets"),
        Some(repo.version["targets"])
    );
}

/// Mix-and-match: files of different releases served together. The snapshot
/// names each targets version, and the timestamp the snapshot's, so an older
/// file under the current name is refused.
#[test]
fn mix_and_match() {
    // An older targets file served under the current consistent name.
    let mut repo = Repo::new();
    repo.publish(&["targets"]);
    repo.files.insert(
        "2.targets.json".into(),
        repo.files["1.targets.json"].clone(),
    );
    let (result, _) = refresh(&repo, &mut fresh(&repo), NOW);
    assert_eq!(
        result,
        Err(Error::Version("targets 1 is not the snapshot's 2".into()))
    );

    // Without consistent snapshots: an older snapshot.json beside a newer
    // timestamp.json, and an older targets.json beside a newer snapshot.
    let mut repo = Repo::new();
    repo.consistent = false;
    let old = repo.signers["root"].clone();
    repo.publish_root(&old);
    repo.publish(&["targets"]);
    let older_snapshot = repo.files["snapshot.json"].clone();
    let older_targets = repo.files["targets.json"].clone();
    repo.publish(&["targets"]);
    let mut mixed = std::mem::take(&mut repo.files);
    mixed.insert("snapshot.json".into(), older_snapshot);
    repo.files = mixed.clone();
    let (result, _) = refresh(&repo, &mut fresh(&repo), NOW);
    assert_eq!(
        result,
        Err(Error::Version("snapshot 2 is not the timestamp's 3".into()))
    );
    repo.publish(&["targets"]);
    repo.files.insert("targets.json".into(), older_targets);
    let (result, _) = refresh(&repo, &mut fresh(&repo), NOW);
    assert_eq!(
        result,
        Err(Error::Version("targets 2 is not the snapshot's 4".into()))
    );
}

/// Rollback: what `versions_never_go_back` in `client.rs` does not cover.
/// An older root under the next root's name, a snapshot that drops a role
/// or names an older delegated one, and a client restarted from its cache.
#[test]
fn rollback() {
    // Root: the next version's name, an older version's bytes.
    let mut repo = Repo::new();
    let old = repo.signers["root"].clone();
    repo.publish_root(&old);
    let mut cache = fresh(&repo);
    refresh(&repo, &mut cache, NOW).0.unwrap();
    assert_eq!(cached_version(&cache, "root"), Some(2));
    repo.files
        .insert("3.root.json".into(), repo.files["1.root.json"].clone());
    let (result, _) = refresh(&repo, &mut cache, NOW);
    assert!(
        matches!(result, Err(Error::Version(ref w)) if w == "root 1 is not 3"),
        "{result:?}"
    );
    assert_eq!(cached_version(&cache, "root"), Some(2));

    // A delegated role, then the snapshot dropping it or naming it older.
    let mut repo = Repo::new();
    repo.delegate("targets", "pkgs", Signer::ed25519(204), &["*"], false);
    repo.publish(&["pkgs", "targets"]);
    repo.publish(&["pkgs"]);
    let mut cache = fresh(&repo);
    refresh(&repo, &mut cache, NOW).0.unwrap();
    assert_eq!(repo.version["pkgs"], 2);
    let current = repo.version["snapshot"];
    for (what, entry) in [("older", Some(json!({ "version": 1 }))), ("dropped", None)] {
        let name = format!("{}.snapshot.json", current + 1);
        let mut next = signed(&repo, &format!("{current}.snapshot.json"));
        next["version"] = json!(current + 1);
        match &entry {
            Some(e) => next["meta"]["pkgs.json"] = e.clone(),
            None => {
                next["meta"].as_object_mut().unwrap().remove("pkgs.json");
            }
        }
        let mut repo_next = Repo::new();
        repo_next.files = repo.files.clone();
        repo_next
            .files
            .insert(name, envelope(&next, &[&repo.signers["snapshot"][0]]));
        let ts = signed(&repo, "timestamp.json");
        let mut ts_next = ts.clone();
        ts_next["version"] = json!(ts["version"].as_u64().unwrap() + 1);
        ts_next["meta"]["snapshot.json"] = json!({ "version": current + 1 });
        repo_next.files.insert(
            "timestamp.json".into(),
            envelope(&ts_next, &[&repo.signers["timestamp"][0]]),
        );
        let mut cache = cache.clone();
        let (result, _) = refresh(&repo_next, &mut cache, NOW);
        let want = match what {
            "older" => "snapshot names an older pkgs.json",
            _ => "snapshot no longer names pkgs.json",
        };
        assert_eq!(result, Err(Error::Version(want.into())), "{what}");
        assert_eq!(cached_version(&cache, "snapshot"), Some(current), "{what}");
    }

    // A client restarted from its cache holds the same floor: the older
    // release's files are refused after a restart as before it.
    let mut repo = Repo::new();
    let older = repo.files.clone();
    repo.publish(&["targets"]);
    let mut cache = fresh(&repo);
    refresh(&repo, &mut cache, NOW).0.unwrap();
    repo.files = older;
    let mut restarted = cache.clone();
    let (result, _) = refresh(&repo, &mut restarted, NOW);
    assert!(
        matches!(result, Err(Error::Version(ref w)) if w == "timestamp 1 is older than 2"),
        "{result:?}"
    );
}

/// A key compromise below the threshold: an attacker holding some of a
/// role's keys, fewer than its threshold, cannot sign for it, and one key
/// listed under two key IDs is still one key.
#[test]
fn threshold_of_keys_compromised() {
    // Targets needs two of its two keys; the attacker holds one.
    let mut repo = Repo::new();
    let second = Signer::ed25519(205);
    repo.signers.get_mut("targets").unwrap().push(second);
    repo.thresholds.insert("targets".into(), 2);
    let old = repo.signers["root"].clone();
    repo.publish_root(&old);
    repo.publish(&["targets"]);
    let mut cache = fresh(&repo);
    refresh(&repo, &mut cache, NOW).0.unwrap();
    repo.add_artifact("targets", "kernel.img", b"the attacker's");
    repo.publish(&["targets"]);
    let name = format!("{}.targets.json", repo.version["targets"]);
    let held = repo.signers["targets"][0].clone();
    let evil = envelope(&signed(&repo, &name), &[&held]);
    repo.files.insert(name, evil);
    let (result, _) = refresh(&repo, &mut cache, NOW);
    assert_eq!(
        result,
        Err(Error::Threshold(
            "targets has 1 valid signatures of the 2 it needs".into()
        ))
    );

    // Root lists the timestamp key twice, under its own ID and an alias,
    // with a threshold of two: one key's two signatures are one key's.
    let mut repo = Repo::new();
    let key = repo.signers["timestamp"][0].clone();
    let mut root = repo.root_signed();
    root["version"] = json!(2);
    root["keys"]["alias"] = key.public.clone();
    root["roles"]["timestamp"] = json!({ "keyids": [key.id(), "alias"], "threshold": 2 });
    repo.files.insert(
        "2.root.json".into(),
        envelope(&root, &[&repo.signers["root"][0]]),
    );
    let ts = signed(&repo, "timestamp.json");
    let message = kaituf::canonical::canonical(&ts).unwrap();
    let sig = key.sign(&message);
    let doubled = json!({
        "signed": ts,
        "signatures": [{ "keyid": key.id(), "sig": sig }, { "keyid": "alias", "sig": sig }],
    });
    repo.files.insert(
        "timestamp.json".into(),
        serde_json::to_vec(&doubled).unwrap(),
    );
    let (result, _) = refresh(&repo, &mut fresh(&repo), NOW);
    assert!(
        matches!(result, Err(Error::Threshold(ref w)) if w.starts_with("timestamp")),
        "{result:?}"
    );
}

/// Wrong software: another target's bytes, validly signed for it, served for
/// this one; and a delegated role naming a path its delegation does not give
/// it, which the search never asks it for.
#[test]
fn wrong_software() {
    let mut repo = Repo::new();
    repo.add_artifact("targets", "editor.kpkg", b"the editor");
    repo.add_artifact("targets", "shell.kpkg", b"the shell!");
    repo.delegate(
        "targets",
        "models",
        Signer::ed25519(206),
        &["*.gguf"],
        false,
    );
    repo.add_artifact("models", "kernel.img", b"a model role's kernel");
    repo.publish(&["models", "targets"]);
    let mut remote = Served {
        repo: &repo,
        log: Vec::new(),
    };
    let mut cache = fresh(&repo);
    let mut updater = updater_over(&mut remote, &mut cache);
    let editor = updater.target("editor.kpkg").unwrap().unwrap();
    // The same length, so the hash is what refuses it.
    assert_eq!(editor.length, b"the shell!".len() as u64);
    let other = check_target(&editor, b"the shell!");
    assert!(matches!(other, Err(Error::Hash(_))), "{other:?}");
    assert_eq!(updater.target("kernel.img").unwrap(), None);
    assert_eq!(updater.top_level_target("kernel.img").unwrap(), None);
    drop(updater);
    assert!(
        !remote.log.iter().any(|n| n.contains("models")),
        "{:?}",
        remote.log
    );
}
