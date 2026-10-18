//! The build service against temporary git repositories and a fake compiler: no
//! cargo builds.

use super::build::{Compiler, sanitize};
use super::repo::{self, git};
use super::*;
use editcore::wire::{BuildRequest, SyncRequest};
use kabi::live::Reason;
use livecore::artifact;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;

const LIB: &str = "components/notes/src/lib.rs";
const TOML: &str = "components/notes/Cargo.toml";

/// A directory removed when the test ends.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn scratch(name: &str) -> Scratch {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("kai-buildsvc-{}-{n}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    Scratch(dir)
}

fn write(root: &Path, path: &str, text: &str) {
    let to = root.join(path);
    fs::create_dir_all(to.parent().unwrap()).unwrap();
    fs::write(to, text).unwrap();
}

/// A workspace with one component, committed.
fn workspace(root: &Path) -> PathBuf {
    let ws = root.join("ws");
    write(&ws, ".cargo/config.toml", "[alias]\n");
    write(&ws, ".gitignore", "/target\n");
    write(&ws, TOML, "[package]\nname = \"comp-notes\"\n");
    write(&ws, LIB, "fn about() -> &'static str {\n    \"notes\"\n}\n");
    git(&ws, &["init", "-q"]).unwrap();
    git(&ws, &["add", "-A"]).unwrap();
    git(
        &ws,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "ws",
        ],
    )
    .unwrap();
    ws
}

/// Fails on a lib.rs holding BROKEN; otherwise returns a module.
struct Fake;

impl Compiler for Fake {
    fn compile(&mut self, repo: &Path, krate: &str, label: &str) -> Result<Vec<u8>, String> {
        assert_eq!(krate, "comp-notes");
        let lib = fs::read_to_string(repo.join(LIB)).unwrap();
        if lib.contains("BROKEN") {
            return Err(format!(
                "{}\\components\\notes\\src\\lib.rs:2:5: error[E0277]: broken in {label}\n",
                repo.display()
            ));
        }
        Ok(b"\0asm\x01\0\0\0".to_vec())
    }
}

struct Setup {
    _scratch: Scratch,
    service: BuildService,
    repo: PathBuf,
}

fn setup(name: &str) -> Setup {
    let scratch = scratch(name);
    let ws = workspace(&scratch.0);
    let repo = repo::snapshot(&ws, &scratch.0.join("repo")).unwrap();
    let root = repo.root.clone();
    let service = start(repo, Box::new(Fake)).unwrap();
    Setup {
        _scratch: scratch,
        service,
        repo: root,
    }
}

fn exchange(port: u16, raw: &[u8]) -> (u16, Vec<u8>) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.write_all(raw).unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    let parsed = llmproto::http::parse_response(&response, true).unwrap();
    (parsed.status, parsed.body)
}

fn post(setup: &Setup, path: &str, body: &[u8]) -> (u16, Vec<u8>) {
    let token = [("X-Kai-Token", setup.service.token.as_str())];
    let request = llmproto::http::post("h", path, "application/octet-stream", &token, body);
    exchange(setup.service.port, &request)
}

fn bundle(lib: &str) -> Vec<u8> {
    artifact::bundle(&[
        (TOML, b"[package]\nname = \"comp-notes\"\n"),
        (LIB, lib.as_bytes()),
    ])
}

const EDITED: &str = "fn about() -> &'static str {\n    \"notes, edited\"\n}\n";

fn build(setup: &Setup, label: &str, base: &str, lib: &str) -> (u16, Vec<u8>) {
    let body = wire::encode_build(&BuildRequest {
        component: "notes",
        label,
        base,
        bundle: &bundle(lib),
    });
    post(setup, "/build", &body)
}

fn sync(setup: &Setup, job: &SyncRequest<'_>) -> String {
    let (status, body) = post(setup, "/sync", &wire::encode_sync(job));
    assert_eq!(status, 200);
    String::from_utf8(body).unwrap()
}

