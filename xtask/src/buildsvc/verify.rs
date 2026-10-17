//! Checks after the edit scenario, on the build service's own repository, and that
//! the founder's repository was not touched by any of it.

use super::BuildService;
use super::repo::git;
use std::path::Path;

const LIB: &str = "components/notes/src/lib.rs";
/// Part C's edit; it never compiled, so no commit may hold it.
const COMPILE_ERROR: &str = "next_id += \"1\"";
/// Part D's edit, which compiled and then rolled back.
const TRAP: &str = "self.state.notes[self.state.notes.len()]";

fn lib_at(repo: &Path, commit: &str) -> Result<String, String> {
    git(repo, &["show", &format!("{commit}:{LIB}")])
}

fn one_ref(repo: &Path, pattern: &str) -> Result<(String, String), String> {
    let listed = git(
        repo,
        &["for-each-ref", "--format=%(refname) %(objectname)", pattern],
    )?;
    let refs: Vec<&str> = listed.lines().collect();
    match refs[..] {
        [line] => {
            let (name, sha) = line
                .split_once(' ')
                .ok_or("for-each-ref printed nonsense")?;
            Ok((name.to_string(), sha.to_string()))
        }
        _ => Err(format!(
            "expected one ref matching {pattern}, found {refs:?}"
        )),
    }
}

fn parent(repo: &Path, commit: &str) -> Result<String, String> {
    git(repo, &["rev-parse", &format!("{commit}^")])
}

/// The service's repository after the scenario: e1 and e2 built on each other,
/// the failed build nowhere, `kai/notes` back at the base after e1, and e2's
/// rollback recorded.
pub fn after_edit_case(service: &BuildService) -> Result<(), String> {
    let repo = service.repo_root();
    let base = service.base();
    let fail = |why: String| format!("edit case repository: {why}");
    let builds = git(
        &repo,
        &[
            "for-each-ref",
            "--format=%(refname)",
            "refs/kai/builds/notes",
        ],
    )?;
    if builds.lines().count() != 2 {
        return Err(fail(format!("expected two builds, found {builds:?}")));
    }
    let (_, e1) = one_ref(&repo, "refs/kai/builds/notes/e1-*").map_err(fail)?;
    let (_, e2) = one_ref(&repo, "refs/kai/builds/notes/e2-*").map_err(fail)?;
    if parent(&repo, &e1)? != base || parent(&repo, &e2)? != e1 {
        return Err(fail(String::from("e1 is not on the base, or e2 not on e1")));
    }
    let (lib1, lib2) = (lib_at(&repo, &e1)?, lib_at(&repo, &e2)?);
    if !lib1.contains(", edited live") || lib1.contains(TRAP) || lib1.contains(COMPILE_ERROR) {
        return Err(fail(String::from("e1's source is not part B's edit")));
    }
    if !lib2.contains(", edited live") || !lib2.contains(TRAP) || lib2.contains(COMPILE_ERROR) {
        return Err(fail(String::from("e2's source is not part D's edit on e1")));
    }
    for commit in git(&repo, &["rev-list", "--all"])?.lines() {
        if lib_at(&repo, commit).is_ok_and(|lib| lib.contains(COMPILE_ERROR)) {
            return Err(fail(format!(
                "commit {commit} holds the edit that did not compile"
            )));
        }
    }
    let branch = git(&repo, &["rev-parse", "refs/heads/kai/notes"])?;
    if branch != base {
        return Err(fail(format!("kai/notes is {branch}, not the base {base}")));
    }
    let reflog = git(
        &repo,
        &["reflog", "show", "--format=%gs", "refs/heads/kai/notes"],
    )?;
    let moves: Vec<&str> = reflog.lines().collect();
    let expected = [
        "kai: notes 2 committed live (generation 2)",
        "kai: notes e1 committed live (generation 1)",
    ];
    if moves != expected {
        return Err(fail(format!(
            "kai/notes moved {moves:?}, expected {expected:?}"
        )));
    }
    let (rollback, at) = one_ref(&repo, "refs/kai/rollbacks/notes/e2-*").map_err(fail)?;
    let why = git(&repo, &["reflog", "show", "--format=%gs", &rollback])?;
    if at != e2 || !why.contains("(trap)") {
        return Err(fail(format!(
            "{rollback} is {at} ({why}), expected e2 and a trap"
        )));
    }
    let (built, failed, _, syncs, _) = service.counters.get();
    if (built, failed) != (2, 1) || syncs < 3 {
        return Err(fail(format!(
            "the service built {built}, failed {failed} and synced {syncs} times; expected 2, 1 and 3"
        )));
    }
    if !git(&repo, &["status", "--porcelain"])?.is_empty() {
        return Err(fail(String::from("the work tree is not clean")));
    }
    println!("xtask: the edit case's repository holds e1 and e2, kai/notes and the rollback");
    Ok(())
}

/// What a run must leave as it was in the founder's repository.
#[derive(Debug, PartialEq, Eq)]
pub struct RealRepo {
    refs: String,
    head: String,
    status: String,
}

pub fn real_repo(root: &Path) -> Result<RealRepo, String> {
    Ok(RealRepo {
        refs: git(root, &["for-each-ref", "--format=%(refname) %(objectname)"])?,
        head: git(root, &["rev-parse", "HEAD"])?,
        status: git(root, &["status", "--porcelain"])?,
    })
}

/// The founder's repository is as it was before the run.
pub fn unchanged(root: &Path, before: &RealRepo) -> Result<(), String> {
    let after = real_repo(root)?;
    if &after != before {
        return Err(format!(
            "the run changed the repository at {}:\nbefore {before:?}\nafter  {after:?}",
            root.display()
        ));
    }
    Ok(())
}
