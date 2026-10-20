//! The boot manifest on the host before QEMU boots (M9d): a snapshot repository
//! whose `kai/notes` holds edits, built from their commits by the real build
//! service, run by the real host on wasmi against the store and ledger standing in
//! for the kernel, with each commit and rollback posted to git as the kernel's
//! outbox would. The `manifest` boot cases prepare their repositories and check
//! them afterwards with the same functions.

use crate::buildsvc::repo::git;
use crate::buildsvc::{BuildService, boot};
use crate::cases::TestCase;
use crate::esp::Module;
use crate::smoke::loader;
use editcore::command::{self, Command};
use editcore::scenario::{EDIT_ABOUT, EDIT_TRAP};
use editcore::wire::{self, SyncRequest, Synced};
use kabi::live::Reason;
use livecore::host::{Exit, Host};
use livecore::local::{Kernel, LocalEnv};
use livecore::settings::Settings;
use livecore::store::{Bytes, Component};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use wasmhost::WasmInstance;

const LIB: &str = "components/notes/src/lib.rs";
/// What the version with part B's edit says about itself.
const EDITED: &str = ", edited live";
/// Part D's edit, which traps on `count`.
const TRAPS: &str = "self.state.notes[self.state.notes.len()]";
/// How long git has to record a sync.
const SYNC_WAIT: Duration = Duration::from_secs(60);

/// Commits `edit`, an editor `s` command, to notes's lib.rs on top of `parent`,
/// and moves `kai/notes` to it, as a ledger commit of that edit would have.
pub fn commit_edit(service: &BuildService, parent: &str, edit: &str) -> Result<String, String> {
    let Ok(Command::Substitute {
        line: None,
        old,
        new,
    }) = command::parse(edit)
    else {
        return Err(format!("{edit:?} is not a substitution"));
    };
    let repo = service.repo_root();
    git(&repo, &["checkout", "-q", "--force", "--detach", parent])?;
    let path = repo.join(LIB);
    let text = fs::read_to_string(&path).map_err(|e| format!("cannot read {LIB}: {e}"))?;
    if text.matches(old).count() != 1 {
        return Err(format!("{old:?} is not in {LIB} exactly once"));
    }
    fs::write(&path, text.replacen(old, new, 1)).map_err(|e| format!("cannot write {LIB}: {e}"))?;
    git(&repo, &["add", "--", LIB])?;
    let message = format!("notes: {edit}");
    git(
        &repo,
        &[
            "-c",
            "user.name=Kernel AI boot test",
            "-c",
            "user.email=boot@kernel-ai.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            &message,
        ],
    )?;
    let commit = git(&repo, &["rev-parse", "HEAD"])?;
    git(&repo, &["update-ref", boot::TIP, &commit])?;
    git(
        &repo,
        &["checkout", "-q", "--force", "--detach", &service.base()],
    )?;
    Ok(commit)
}

/// Case `manifest`: part B's edit on `kai/notes`, which the boot swaps to.
pub fn prepare_manifest(service: &BuildService) -> Result<(), String> {
    commit_edit(service, &service.base(), EDIT_ABOUT).map(|_| ())
}

/// Case `manifest-good`: part B's edit is good, as if a boot had committed it,
/// and part D's edit on top is the tip, which the boot swaps to.
pub fn prepare_manifest_good(service: &BuildService) -> Result<(), String> {
    let about = commit_edit(service, &service.base(), EDIT_ABOUT)?;
    git(
        &service.repo_root(),
        &["update-ref", &boot::good_ref("notes"), &about],
    )?;
    commit_edit(service, &about, EDIT_TRAP).map(|_| ())
}

/// For a case with a build service: its refs prepared, then the boot manifest's
/// versions for the boot drive and the kernel arguments naming them.
pub fn manifest_for(
    case: &TestCase,
    service: &BuildService,
) -> Result<(Vec<Module>, String), String> {
    if let Some(prepare) = case.prepare {
        prepare(service)?;
    }
    let manifest = service.boot_manifest(&test_cache(service)?)?;
    Ok((manifest.modules(), manifest.cmdline()))
}

/// A run's own build cache, beside its snapshot: test commits are new every run,
/// so `target/live-boot`, which `run --edit` uses, would only fill up with them.
fn test_cache(service: &BuildService) -> Result<PathBuf, String> {
    let repo = service.repo_root();
    let cache = repo.parent().unwrap_or(&repo).join("live-boot");
    if cache.exists() {
        fs::remove_dir_all(&cache).map_err(|e| format!("cannot clear {}: {e}", cache.display()))?;
    }
    Ok(cache)
}

fn resolve(repo: &Path, reference: &str) -> Result<String, String> {
    git(repo, &["rev-parse", "-q", "--verify", reference])
        .map_err(|_| format!("{reference} is not there"))
}

fn lib_at(repo: &Path, commit: &str) -> Result<String, String> {
    git(repo, &["show", &format!("{commit}:{LIB}")])
}