#[test]
fn requests_need_the_token_and_a_sane_size_and_shape() {
    let setup = setup("token");
    let port = setup.service.port;
    let (status, _) = exchange(port, b"GET /health HTTP/1.1\r\n\r\n");
    assert_eq!(status, 403);
    let (status, _) = exchange(port, b"GARBAGE\r\n\r\n");
    assert_eq!(status, 400);
    let big = format!(
        "POST /build HTTP/1.1\r\nX-Kai-Token: {}\r\nContent-Length: {}\r\n\r\n",
        setup.service.token,
        MAX_BODY + 1
    );
    assert_eq!(exchange(port, big.as_bytes()).0, 413);
    assert_eq!(post(&setup, "/build", b"kai-build 9\n").0, 400);
    let token = [("X-Kai-Token", setup.service.token.as_str())];
    let health = format!(
        "GET /health HTTP/1.1\r\nX-Kai-Token: {}\r\n\r\n",
        token[0].1
    );
    assert_eq!(exchange(port, health.as_bytes()), (200, b"ok".to_vec()));
    assert_eq!(setup.service.counters.get().4, 4);
}

#[test]
fn a_good_build_is_committed_on_its_base_before_it_is_returned() {
    let setup = setup("good");
    let (status, body) = build(&setup, "e1", "2", EDITED);
    assert_eq!(status, 200);
    let built = wire::decode_built(&body).unwrap();
    assert_eq!(built.result, BuildResult::Ok);
    let parsed = artifact::parse(built.artifact).unwrap();
    assert_eq!(parsed.manifest.version, "e1");
    assert_eq!(parsed.source, bundle(EDITED));
    let base = setup.service.base();
    let parent = git(&setup.repo, &["rev-parse", &format!("{}^", built.commit)]).unwrap();
    assert_eq!(parent, base);
    let trailers = git(
        &setup.repo,
        &["log", "-1", "--format=%(trailers)", built.commit],
    )
    .unwrap();
    for key in [
        "Kai-Component: notes",
        "Kai-Label: e1",
        "Kai-Base: 2",
        "Kai-Build: ",
        "Kai-Source: ",
    ] {
        assert!(trailers.contains(key), "{trailers}");
    }
    let source12 = &crate::sha256_hex(&bundle(EDITED))[..12];
    let reference = format!("refs/kai/builds/notes/e1-{source12}");
    assert_eq!(
        git(&setup.repo, &["rev-parse", &reference]).unwrap(),
        built.commit
    );
    let shown = git(&setup.repo, &["show", &format!("{}:{LIB}", built.commit)]).unwrap();
    assert!(shown.contains("notes, edited"));
    // The next build stacks on the first.
    let second = "fn about() -> &'static str {\n    \"notes, again\"\n}\n";
    let (_, body) = build(&setup, "e2", "e1", second);
    let e2 = wire::decode_built(&body).unwrap();
    let parent = git(&setup.repo, &["rev-parse", &format!("{}^", e2.commit)]).unwrap();
    assert_eq!(parent, built.commit);
    assert_eq!(setup.service.counters.get().0, 2);
}

