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
/// e3 (part F's) on the base, the failed build nowhere, `kai/notes` back at the
/// base after e1, and e2's rollback recorded.
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
    if builds.lines().count() != 3 {
        return Err(fail(format!("expected three builds, found {builds:?}")));
    }
    let (_, e1) = one_ref(&repo, "refs/kai/builds/notes/e1-*").map_err(fail)?;
    let (_, e2) = one_ref(&repo, "refs/kai/builds/notes/e2-*").map_err(fail)?;
    let (_, e3) = one_ref(&repo, "refs/kai/builds/notes/e3-*").map_err(fail)?;
    if parent(&repo, &e1)? != base || parent(&repo, &e2)? != e1 || parent(&repo, &e3)? != base {
        return Err(fail(String::from(
            "e1 is not on the base, e2 not on e1, or e3 not on the base",
        )));
    }
    let lib3 = lib_at(&repo, &e3)?;
    if lib3.contains(", edited live") || !lib3.contains(TRAP) || lib3.contains(COMPILE_ERROR) {
        return Err(fail(String::from(
            "e3's source is not part F's edit on the base",
        )));
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
    if (built, failed) != (3, 1) || syncs < 3 {
        return Err(fail(format!(
            "the service built {built}, failed {failed} and synced {syncs} times; expected 3, 1 and 3"
        )));
    }
    if !git(&repo, &["status", "--porcelain"])?.is_empty() {
        return Err(fail(String::from("the work tree is not clean")));
    }
    println!("xtask: the edit case's repository holds e1, e2 and e3, kai/notes and the rollback");
    Ok(())
}

/// The refs a run could write, were it to reach the founder's repository: the
/// build service's own (`refs/kai/...`) and its component branches
/// (`refs/heads/kai/...`). Other refs are left out, because other sessions
/// sharing the repository push to their branches while a run goes on.
const RUN_REFS: [&str; 2] = ["refs/kai/", "refs/heads/kai/"];

/// What a run must leave as it was in the founder's repository.
#[derive(Debug, PartialEq, Eq)]
pub struct RealRepo {
    refs: String,
    head: String,
    status: String,
}

pub fn real_repo(root: &Path) -> Result<RealRepo, String> {
    let mut args = vec!["for-each-ref", "--format=%(refname) %(objectname)"];
    args.extend(RUN_REFS);
    Ok(RealRepo {
        refs: git(root, &args)?,
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

#[cfg(test)]
mod tests {
    use super::super::tests::scratch;
    use super::{git, real_repo, unchanged};

    #[test]
    fn the_guard_sees_the_service_refs_and_not_other_sessions_branches() {
        let dir = scratch("guard");
        let repo = dir.0.as_path();
        let run = |args: &[&str]| git(repo, args).unwrap();
        run(&["init", "-q"]);
        run(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "root",
        ]);
        let head = run(&["rev-parse", "HEAD"]);
        let head = head.trim();
        let before = real_repo(repo).unwrap();
        // Another session pushes its branch, or a remote ref moves: not the run.
        run(&["update-ref", "refs/heads/dane/other", head]);
        run(&["update-ref", "refs/remotes/origin/main", head]);
        assert_eq!(unchanged(repo, &before), Ok(()));
        // A build ref or a component branch is the run's doing.
        for name in ["refs/kai/builds/notes/e1", "refs/heads/kai/notes"] {
            let before = real_repo(repo).unwrap();
            run(&["update-ref", name, head]);
            assert!(unchanged(repo, &before).is_err(), "{name} went unseen");
        }
        // So is a change to the work tree.
        let before = real_repo(repo).unwrap();
        std::fs::write(repo.join("stray.txt"), "x").unwrap();
        assert!(unchanged(repo, &before).is_err());
    }
}
