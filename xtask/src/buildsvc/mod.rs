//! The build service (M9b): a thread in the `cargo xtask` process that starts QEMU.
//! The guest's edit service reaches it at 10.0.2.2 with the token from the kernel
//! command line. It builds the source it is sent with the boot versions' toolchain,
//! profile and packer, commits that source to its repository before the artifact
//! leaves the host, and moves the component's branch when the kernel's ledger
//! commits a version. It listens on 127.0.0.1 only, and builds one at a time.

pub mod build;
pub mod repo;
#[cfg(test)]
mod tests;
pub mod verify;

use crate::components::{self, NOTES_RECIPE, Recipe};
use crate::httpd::{self, HttpError, Request};
use build::Compiler;
use editcore::wire::{self, BuildResult, Built, Origin, Synced};
use editcore::{MAX_BUNDLE, MAX_LOG, editable};
use livecore::uploads::Outcome;
use repo::Repo;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

/// Why a build of model-written source is refused.
pub const MODEL_REFUSED: &str = "the model wrote some of this source; it is built only in the build sandbox, which is not set up yet";
/// Largest request body: a bundle and its header.
const MAX_BODY: usize = MAX_BUNDLE + 1024;
/// Connections served at once. Each must send its headers within 5 s, so a
/// local process holding them all has to keep reconnecting; a determined one on
/// the founder's own machine can still deny service, which is accepted.
const MAX_HANDLERS: usize = 64;

#[derive(Default)]
pub struct Counters {
    pub built: AtomicU64,
    pub failed: AtomicU64,
    pub refused: AtomicU64,
    pub syncs: AtomicU64,
    /// Requests without the token, malformed or too large.
    pub bad: AtomicU64,
}

impl Counters {
    pub fn get(&self) -> (u64, u64, u64, u64, u64) {
        let read = |c: &AtomicU64| c.load(Ordering::SeqCst);
        (
            read(&self.built),
            read(&self.failed),
            read(&self.refused),
            read(&self.syncs),
            read(&self.bad),
        )
    }
}

struct State {
    repo: Repo,
    compiler: Box<dyn Compiler>,
    /// This run's uploads: label to the commit holding its source.
    builds: HashMap<String, String>,
    /// The hash of the boot version's source bundle at the base commit.
    boot_source: String,
}

pub struct BuildService {
    pub port: u16,
    pub token: String,
    pub counters: Arc<Counters>,
    state: Arc<Mutex<State>>,
}

impl BuildService {
    /// The repository's root, for the checks after a run.
    pub fn repo_root(&self) -> std::path::PathBuf {
        self.lock().repo.root.clone()
    }