/// After case `manifest`: the tip is part B's edit on the base, and the boot
/// that committed it moved the good ref there.
pub fn after_manifest(service: &BuildService) -> Result<(), String> {
    let repo = service.repo_root();
    let tip = resolve(&repo, boot::TIP)?;
    let good = resolve(&repo, &boot::good_ref("notes"))?;
    let parent = resolve(&repo, &format!("{tip}^"))?;
    if good != tip || parent != service.base() || !lib_at(&repo, &tip)?.contains(EDITED) {
        return Err(format!(
            "manifest: good is {good}, kai/notes {tip} on {parent}; expected part B's edit on the base"
        ));
    }
    println!("xtask: the good ref is {}", boot::label(&tip));
    Ok(())
}

/// After a boot whose pending version trapped: the tip is part D's edit on the
/// good one, recorded as failed, and good did not move.
pub fn after_manifest_good(service: &BuildService) -> Result<(), String> {
    let repo = service.repo_root();
    let tip = resolve(&repo, boot::TIP)?;
    let good = resolve(&repo, &boot::good_ref("notes"))?;
    let failed = resolve(&repo, &boot::failed_ref("notes", &tip))?;
    let parent = resolve(&repo, &format!("{tip}^"))?;
    if failed != tip || parent != good || !lib_at(&repo, &tip)?.contains(TRAPS) {
        return Err(format!(
            "manifest: failed {failed}, kai/notes {tip} on {parent}, good {good}; expected part D's edit on good, failed"
        ));
    }
    println!(
        "xtask: {} is recorded as failed; good stays {}",
        boot::label(&tip),
        boot::label(&good)
    );
    Ok(())
}

type NotesHost = Host<LocalEnv<WasmInstance>>;

fn failed(exit: Exit) -> String {
    format!("the host exited ({}): {}", exit.code, exit.why)
}

/// One boot on the host: the store with the boot drive's versions and the
/// manifest's, and notes started from the plan line the kernel would write.
struct Machine {
    kernel: Kernel,
    host: NotesHost,
    manifest: livecore::boot::Manifest,
}

fn boot_on(modules: &[Module], manifest: &boot::Manifest) -> Result<Machine, String> {
    let mut component = Component::new("notes");
    for module in modules.iter().chain(&manifest.modules()) {
        if let Some(label) = module
            .path
            .strip_prefix("comp/notes/")
            .and_then(|p| p.strip_suffix(".wasm"))
        {
            component.add_version(label, Bytes::shared(module.bytes.clone()))?;
        }
    }
    component.uploads.outbox.enable();
    let kernel = Kernel::new(component);
    let (parsed, _) = livecore::boot::Manifest::parse(&manifest.cmdline());
    let (version, fallback) = parsed.start();
    let line = format!(
        "{} {}",
        kabi::plan::notes(version, fallback),
        kabi::plan::CALLS_ONLY
    );
    let plan = supervise::parse_plan(&line);
    let notes = plan
        .services
        .first()
        .ok_or("the notes plan does not parse")?;
    let settings = Settings::parse(&notes.args)?;
    let env = kernel.env(3, loader(&settings));
    let host = Host::start(env, "notes", settings).map_err(failed)?;
    Ok(Machine {
        kernel,
        host,
        manifest: parsed,
    })
}

impl Machine {
    fn call(&mut self, request: &str) -> Result<String, String> {
        let reply = self.host.handle(0, request.as_bytes()).map_err(failed)?;
        Ok(String::from_utf8_lossy(&reply).into_owned())
    }

    fn ledger(&self) -> livecore::ledger::Ledger {
        self.kernel.component.borrow().ledger.clone()
    }

    /// notes started on `start`; then the kernel's swap, as its boot thread sends
    /// it.
    fn swap(&mut self, start: &str) -> Result<String, String> {
        let ledger = self.ledger();
        let active = ledger.active().unwrap_or("");
        if active != start {
            return Err(format!("notes started on {active}, not {start}"));
        }
        let label = self
            .manifest
            .swap_to(active, |label| ledger.is_bad(label).is_some())
            .ok_or("the manifest names nothing to swap to")?
            .to_string();
        let reply = self.call(&format!("ctl swap {label}"))?;
        if !reply.starts_with(&format!("ok probation {label} ")) {
            return Err(format!("the swap to {label} was answered {reply:?}"));
        }
        Ok(label)
    }

    /// Posts every job in the outbox to the build service, as the kernel's sync
    /// thread does through the edit service.
    fn sync(&mut self, service: &BuildService) -> Result<(), String> {
        loop {
            let job = self
                .kernel
                .component
                .borrow()
                .uploads
                .outbox
                .peek()
                .cloned();
            let Some(job) = job else { return Ok(()) };
            let body = wire::encode_sync(&SyncRequest {
                component: "notes",
                label: &job.label,
                outcome: job.outcome,
                generation: job.generation,
                source: &job.source,
                commit: &job.commit,
            });
            let sha = post_sync(service, &body)?;
            let mut component = self.kernel.component.borrow_mut();
            component.uploads.outbox.ack(job.seq, &sha);
        }
    }
}

