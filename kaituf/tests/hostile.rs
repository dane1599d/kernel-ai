//! Hostile metadata (ADR 0008's change list: the TUF JSON reader joins the
//! `testkit/hostile.rs` harness): mutated, truncated and field-swept copies of
//! a valid repository's files. Each must parse or be refused, never panic, and
//! a refresh over a repository with one file mutated must fail or succeed
//! cleanly, keeping only what verifies.

#[path = "../../testkit/hostile.rs"]
mod hostile;

mod common;

use common::*;
use hostile::{fuzz, peak_alloc, truncations};
use kaituf::meta::{Root, Snapshot, Targets, Timestamp};
use kaituf::trusted::TrustedSet;

/// A repository with a delegation, an artifact and P-256 and Ed25519 keys.
fn repo() -> Repo {
    let mut repo = Repo::new();
    repo.signers
        .insert("snapshot".into(), vec![Signer::p256(3, true, false)]);
    let old = repo.signers["root"].clone();
    repo.publish_root(&old);
    repo.hashes_in_meta = true;
    repo.delegate("targets", "pkgs", Signer::ed25519(40), &["*.kpkg"], false);
    repo.add_artifact("pkgs", "notes-1.0.0.kpkg", b"notes");
    repo.publish(&["pkgs", "targets"]);
    repo
}

fn parse_all(bytes: &[u8]) {
    let _ = Root::parse(bytes);
    let _ = Timestamp::parse(bytes);
    let _ = Snapshot::parse(bytes);
    let _ = Targets::parse(bytes);
    let _ = TrustedSet::new(bytes, NOW);
}

#[test]
fn mutated_metadata_never_panics() {
    let repo = repo();
    for (i, bytes) in repo.files.values().enumerate() {
        fuzz(bytes, 0x7f00 + i as u64, 400, parse_all);
        truncations(bytes, |_, cut| parse_all(cut));
    }
}

#[test]
fn a_refresh_over_one_mutated_file_fails_cleanly() {
    let base = repo();
    let names: Vec<String> = base.files.keys().cloned().collect();
    let mut refused = 0;
    for (i, name) in names.iter().enumerate() {
        fuzz(&base.files[name], 0xbad0 + i as u64, 60, |mutated| {
            let mut repo = repo();
            repo.files.insert(name.clone(), mutated.to_vec());
            // The trusted root is the unmutated one.
            let mut cache = fresh(&base);
            let (result, _) = refresh(&repo, &mut cache, NOW);
            if result.is_err() {
                refused += 1;
            }
            // Whatever was kept verifies again from scratch.
            if let Some(root) = cache.0.get("root") {
                assert!(TrustedSet::new(root, NOW).is_ok());
            }
        });
    }
    assert!(refused > 0);
}

#[test]
fn deep_and_wide_json_is_refused_within_bounds() {
    // serde_json stops at 128 levels of nesting.
    let deep = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
    assert!(Root::parse(deep.as_bytes()).is_err());
    let wide = format!(
        "{{\"signed\":{{\"keys\":[{}]}},\"signatures\":[]}}",
        "0,".repeat(50_000) + "0"
    );
    let (result, peak) = peak_alloc(|| Root::parse(wide.as_bytes()).is_err());
    assert!(result);
    assert!(peak < 32 << 20, "{peak} bytes");
}
