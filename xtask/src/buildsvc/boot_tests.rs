//! The boot manifest against temporary git repositories and a fake compiler.

use super::boot::{self, Manifest, TIP, failed_ref, good_ref, label};
use super::build::Compiler;
use super::repo::{self, Repo, git};
use super::tests::{EDITED, Fake, LIB, Scratch, bundle, scratch, setup, sync, workspace, write};
use super::*;
use editcore::wire::SyncRequest;
use kabi::live::{KEEP_UPLOADS, MAX_VERSIONS, Reason};
use std::path::Path;

/// Counts the builds it passes to [`Fake`].
struct Counting(usize);

impl Compiler for Counting {
    fn compile(&mut self, repo: &Path, krate: &str, label: &str) -> Result<Vec<u8>, String> {
        self.0 += 1;
        Fake.compile(repo, krate, label)
    }
}

/// Commits `text` as `path` on top of `parent`; the new commit.
fn commit_file(repo: &Path, parent: &str, path: &str, text: &str) -> String {
    git(repo, &["checkout", "-q", "--force", "--detach", parent]).unwrap();
    write(repo, path, text);
    git(repo, &["add", "-A"]).unwrap();
    let identity = ["-c", "user.name=t", "-c", "user.email=t@t"];
    let mut args = identity.to_vec();
    args.extend(["-c", "commit.gpgsign=false", "commit", "-q", "-m", "edit"]);
    git(repo, &args).unwrap();
    git(repo, &["rev-parse", "HEAD"]).unwrap()
}

/// Commits `lib` as notes's lib.rs on top of `parent`.
fn commit_lib(repo: &Path, parent: &str, lib: &str) -> String {
    commit_file(repo, parent, LIB, lib)
}

fn point(repo: &Path, reference: &str, commit: &str) {
    git(repo, &["update-ref", reference, commit]).unwrap();
}

fn lib(text: &str) -> String {
    format!("fn about() -> &'static str {{\n    \"{text}\"\n}}\n")
}

struct Fixture {
    scratch: Scratch,
    repo: Repo,
}

fn fixture(name: &str) -> Fixture {
    let scratch = scratch(name);
    let ws = workspace(&scratch.0);
    let repo = repo::snapshot(&ws, &scratch.0.join("repo")).unwrap();
    Fixture { scratch, repo }
}

impl Fixture {
    fn root(&self) -> &Path {
        &self.repo.root
    }

    fn choose(&self, compiler: &mut dyn Compiler) -> Manifest {
        let cache = self.scratch.0.join("cache");
        boot::choose(&self.repo, compiler, &cache).unwrap()
    }
}

fn labels(manifest: &Manifest) -> (Option<String>, Option<String>) {
    let label = |boot: &Option<boot::Boot>| boot.as_ref().map(|b| b.label.clone());
    (label(&manifest.good), label(&manifest.pending))
}

#[test]
fn no_tip_a_tip_in_head_or_notes_changed_in_head_means_no_manifest() {
    let mut fixture = fixture("none");
    let base = fixture.repo.base.clone();
    assert_eq!(labels(&fixture.choose(&mut Fake)), (None, None));
    point(fixture.root(), TIP, &base);
    assert_eq!(labels(&fixture.choose(&mut Fake)), (None, None));
    let tip = commit_lib(fixture.root(), &base, EDITED);
    point(fixture.root(), TIP, &tip);
    // The founder's HEAD changed notes since kai/notes left it.
    let head = commit_lib(fixture.root(), &base, &lib("notes, by hand"));
    fixture.repo.base = head;
    assert_eq!(labels(&fixture.choose(&mut Fake)), (None, None));
    // A change elsewhere does not stop the tip.
    fixture.repo.base = commit_file(
        fixture.root(),
        &base,
        "README.md",
        "elsewhere
",
    );
    let manifest = fixture.choose(&mut Fake);
    assert_eq!(labels(&manifest), (None, Some(label(&tip))));
    // The work tree is back at HEAD.
    assert_eq!(
        git(fixture.root(), &["rev-parse", "HEAD"]).unwrap(),
        fixture.repo.base
    );
}

#[test]
fn a_version_head_runs_already_is_not_booted() {
    let mut fixture = fixture("inhead");
    let root = fixture.root().to_path_buf();
    let base = fixture.repo.base.clone();
    // kai/notes went back to 2 at a HEAD the founder then amended: no longer an
    // ancestor, but the same notes.
    let tip = commit_file(
        &root,
        &base,
        "README.md",
        "before the amend
",
    );
    point(&root, TIP, &tip);
    fixture.repo.base = commit_file(
        &root,
        &base,
        "README.md",
        "amended
",
    );
    assert_eq!(labels(&fixture.choose(&mut Fake)), (None, None));
    // HEAD merged the good version but not the tip after it: 2 runs good's notes.
    let good = commit_lib(&root, &base, EDITED);
    let tip = commit_lib(&root, &good, &lib("notes, again"));
    point(&root, TIP, &tip);
    point(&root, &good_ref("notes"), &good);
    fixture.repo.base = commit_file(
        &root,
        &good,
        "README.md",
        "merged
",
    );
    assert_eq!(
        labels(&fixture.choose(&mut Fake)),
        (None, Some(label(&tip)))
    );
}