    pub fn base(&self) -> String {
        self.lock().repo.base.clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    crate::sha256_hex(bytes)
}

/// A token for one run: 32 hex digits from the clock, the process and an address.
fn new_token() -> String {
    let mut hasher = Sha256::new();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    hasher.update(now.to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    let marker = 0u8;
    hasher.update((&raw const marker as usize).to_le_bytes());
    hasher.update(format!("{:?}", thread::current().id()).as_bytes());
    let digest = hasher.finalize();
    digest[..16].iter().map(|b| format!("{b:02x}")).collect()
}

/// Starts the service on a free local port, building in `repo`.
pub fn start(repo: Repo, compiler: Box<dyn Compiler>) -> Result<BuildService, String> {
    let root = repo.root.clone();
    let source = components::bundle_source(&root, NOTES_RECIPE.dirs)?;
    let state = State {
        repo,
        compiler,
        builds: HashMap::new(),
        boot_source: sha256_hex(&source),
    };
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| format!("cannot start the build service: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("the build service has no address: {e}"))?
        .port();
    let service = BuildService {
        port,
        token: new_token(),
        counters: Arc::new(Counters::default()),
        state: Arc::new(Mutex::new(state)),
    };
    let (token, counters, state) = (
        service.token.clone(),
        Arc::clone(&service.counters),
        Arc::clone(&service.state),
    );
    thread::spawn(move || {
        let active = Arc::new(AtomicUsize::new(0));
        for stream in listener.incoming().map_while(Result::ok) {
            // A few at a time: a build is one at a time anyway, and a flood of
            // connections must not pile up threads in the xtask process.
            if active.fetch_add(1, Ordering::SeqCst) >= MAX_HANDLERS {
                active.fetch_sub(1, Ordering::SeqCst);
                drop(stream);
                continue;
            }
            let (token, counters, state, active) = (
                token.clone(),
                Arc::clone(&counters),
                Arc::clone(&state),
                Arc::clone(&active),
            );
            thread::spawn(move || {
                if let Err(error) = serve(&stream, &token, &counters, &state) {
                    eprintln!("xtask: build service: {error}");
                }
                active.fetch_sub(1, Ordering::SeqCst);
            });
        }
    });
    Ok(service)
}

fn serve(
    stream: &TcpStream,
    token: &str,
    counters: &Counters,
    state: &Mutex<State>,
) -> std::io::Result<()> {
    let text = "text/plain";
    // The token is checked as soon as the headers are in, before any body.
    let allowed = |request: &Request| request.header("x-kai-token") == Some(token);
    let request = match httpd::read_request(stream, MAX_BODY, allowed) {
        Ok(request) => request,
        Err(HttpError::Io(error)) => return Err(error),
        Err(HttpError::Forbidden) => {
            counters.bad.fetch_add(1, Ordering::SeqCst);
            return httpd::respond(stream, "403 Forbidden", text, b"bad token");
        }
        Err(HttpError::TooLarge) => {
            counters.bad.fetch_add(1, Ordering::SeqCst);
            return httpd::respond(stream, "413 Payload Too Large", text, b"too large");
        }
        Err(HttpError::Malformed(why)) => {
            counters.bad.fetch_add(1, Ordering::SeqCst);
            return httpd::respond(stream, "400 Bad Request", text, why.as_bytes());
        }
    };
    let octets = "application/octet-stream";
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/health") => httpd::respond(stream, "200 OK", text, b"ok"),
        ("POST", "/build") => match build(&request, counters, state) {
            Ok(reply) => httpd::respond(stream, "200 OK", octets, &reply),
            Err(why) => bad(stream, counters, why),
        },
        ("POST", "/sync") => match sync(&request, counters, state) {
            Ok(Ok(reply)) => httpd::respond(stream, "200 OK", octets, &reply),
            Ok(Err(why)) => httpd::respond(stream, "503 Service Unavailable", text, why.as_bytes()),
            Err(why) => bad(stream, counters, why),
        },
        _ => httpd::respond(stream, "404 Not Found", text, b"not found"),
    }
}

fn bad(stream: &TcpStream, counters: &Counters, why: &str) -> std::io::Result<()> {
    counters.bad.fetch_add(1, Ordering::SeqCst);
    httpd::respond(stream, "400 Bad Request", "text/plain", why.as_bytes())
}

fn reply(result: BuildResult, log: &str) -> Vec<u8> {
    wire::encode_built(&Built {
        result,
        base: "",
        commit: "",
        log: log.as_bytes(),
        artifact: &[],
    })
}

/// POST /build: check, compile, pack, commit, then answer.
fn build(
    request: &Request,
    counters: &Counters,
    state: &Mutex<State>,
) -> Result<Vec<u8>, &'static str> {
    let job = wire::decode_build(&request.body)?;
    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let refuse = |why: String| {
        counters.refused.fetch_add(1, Ordering::SeqCst);
        eprintln!(
            "xtask: build service: refused {} {}: {why}",
            job.component, job.label
        );
        reply(BuildResult::Refused, &why)
    };
    // Source a model wrote is built only inside a build sandbox, and this host
    // has none yet: what the founder typed still builds, a draft does not.
    if job.origin == Origin::Model {
        return Ok(refuse(String::from(MODEL_REFUSED)));
    }
    let recipe = &NOTES_RECIPE;
    let Some(editable) = editable(job.component) else {
        return Ok(refuse(format!("{} cannot be edited", job.component)));
    };
    if recipe.component != editable.component || recipe.krate != editable.krate {
        return Ok(refuse(String::from(
            "the recipe and the edit table disagree",
        )));
    }
    let base = match state.builds.get(job.base) {
        Some(commit) => commit.clone(),
        None if job.base == "2" => state.repo.base.clone(),
        None => return Ok(refuse(format!("unknown base {}", job.base))),
    };
    if !kabi::live::is_upload_label(job.label) {
        return Ok(refuse(format!("{} is not an upload label", job.label)));
    }
    let started = Instant::now();
    let result = compile(&mut state, &job, recipe, editable.dir, &base);
    let seconds = started.elapsed().as_secs_f32();
    match result {
        Ok((commit, artifact)) => {
            counters.built.fetch_add(1, Ordering::SeqCst);
            println!(
                "xtask: build service: {} {} from {} built in {seconds:.1} s, commit {}",
                job.component,
                job.label,
                job.base,
                &commit[..12]
            );
            state.builds.insert(job.label.to_string(), commit.clone());
            Ok(wire::encode_built(&Built {
                result: BuildResult::Ok,
                base: job.base,
                commit: &commit,
                log: b"",
                artifact: &artifact,
            }))
        }
        Err(Failure::Refused(why)) => Ok(refuse(why)),
        Err(Failure::Failed(log)) => {
            counters.failed.fetch_add(1, Ordering::SeqCst);
            println!(
                "xtask: build service: {} {} from {} failed in {seconds:.1} s",
                job.component, job.label, job.base
            );
            Ok(reply(BuildResult::Failed, &log))
        }
    }
}

enum Failure {
    Refused(String),
    Failed(String),
}

fn compile(
    state: &mut State,
    job: &wire::BuildRequest<'_>,
    recipe: &Recipe,
    dir: &str,
    base: &str,
) -> Result<(String, Vec<u8>), Failure> {
    let repo = &state.repo;
    let root = repo.root.clone();
    let restore = |repo: &Repo| {
        if let Err(why) = repo.prepare(base, dir) {
            eprintln!("xtask: build service: cannot restore the tree: {why}");
        }
    };
    repo.prepare(base, dir).map_err(Failure::Failed)?;
    let base_bundle = components::bundle_source(&root, recipe.dirs).map_err(Failure::Failed)?;
    let editable =
        editable(job.component).ok_or_else(|| Failure::Refused("not editable".into()))?;
    editcore::rules::check(editable, &base_bundle, job.bundle).map_err(Failure::Refused)?;
    repo.write_bundle(job.bundle).map_err(Failure::Refused)?;
    let module = match state.compiler.compile(&root, recipe.krate, job.label) {
        Ok(module) => module,
        Err(log) => {
            restore(&state.repo);
            return Err(Failure::Failed(build::sanitize(&log, &root, MAX_LOG)));
        }
    };
    let artifact = components::pack(recipe, job.label, module, job.bundle);
    let source = sha256_hex(job.bundle);
    let message = format!(
        "{} {}: live edit from {}",
        job.component, job.label, job.base
    );
    let trailers = [
        ("Kai-Component", job.component),
        ("Kai-Label", job.label),
        ("Kai-Base", job.base),
        ("Kai-Build", &crate::sha256_hex(&artifact)),
        ("Kai-Source", &source),
    ];
    let repo = &state.repo;
    let committed = repo
        .commit_build(dir, &message, &trailers)
        .and_then(|commit| {
            let reference = format!(
                "refs/kai/builds/{}/{}-{}",
                job.component,
                job.label,
                &source[..12]
            );
            repo.update_ref(&reference, &commit, &message)?;
            Ok(commit)
        });
    match committed {
        Ok(commit) => Ok((commit, artifact)),
        Err(why) => {
            restore(repo);
            Err(Failure::Failed(format!("failed: {why}")))
        }
    }
}

/// POST /sync: a commit moves the component's branch; a rollback gets its own ref.
/// Repeating a sync changes nothing. The outer error is a bad request; the inner
/// one a git failure worth retrying.
type SyncResult = Result<Result<Vec<u8>, &'static str>, &'static str>;

fn sync(request: &Request, counters: &Counters, state: &Mutex<State>) -> SyncResult {
    let job = wire::decode_sync(&request.body)?;
    let state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    counters.syncs.fetch_add(1, Ordering::SeqCst);
    let repo = &state.repo;
    let sha = if !job.commit.is_empty() && repo.has_commit(job.commit) {
        job.commit.to_string()
    } else if job.commit.is_empty() && job.source == state.boot_source {
        repo.base.clone()
    } else {
        return Ok(Ok(wire::encode_synced(&Synced::Untracked)));
    };
    let (component, label) = (job.component, job.label);
    let (reference, message) = match job.outcome {
        Outcome::Commit => (
            format!("refs/heads/kai/{component}"),
            format!(
                "kai: {component} {label} committed live (generation {})",
                job.generation
            ),
        ),
        Outcome::Rollback(reason) => (
            format!(
                "refs/kai/rollbacks/{component}/{label}-{}",
                job.source.get(..12).unwrap_or("boot")
            ),
            format!("kai: {component} {label} rolled back ({})", reason.name()),
        ),
    };
    if let Err(why) = repo.update_ref(&reference, &sha, &message) {
        eprintln!("xtask: build service: sync failed: {why}");
        // Most likely a lock or a full disk for the moment: worth trying again.
        return Ok(Err("git could not record the sync now"));
    }
    let synced = match job.outcome {
        Outcome::Commit => Synced::Moved {
            reference: &reference,
            sha: &sha,
        },
        Outcome::Rollback(_) => Synced::Recorded {
            reference: &reference,
            sha: &sha,
        },
    };
    Ok(Ok(wire::encode_synced(&synced)))
}

/// The kernel command-line arguments that point the kernel at `service`.
pub fn cmdline(service: &BuildService) -> String {
    format!(
        "build=10.0.2.2:{} build_token={}",
        service.port, service.token
    )
}

/// `text` with any build token hidden, for printing.
pub fn redact(text: &str) -> String {
    text.split(' ')
        .map(|word| match word.strip_prefix("build_token=") {
            Some(_) => "build_token=...",
            None => word,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Where uploads are built, apart from the boot versions' output.
pub fn target_dir(workspace: &std::path::Path) -> std::path::PathBuf {
    workspace.join("target").join("edit-build")
}

/// The build service for `run --edit`, on the dev clone at `.live/repo`.
pub fn start_dev(workspace: &std::path::Path) -> Result<BuildService, String> {
    let repo = repo::dev(workspace, &workspace.join(".live").join("repo"))?;
    let compiler = build::CargoCompiler {
        target_dir: target_dir(workspace),
    };
    start(repo, Box::new(compiler))
}
