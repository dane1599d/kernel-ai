//! The boot manifest (M9d): which versions of notes committed in git the next
//! boot runs. A ledger commit moves `kai/notes`, so its tip is the newest
//! committed version; a boot that swapped to a version and committed it moves the
//! good ref, and one that rolled it back writes a failed ref, so it is never tried
//! again. Each version named is built from its own commit's whole tree, labelled
//! `c-<sha12>`, and cached by commit.
//!
//! There is no manifest, and notes boots 2, when there is no tip, when HEAD holds
//! the tip already (merged, reverted to 2, or the same source), or when HEAD
//! changed `components/notes` since the two parted: those edits are the founder's
//! to merge. Otherwise the good version boots if it is on the tip's line, did not
//! fail and is not in HEAD, and the tip is pending unless it is good or failed.

use super::build::{Compiler, sanitize};
use super::repo::{Repo, git};
use crate::components::{self, NOTES_RECIPE};
use crate::esp::Module;
use livecore::artifact;
use livecore::boot::LABEL_PREFIX;
use std::fs;
use std::path::{Path, PathBuf};

/// The branch the ledger's commits move.
pub const TIP: &str = "refs/heads/kai/notes";
/// Lines of a failed build's log printed.
const LOG_LINES: usize = 12;

/// The last version of `component` a boot swapped to and committed.
pub fn good_ref(component: &str) -> String {
    format!("refs/kai/boot/good/{component}")
}

/// Where a boot's rollback of the version built from `commit` is recorded.
pub fn failed_ref(component: &str, commit: &str) -> String {
    format!(
        "refs/kai/boot/failed/{component}/{}",
        commit.get(..12).unwrap_or(commit)
    )
}

/// The label of the version built from `commit`.
pub fn label(commit: &str) -> String {
    format!("{LABEL_PREFIX}{}", commit.get(..12).unwrap_or(commit))
}

/// Where built versions are kept, one file per commit.
pub fn cache_dir(workspace: &Path) -> PathBuf {
    workspace.join("target").join("live-boot")
}

/// A version built from a commit.
pub struct Boot {
    pub label: String,
    pub commit: String,
    /// SHA-256 of its source bundle, as its manifest says.
    pub source: String,
    pub artifact: Vec<u8>,
}

#[derive(Default)]
pub struct Manifest {
    pub good: Option<Boot>,
    pub pending: Option<Boot>,
}

impl Manifest {
    pub fn versions(&self) -> impl Iterator<Item = &Boot> {
        self.good.iter().chain(&self.pending)
    }

    /// The good and pending labels.
    pub fn labels(&self) -> (Option<&str>, Option<&str>) {
        (
            self.good.as_ref().map(|b| b.label.as_str()),
            self.pending.as_ref().map(|b| b.label.as_str()),
        )
    }

    /// The kernel arguments that name the versions.
    pub fn cmdline(&self) -> String {
        let name = NOTES_RECIPE.component;
        let mut args = Vec::new();
        if let Some(good) = &self.good {
            args.push(format!("live_good={name}:{}", good.label));
        }
        if let Some(pending) = &self.pending {
            args.push(format!("live_pending={name}:{}", pending.label));
        }
        args.join(" ")
    }

    /// The boot modules that hold them.
    pub fn modules(&self) -> Vec<Module> {
        self.versions()
            .map(|boot| Module {
                path: format!("comp/{}/{}.wasm", NOTES_RECIPE.component, boot.label),
                bytes: boot.artifact.clone(),
            })
            .collect()
    }
}

/// The commit `reference` points to, if it does.
fn resolve(root: &Path, reference: &str) -> Option<String> {
    git(
        root,
        &[
            "rev-parse",
            "-q",
            "--verify",
            &format!("{reference}^{{commit}}"),
        ],
    )
    .ok()
}

fn is_ancestor(root: &Path, older: &str, newer: &str) -> bool {
    git(root, &["merge-base", "--is-ancestor", older, newer]).is_ok()
}

/// Whether `from` and `to` differ in notes's source.
fn notes_differ(root: &Path, from: &str, to: &str) -> Result<bool, String> {
    let mut diff = vec!["diff", "--name-only", from, to, "--"];
    diff.extend_from_slice(NOTES_RECIPE.dirs);
    Ok(!git(root, &diff)?.is_empty())
}

/// Whether HEAD runs `commit`'s notes already as version 2: it holds the commit,
/// or the same source (a squash, a cherry-pick, a revert to 2 before a rebase).
fn in_head(root: &Path, head: &str, commit: &str) -> Result<bool, String> {
    Ok(is_ancestor(root, commit, head) || !notes_differ(root, commit, head)?)
}

