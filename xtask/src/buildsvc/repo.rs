//! The git repository the build service works in, through the git command line.
//!
//! Tests use a snapshot: the working tree's files (tracked and untracked, not
//! ignored) copied into a fresh repository with no remotes, so the founder's
//! repository is never written and an uncommitted tree still tests. `run --edit`
//! uses a clone at `.live/repo`, outside `target/` so `cargo clean` keeps it.

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The identity build commits are made with.
const NAME: &str = "Kernel AI build service";
const EMAIL: &str = "build@kernel-ai.invalid";
/// Build inputs that must be committed before dev mode starts.
const INPUTS: &[&str] = &[
    "components",
    "compsdk",
    "kabi",
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    ".cargo",
];

/// Runs git in `dir` with the environment variables that would point it at
/// another repository removed; its trimmed output.
pub fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_CEILING_DIRECTORIES")
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[derive(Debug)]
pub struct Repo {
    pub root: PathBuf,
    /// The commit the run started from: the boot versions' source.
    pub base: String,
    /// Held while dev mode uses the clone.
    lock: Option<PathBuf>,
}

impl Drop for Repo {
    fn drop(&mut self) {
        if let Some(lock) = &self.lock {
            let _ = fs::remove_file(lock);
        }
    }
}

/// Canonical form of a path, for comparing git directories.
fn canonical(path: &str) -> PathBuf {
    let path = PathBuf::from(path);
    fs::canonicalize(&path).unwrap_or(path)
}

/// The repository must be its own, not the workspace's, and have no remotes but
/// (in dev mode) the workspace; its cargo configuration must be the workspace's,
/// because cargo merges the workspace's on top of it.
pub(crate) fn check(repo: &Path, workspace: &Path, remotes_allowed: bool) -> Result<(), String> {
    let own = canonical(&git(repo, &["rev-parse", "--absolute-git-dir"])?);
    let theirs = canonical(&git(workspace, &["rev-parse", "--absolute-git-dir"])?);
    let expected = canonical(&repo.join(".git").to_string_lossy());
    if own == theirs || own != expected {
        return Err(format!(
            "refusing to use {}: its git directory is {}",
            repo.display(),
            own.display()
        ));
    }
    if !remotes_allowed && !git(repo, &["remote"])?.is_empty() {
        return Err(format!("{} has remotes", repo.display()));
    }
    // Compared as text with CRLF taken as LF: a checkout may differ only there.
    let config = |root: &Path| {
        fs::read_to_string(root.join(".cargo").join("config.toml"))
            .ok()
            .map(|text| text.replace("\r\n", "\n"))
    };
    if config(repo) != config(workspace) {
        return Err(format!(
            "{}/.cargo/config.toml differs from the workspace's",
            repo.display()
        ));
    }
    Ok(())
}

/// Local settings that keep commits exactly the bytes built and keep the user's
/// hooks and signing out of a test repository.
fn configure_local(repo: &Path) -> Result<(), String> {
    let hooks = repo.join(".git").join("hooks");
    git(repo, &["config", "core.autocrlf", "false"])?;
    git(repo, &["config", "commit.gpgsign", "false"])?;
    git(
        repo,
        &["config", "core.hooksPath", &hooks.to_string_lossy()],
    )?;
    Ok(())
}

/// A fresh repository at `dest` holding the workspace's working tree.
pub fn snapshot(workspace: &Path, dest: &Path) -> Result<Repo, String> {
    if dest.exists() {
        fs::remove_dir_all(dest).map_err(|e| format!("cannot clear {}: {e}", dest.display()))?;
    }
    fs::create_dir_all(dest).map_err(|e| format!("cannot create {}: {e}", dest.display()))?;
    let listed = git(
        workspace,
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
    )?;
    for name in listed.split('\0').filter(|name| !name.is_empty()) {
        let from = workspace.join(name);
        if !from.is_file() {
            continue;
        }
        let to = dest.join(name);
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::copy(&from, &to).map_err(|e| format!("cannot copy {name}: {e}"))?;
    }
    git(dest, &["init", "-q"])?;
    configure_local(dest)?;
    git(dest, &["add", "-A"])?;
    commit(dest, "kai: snapshot of the working tree", &[])?;
    check(dest, workspace, false)?;
    let base = git(dest, &["rev-parse", "HEAD"])?;
    Ok(Repo {
        root: dest.to_path_buf(),
        base,
        lock: None,
    })
}