#[test]
fn unknown_bases_and_rule_breaks_are_refused() {
    let setup = setup("refused");
    let (_, body) = build(&setup, "e1", "e7", EDITED);
    let reply = wire::decode_built(&body).unwrap();
    assert_eq!(reply.result, BuildResult::Refused);
    assert!(String::from_utf8_lossy(reply.log).contains("unknown base e7"));
    let sneaky = "const X: &str = include_str!(\"/etc/passwd\");\n";
    let (_, body) = build(&setup, "e1", "2", sneaky);
    let reply = wire::decode_built(&body).unwrap();
    assert_eq!(reply.result, BuildResult::Refused);
    assert!(String::from_utf8_lossy(reply.log).contains("include_str"));
    let (_, body) = build(&setup, "2", "2", EDITED);
    assert_eq!(
        wire::decode_built(&body).unwrap().result,
        BuildResult::Refused
    );
    assert_eq!(setup.service.counters.get().2, 3);
    assert!(
        git(&setup.repo, &["for-each-ref", "refs/kai"])
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_failed_compile_leaves_a_clean_tree_and_no_ref() {
    let setup = setup("failed");
    let (_, body) = build(&setup, "e1", "2", "fn x() { BROKEN }\n");
    let reply = wire::decode_built(&body).unwrap();
    assert_eq!(reply.result, BuildResult::Failed);
    let log = String::from_utf8_lossy(reply.log);
    assert!(
        log.starts_with("components/notes/src/lib.rs:2:5: error[E0277]"),
        "{log}"
    );
    assert!(!log.contains(&setup.repo.display().to_string()));
    assert!(
        git(&setup.repo, &["status", "--porcelain"])
            .unwrap()
            .is_empty()
    );
    assert!(
        git(&setup.repo, &["for-each-ref", "refs/kai"])
            .unwrap()
            .is_empty()
    );
    assert_eq!(setup.service.counters.get().1, 1);
}

#[test]
fn a_commit_git_refuses_returns_no_artifact() {
    let setup = setup("hook");
    let hook = repo::hooks_dir(&setup.repo).join("pre-commit");
    fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let (_, body) = build(&setup, "e1", "2", EDITED);
    let reply = wire::decode_built(&body).unwrap();
    assert_eq!(reply.result, BuildResult::Failed);
    assert!(reply.artifact.is_empty());
    assert!(String::from_utf8_lossy(reply.log).starts_with("failed: git"));
    assert!(
        git(&setup.repo, &["status", "--porcelain"])
            .unwrap()
            .is_empty()
    );
}

#[test]
fn syncs_move_the_branch_record_rollbacks_and_repeat_safely() {
    let setup = setup("sync");
    let boot_source =
        crate::sha256_hex(&components::bundle_source(&setup.repo, NOTES_RECIPE.dirs).unwrap());
    let (_, body) = build(&setup, "e1", "2", EDITED);
    let e1 = wire::decode_built(&body).unwrap().commit.to_string();
    let source = crate::sha256_hex(&bundle(EDITED));
    let commit = SyncRequest {
        component: "notes",
        label: "e1",
        outcome: Outcome::Commit,
        generation: 1,
        source: &source,
        commit: &e1,
    };
    let moved = format!("kai-synced 1\nresult moved refs/heads/kai/notes {e1}\n");
    assert_eq!(sync(&setup, &commit), moved);
    assert_eq!(sync(&setup, &commit), moved);
    let reflog = git(
        &setup.repo,
        &["reflog", "show", "--format=%gs", "refs/heads/kai/notes"],
    )
    .unwrap();
    assert_eq!(reflog, "kai: notes e1 committed live (generation 1)");
    // A boot version whose source is the base's goes back to the base commit.
    let boot = SyncRequest {
        label: "2",
        generation: 2,
        source: &boot_source,
        commit: "",
        ..commit.clone()
    };
    let base = setup.service.base();
    assert!(sync(&setup, &boot).contains(&base));
    let rollback = SyncRequest {
        outcome: Outcome::Rollback(Reason::Trap),
        ..commit.clone()
    };
    let reference = format!("refs/kai/rollbacks/notes/e1-{}", &source[..12]);
    assert_eq!(
        sync(&setup, &rollback),
        format!("kai-synced 1\nresult recorded {reference} {e1}\n")
    );
    let reflog = git(&setup.repo, &["reflog", "show", "--format=%gs", &reference]).unwrap();
    assert!(reflog.contains("rolled back (trap)"));
    let unknown = SyncRequest {
        commit: "0123456789abcdef0123456789abcdef01234567",
        ..commit
    };
    assert_eq!(sync(&setup, &unknown), "kai-synced 1\nresult untracked\n");
    assert_eq!(setup.service.counters.get().3, 5);
}

#[test]
fn snapshots_are_their_own_repository_and_dev_needs_committed_inputs() {
    let scratch = scratch("repos");
    let ws = workspace(&scratch.0);
    let snap = repo::snapshot(&ws, &scratch.0.join("snap")).unwrap();
    assert!(git(&snap.root, &["remote"]).unwrap().is_empty());
    assert!(repo::check(&ws, &ws, false).is_err());
    write(&snap.root, ".cargo/config.toml", "[alias]\nx = \"y\"\n");
    assert!(
        repo::check(&snap.root, &ws, false)
            .unwrap_err()
            .contains("config.toml")
    );
    drop(snap);
    write(&ws, LIB, "fn changed() {}\n");
    let dev = scratch.0.join("live");
    assert!(
        repo::dev(&ws, &dev)
            .unwrap_err()
            .contains("commit the build inputs")
    );
    git(&ws, &["checkout", "-q", "--", "."]).unwrap();
    let first = repo::dev(&ws, &dev).unwrap();
    assert!(repo::dev(&ws, &dev).unwrap_err().contains("another xtask"));
    drop(first);
    let again = repo::dev(&ws, &dev).unwrap();
    assert_eq!(again.base, git(&ws, &["rev-parse", "HEAD"]).unwrap());
}

#[test]
fn compiler_output_loses_host_paths() {
    let repo = Path::new("C:\\work\\repo");
    let log =
        "C:\\work\\repo\\components\\notes\\src\\lib.rs:40:24: error[E0277]: x\n\x1b[1mnext\n";
    let clean = sanitize(log, repo, 1000);
    assert_eq!(
        clean,
        "components/notes/src/lib.rs:40:24: error[E0277]: x\n?[1mnext\n"
    );
    assert!(sanitize(&"x\n".repeat(100), repo, 10).len() <= 10);
}

#[test]
fn printed_command_lines_hide_the_token() {
    let line = "test=edit build=10.0.2.2:5000 build_token=0123456789abcdef llm=10.0.2.2:1";
    assert_eq!(
        redact(line),
        "test=edit build=10.0.2.2:5000 build_token=... llm=10.0.2.2:1"
    );
    let token = new_token();
    assert_eq!(token.len(), 32);
    assert!(token.bytes().all(|b| b.is_ascii_hexdigit()));
}

#[test]
fn the_edit_table_names_the_recipe_the_service_builds() {
    let notes = editcore::editable("notes").unwrap();
    assert_eq!(
        (notes.component, notes.krate, NOTES_RECIPE.dirs),
        (NOTES_RECIPE.component, NOTES_RECIPE.krate, &[notes.dir][..])
    );
}

#[test]
fn a_git_failure_during_a_sync_is_worth_retrying() {
    let setup = setup("busy");
    let (_, body) = build(&setup, "e1", "2", EDITED);
    let e1 = wire::decode_built(&body).unwrap().commit.to_string();
    let source = crate::sha256_hex(&bundle(EDITED));
    // Another git process holds the branch's lock.
    let lock = setup
        .repo
        .join(".git")
        .join("refs")
        .join("heads")
        .join("kai")
        .join("notes.lock");
    fs::create_dir_all(lock.parent().unwrap()).unwrap();
    fs::write(&lock, "").unwrap();
    let job = SyncRequest {
        component: "notes",
        label: "e1",
        outcome: Outcome::Commit,
        generation: 1,
        source: &source,
        commit: &e1,
    };
    let (status, _) = post(&setup, "/sync", &wire::encode_sync(&job));
    assert_eq!(status, 503);
    fs::remove_file(&lock).unwrap();
    assert_eq!(post(&setup, "/sync", &wire::encode_sync(&job)).0, 200);
}

#[test]
fn build_commits_use_the_service_hooks_not_the_users() {
    let setup = setup("hooks");
    let hooks = git(&setup.repo, &["config", "core.hooksPath"]).unwrap();
    assert_eq!(PathBuf::from(hooks), repo::hooks_dir(&setup.repo));
    assert_eq!(
        git(&setup.repo, &["rev-parse", "--show-object-format"]).unwrap(),
        "sha1"
    );
}