/// The good and pending commits, or why there is no manifest.
fn commits(repo: &Repo) -> Result<(Option<String>, Option<String>), String> {
    let (root, head) = (repo.root.as_path(), repo.base.as_str());
    let component = NOTES_RECIPE.component;
    let tip = resolve(root, TIP).ok_or("kai/notes has no commits")?;
    if in_head(root, head, &tip)? {
        return Err(String::from("HEAD holds kai/notes already"));
    }
    let fork = git(root, &["merge-base", head, &tip])?;
    if notes_differ(root, &fork, head)? {
        return Err(format!(
            "HEAD changed {} since kai/notes left it; to keep the live edits, git fetch \"{}\" kai/notes and merge FETCH_HEAD",
            NOTES_RECIPE.dirs.join(", "),
            root.display()
        ));
    }
    let failed = |commit: &str| {
        let failed = resolve(root, &failed_ref(component, commit)).as_deref() == Some(commit);
        if failed {
            println!(
                "xtask: boot: {} failed at a boot before; not tried again",
                label(commit)
            );
        }
        failed
    };
    let tip_failed = failed(&tip);
    // Good must be on the tip's line, not failed, and newer than what HEAD runs.
    let good = match resolve(root, &good_ref(component)) {
        Some(good) if good == tip => (!tip_failed).then_some(good),
        Some(good) if is_ancestor(root, &good, &tip) && !failed(&good) => Some(good),
        _ => None,
    };
    let good = match good {
        Some(good) if in_head(root, head, &good)? => None,
        good => good,
    };
    let pending = Some(tip).filter(|tip| Some(tip) != good.as_ref() && !tip_failed);
    Ok((good, pending))
}

/// The manifest for the next boot from `repo`, built with `compiler` and cached
/// in `cache`. A commit that does not build is left out. The work tree is left
/// at the base.
pub fn choose(repo: &Repo, compiler: &mut dyn Compiler, cache: &Path) -> Result<Manifest, String> {
    let (good, pending) = match commits(repo) {
        Ok(chosen) => chosen,
        Err(why) => {
            println!("xtask: boot: no manifest ({why}); notes boots 2");
            return Ok(Manifest::default());
        }
    };
    let mut build_one = |commit: Option<String>| {
        let commit = commit?;
        match build(repo, compiler, &commit, cache) {
            Ok(boot) => Some(boot),
            Err(why) => {
                println!("xtask: boot: {} does not build; left out:", label(&commit));
                for line in why.lines().take(LOG_LINES) {
                    println!("  | {line}");
                }
                None
            }
        }
    };
    let manifest = Manifest {
        good: build_one(good),
        pending: build_one(pending),
    };
    repo.prepare(&repo.base, NOTES_RECIPE.dirs[0])?;
    let (good, pending) = manifest.labels();
    println!(
        "xtask: boot: manifest good {} pending {}",
        good.unwrap_or("none"),
        pending.unwrap_or("none")
    );
    Ok(manifest)
}

/// The version built from `commit`, from the cache if it is there.
fn build(
    repo: &Repo,
    compiler: &mut dyn Compiler,
    commit: &str,
    cache: &Path,
) -> Result<Boot, String> {
    let label = label(commit);
    let cached = cache.join(format!("{commit}.wasm"));
    if let Some(boot) = fs::read(&cached)
        .ok()
        .and_then(|bytes| from_cache(&label, commit, bytes))
    {
        return Ok(boot);
    }
    repo.prepare(commit, NOTES_RECIPE.dirs[0])?;
    let module = compiler
        .compile(&repo.root, NOTES_RECIPE.krate, &label)
        .map_err(|log| sanitize(&log, &repo.root, 4096))?;
    let source = components::bundle_source(&repo.root, NOTES_RECIPE.dirs)?;
    let artifact = components::pack(&repo.root, &NOTES_RECIPE, &label, module, &source)?;
    if let Err(error) = fs::create_dir_all(cache).and_then(|()| fs::write(&cached, &artifact)) {
        println!("xtask: boot: cannot cache {label}: {error}");
    }
    Ok(Boot {
        label,
        commit: commit.to_string(),
        source: crate::sha256_hex(&source),
        artifact,
    })
}

/// A cached artifact, if it is whole and says it is `label` of notes.
fn from_cache(label: &str, commit: &str, artifact: Vec<u8>) -> Option<Boot> {
    let parsed = artifact::parse(&artifact).ok()?;
    let manifest = &parsed.manifest;
    let whole = manifest.component == NOTES_RECIPE.component
        && manifest.version == label
        && manifest.source == crate::sha256_hex(parsed.source);
    let source = manifest.source.clone();
    whole.then(|| Boot {
        label: label.to_string(),
        commit: commit.to_string(),
        source,
        artifact,
    })
}