#[test]
fn good_boots_and_the_tip_is_pending_unless_it_is_good_or_failed() {
    let fixture = fixture("choose");
    let root = fixture.root().to_path_buf();
    let base = fixture.repo.base.clone();
    let first = commit_lib(&root, &base, EDITED);
    point(&root, TIP, &first);
    point(&root, &good_ref("notes"), &first);
    let mut compiler = Counting(0);
    let manifest = fixture.choose(&mut compiler);
    assert_eq!(labels(&manifest), (Some(label(&first)), None));
    let good = manifest.good.as_ref().unwrap();
    assert_eq!(good.commit, first);
    assert_eq!(good.source, crate::sha256_hex(&bundle(EDITED)));
    let parsed = livecore::artifact::parse(&good.artifact).unwrap();
    assert_eq!(parsed.manifest.version, label(&first));
    // The next commit on the tip is pending; the good one comes from the cache.
    let second = commit_lib(&root, &first, &lib("notes, again"));
    point(&root, TIP, &second);
    let manifest = fixture.choose(&mut compiler);
    assert_eq!(
        labels(&manifest),
        (Some(label(&first)), Some(label(&second)))
    );
    assert_eq!(compiler.0, 2);
    assert_eq!(
        manifest.cmdline(),
        format!(
            "live_good=notes:{} live_pending=notes:{}",
            label(&first),
            label(&second)
        )
    );
    let (kernel, _) = livecore::boot::Manifest::parse(&manifest.cmdline());
    assert_eq!((kernel.good, kernel.pending), labels(&manifest));
    let paths: Vec<String> = manifest.modules().into_iter().map(|m| m.path).collect();
    assert_eq!(paths[1], format!("comp/notes/{}.wasm", label(&second)));
    // A tip that failed at a boot is not tried again.
    point(&root, &failed_ref("notes", &second), &second);
    assert_eq!(
        labels(&fixture.choose(&mut compiler)),
        (Some(label(&first)), None)
    );
    // Nor is a good one.
    point(&root, &failed_ref("notes", &first), &first);
    assert_eq!(labels(&fixture.choose(&mut compiler)), (None, None));
}

#[test]
fn a_good_version_off_the_tips_line_or_a_tip_that_does_not_build_is_left_out() {
    let fixture = fixture("leftout");
    let root = fixture.root().to_path_buf();
    let base = fixture.repo.base.clone();
    let good = commit_lib(&root, &base, EDITED);
    point(&root, &good_ref("notes"), &good);
    // kai/notes went back to 2 and on from there.
    let tip = commit_lib(&root, &base, &lib("notes, elsewhere"));
    point(&root, TIP, &tip);
    assert_eq!(
        labels(&fixture.choose(&mut Fake)),
        (None, Some(label(&tip)))
    );
    let broken = commit_lib(&root, &good, "fn x() { BROKEN }\n");
    point(&root, TIP, &broken);
    let manifest = fixture.choose(&mut Fake);
    assert_eq!(labels(&manifest), (Some(label(&good)), None));
    assert!(git(&root, &["status", "--porcelain"]).unwrap().is_empty());
}

#[test]
fn the_service_knows_the_manifests_versions_and_syncs_move_the_boot_refs() {
    let setup = setup("bootsync");
    let base = setup.service.base();
    let built_in =
        crate::sha256_hex(&components::bundle_source(&setup.repo, NOTES_RECIPE.dirs).unwrap());
    let tip = commit_lib(&setup.repo, &base, EDITED);
    point(&setup.repo, TIP, &tip);
    let cache = setup.repo.parent().unwrap().join("cache");
    let manifest = setup.service.boot_manifest(&cache).unwrap();
    let pending = manifest.pending.as_ref().unwrap();
    // An edit of the boot's version builds on its commit.
    let (_, body) = super::tests::build(&setup, "e1", &pending.label, &lib("notes, on c"));
    let e1 = wire::decode_built(&body).unwrap();
    assert_eq!(e1.result, BuildResult::Ok);
    let parent = git(&setup.repo, &["rev-parse", &format!("{}^", e1.commit)]).unwrap();
    assert_eq!(parent, tip);
    // Its commit moves kai/notes, where it is already, and the good ref.
    let commit = SyncRequest {
        component: "notes",
        label: &pending.label,
        outcome: Outcome::Commit,
        generation: 1,
        source: &pending.source,
        commit: "",
    };
    let moved = format!("kai-synced 1\nresult moved {TIP} {tip}\n");
    assert_eq!(sync(&setup, &commit), moved);
    let good = git(&setup.repo, &["rev-parse", &good_ref("notes")]).unwrap();
    assert_eq!(good, tip);
    // Another source under that label is not the version built.
    let other = SyncRequest {
        source: &"0".repeat(64),
        ..commit.clone()
    };
    assert_eq!(sync(&setup, &other), "kai-synced 1\nresult untracked\n");
    // Its rollback is recorded, and marks it failed.
    let rollback = SyncRequest {
        outcome: Outcome::Rollback(Reason::Trap),
        ..commit.clone()
    };
    assert!(sync(&setup, &rollback).contains("result recorded refs/kai/rollbacks/notes/"));
    let failed = git(&setup.repo, &["rev-parse", &failed_ref("notes", &tip)]).unwrap();
    assert_eq!(failed, tip);
    // Version 2 is known by its label too, and moves no boot ref.
    let back = SyncRequest {
        label: "2",
        source: &built_in,
        ..commit
    };
    assert_eq!(
        sync(&setup, &back),
        format!("kai-synced 1\nresult moved {TIP} {base}\n")
    );
    assert_eq!(
        git(&setup.repo, &["rev-parse", &good_ref("notes")]).unwrap(),
        tip
    );
}

#[test]
fn the_boot_drive_has_room_for_the_manifest_and_every_upload() {
    // The limit is per component: `notes`'s own versions.
    let boot_drive = crate::components::VERSIONS
        .iter()
        .filter(|v| v.recipe.component == "notes")
        .count();
    assert_eq!(boot_drive + 2 + KEEP_UPLOADS, 13);
    assert!(boot_drive + 2 + KEEP_UPLOADS <= MAX_VERSIONS);
}