/// The dev clone at `dest`, updated to the workspace's HEAD.
pub fn dev(workspace: &Path, dest: &Path) -> Result<Repo, String> {
    let mut status = vec!["status", "--porcelain", "--"];
    status.extend_from_slice(INPUTS);
    let dirty = git(workspace, &status)?;
    if !dirty.is_empty() {
        return Err(format!(
            "commit the build inputs first; the boot versions must come from a commit:\n{dirty}"
        ));
    }
    if !dest.join(".git").exists() {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let output = Command::new("git")
            .args(["clone", "-q", "--no-hardlinks"])
            .arg(workspace)
            .arg(dest)
            .output()
            .map_err(|e| format!("cannot run git: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "git clone failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
    }
    let lock = dest.join(".git").join("kai.lock");
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock)
        .map(|_: File| ())
        .map_err(|_| {
            format!(
                "another xtask is using {}; if none is, delete {}",
                dest.display(),
                lock.display()
            )
        })?;
    let mut repo = Repo {
        root: dest.to_path_buf(),
        base: String::new(),
        lock: Some(lock),
    };
    git(dest, &["fetch", "-q", "origin", "HEAD"])?;
    git(
        dest,
        &["checkout", "-q", "--force", "--detach", "FETCH_HEAD"],
    )?;
    check(dest, workspace, true)?;
    repo.base = git(dest, &["rev-parse", "HEAD"])?;
    Ok(repo)
}

/// Commits what is staged with the build service's identity; the new commit.
fn commit(repo: &Path, message: &str, trailers: &[(&str, &str)]) -> Result<String, String> {
    let name = format!("user.name={NAME}");
    let email = format!("user.email={EMAIL}");
    let mut args = vec![
        "-c",
        name.as_str(),
        "-c",
        email.as_str(),
        "commit",
        "-q",
        "--allow-empty",
        "-m",
        message,
    ];
    let trailers: Vec<String> = trailers.iter().map(|(k, v)| format!("{k}: {v}")).collect();
    for trailer in &trailers {
        args.push("--trailer");
        args.push(trailer);
    }
    git(repo, &args)?;
    git(repo, &["rev-parse", "HEAD"])
}

impl Repo {
    /// The work tree at `commit`, with nothing else under `dir`.
    pub fn prepare(&self, commit: &str, dir: &str) -> Result<(), String> {
        git(
            &self.root,
            &["checkout", "-q", "--force", "--detach", commit],
        )?;
        git(&self.root, &["clean", "-fdxq", "--", dir])?;
        Ok(())
    }

    /// Writes each file of a source bundle, byte for byte.
    pub fn write_bundle(&self, bundle: &[u8]) -> Result<(), String> {
        let files = livecore::artifact::source_files(bundle).map_err(String::from)?;
        for (path, bytes) in files {
            editcore::rules::check_path(path)?;
            let to = self.root.join(path);
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            fs::write(&to, bytes).map_err(|e| format!("cannot write {path}: {e}"))?;
        }
        Ok(())
    }

    /// Commits the component's directory on top of the checked-out base.
    pub fn commit_build(
        &self,
        dir: &str,
        message: &str,
        trailers: &[(&str, &str)],
    ) -> Result<String, String> {
        git(&self.root, &["add", "-A", "--", dir])?;
        commit(&self.root, message, trailers)
    }

    /// Points `reference` at `sha` with a reflog entry; false when it already did.
    pub fn update_ref(&self, reference: &str, sha: &str, message: &str) -> Result<bool, String> {
        let current = git(&self.root, &["rev-parse", "-q", "--verify", reference]).ok();
        if current.as_deref() == Some(sha) {
            return Ok(false);
        }
        git(
            &self.root,
            &[
                "update-ref",
                "--create-reflog",
                "-m",
                message,
                reference,
                sha,
            ],
        )?;
        Ok(true)
    }

    /// Whether `sha` names a commit here.
    pub fn has_commit(&self, sha: &str) -> bool {
        let object = format!("{sha}^{{commit}}");
        git(&self.root, &["cat-file", "-e", &object]).is_ok()
    }
}