/// POST /sync; the commit git recorded.
fn post_sync(service: &BuildService, body: &[u8]) -> Result<String, String> {
    let token = [("X-Kai-Token", service.token.as_str())];
    let request = llmproto::http::post(
        "127.0.0.1",
        "/sync",
        "application/octet-stream",
        &token,
        body,
    );
    let mut stream = TcpStream::connect(("127.0.0.1", service.port)).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(SYNC_WAIT))
        .map_err(|e| e.to_string())?;
    stream.write_all(&request).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    stream
        .read_to_end(&mut bytes)
        .map_err(|e| format!("no answer to a sync: {e}"))?;
    let response = llmproto::http::parse_response(&bytes, true)
        .map_err(|e| format!("a sync got a bad response: {e:?}"))?;
    match wire::decode_synced(&response.body)? {
        Synced::Moved { sha, .. } | Synced::Recorded { sha, .. } => Ok(sha.to_string()),
        Synced::Untracked => Err(String::from("git tracks no commit for a boot version")),
    }
}

/// Runs the boot manifest's boots on the `notes` versions among `modules`.
pub fn run(root: &Path, modules: &[Module]) -> Result<(), String> {
    let started = Instant::now();
    let service = crate::edit_smoke::service(root, "boot")?;
    let cache = test_cache(&service)?;
    // Boot 1: part B's edit is on kai/notes; notes starts on 2 and swaps to it.
    let about = commit_edit(&service, &service.base(), EDIT_ABOUT)?;
    let manifest = service.boot_manifest(&cache)?;
    let c_about = boot::label(&about);
    if manifest.labels() != (None, Some(c_about.as_str())) {
        return Err(format!("boot 1's manifest is {:?}", manifest.labels()));
    }
    let mut machine = boot_on(modules, &manifest)?;
    machine.swap("2")?;
    for _ in 0..3 {
        let reply = machine.call("about")?;
        if !reply.starts_with(&format!("ok notes {c_about} ")) || !reply.contains(EDITED) {
            return Err(format!("about answered {reply:?} on probation"));
        }
    }
    if machine.ledger().active() != Some(c_about.as_str()) {
        return Err(format!("{c_about} was not committed after 3 calls"));
    }
    machine.sync(&service)?;
    after_manifest(&service)?;
    // Boot 2: part D's edit on top; the swap to it traps on `count`.
    let trap = commit_edit(&service, &about, EDIT_TRAP)?;
    let manifest = service.boot_manifest(&cache)?;
    let c_trap = boot::label(&trap);
    if manifest.labels() != (Some(c_about.as_str()), Some(c_trap.as_str())) {
        return Err(format!("boot 2's manifest is {:?}", manifest.labels()));
    }
    let mut machine = boot_on(modules, &manifest)?;
    machine.swap(&c_about)?;
    let reply = machine.call("count")?;
    let ledger = machine.ledger();
    let rolled_back =
        ledger.active() == Some(c_about.as_str()) && ledger.is_bad(&c_trap) == Some(Reason::Trap);
    if !reply.starts_with("ok 0 notes") || !rolled_back {
        return Err(format!(
            "count answered {reply:?}; {c_trap} did not roll back by trap"
        ));
    }
    machine.sync(&service)?;
    after_manifest_good(&service)?;
    for line in machine.kernel.log.borrow().iter() {
        println!("  | {line}");
    }
    // Boot 3: the failed tip is not tried again, and a good version that will
    // not stage (here it is version 2's artifact) starts 2 instead.
    let mut manifest = service.boot_manifest(&cache)?;
    if manifest.labels() != (Some(c_about.as_str()), None) {
        return Err(format!("boot 3's manifest is {:?}", manifest.labels()));
    }
    let built_in = modules
        .iter()
        .find(|m| m.path == "comp/notes/2.wasm")
        .ok_or("no notes 2 on the boot drive")?;
    if let Some(good) = &mut manifest.good {
        good.artifact = built_in.bytes.clone();
    }
    let machine = boot_on(modules, &manifest)?;
    let ledger = machine.ledger();
    let said = machine
        .kernel
        .log
        .borrow()
        .iter()
        .any(|line| line.starts_with(&format!("cannot load version {c_about}: ")));
    if ledger.active() != Some("2") || !said {
        return Err(format!(
            "a good version that will not stage started {:?}",
            ledger.active()
        ));
    }
    for line in machine.kernel.log.borrow().iter() {
        println!("  | {line}");
    }
    println!(
        "xtask: the boot manifest passed on the host: {c_about} swapped in and good, {c_trap} rolled back and failed, a broken good version fell back to 2 ({} s)",
        started.elapsed().as_secs()
    );
    Ok(())
}
