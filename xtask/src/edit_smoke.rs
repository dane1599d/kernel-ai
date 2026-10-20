//! The M9b scenario on the host before QEMU boots: the edit service from editcore
//! talking to the real build service over loopback (real cargo builds, in its own
//! snapshot repository), the store and ledger standing in for the kernel, and
//! notes on wasmi. A failure here is in the editor, the builds or the store, not in
//! the kernel. It also warms the build cache the boot case then uses.

use crate::buildsvc::{self, BuildService, build::CargoCompiler, repo};
use crate::esp::Module;
use crate::smoke::loader;
use editcore::ipc::{self, At, Ipc};
use editcore::journal::Journal;
use editcore::scenario::{self, Applied, Driver};
use editcore::service::{Env, Service};
use editcore::wire::{self, SyncRequest};
use kabi::Error;
use kabi::live::Info;
use livecore::host::{Exit, Host};
use livecore::local::{Kernel, LocalEnv};
use livecore::scenario::Driver as LiveDriver;
use livecore::settings::Settings;
use livecore::store::{Bytes, Component};
use livecore::uploads::{self, SyncJob};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use wasmhost::WasmInstance;

/// A width and height like the boot case's console.
const SCREEN: (usize, usize) = (128, 48);

/// The settings the kernel's edit test plan gives the host.
pub fn test_settings() -> Result<Settings, String> {
    let plan = supervise::parse_plan(kabi::plan::NOTES_EDIT_TEST);
    let notes = plan
        .services
        .first()
        .ok_or("the edit test plan does not parse")?;
    Settings::parse(&notes.args)
}

/// The edit service's view of the fake kernel.
struct StoreEnv {
    kernel: Kernel,
    port: u16,
    builds: u64,
    /// The reply to the latest build, which the next upload must match, as the
    /// kernel requires.
    last_build: Option<Vec<u8>>,
    /// The notes host, lent while the edit service handles a message, so a
    /// relayed control request reaches it.
    host: Option<NotesHost>,
}

type NotesHost = Host<LocalEnv<WasmInstance>>;

/// What the kernel's COMP_CONTROL does: its own check of the request, then
/// `ctl <request>` to the notes host as the kernel.
fn relay(host: Option<&mut NotesHost>, component: &str, request: &str) -> Result<Vec<u8>, Error> {
    if !kabi::live::relayable(request) {
        return Err(Error::Invalid);
    }
    let host = host
        .filter(|_| component == "notes")
        .ok_or(Error::NotFound)?;
    let request = format!("{}{request}", kabi::live::CONTROL_PREFIX);
    host.handle(0, request.as_bytes()).map_err(|_| Error::Dead)
}

impl StoreEnv {
    /// What the kernel's COMP_PUT does: the upload must be the latest build's
    /// artifact, then pass the store's rules.
    fn upload(&mut self, artifact: &[u8], base: &str, commit: &str) -> Result<(), Error> {
        let reply = self.last_build.take();
        let bound = reply
            .as_deref()
            .map_or(Err("no build came before it"), |reply| {
                wire::check_built_response(reply, artifact, base, commit)
            });
        let mut component = self.kernel.component.borrow_mut();
        if bound.is_err() {
            component.uploads.rejected += 1;
            return Err(Error::Invalid);
        }
        let meta = uploads::meta(artifact);
        component
            .put(&meta, Arc::new(artifact.to_vec()), base, commit, 0)
            .map(|_| ())
    }
}

impl Env for StoreEnv {
    fn info(&mut self, component: &str) -> Result<Info, Error> {
        let store = self.kernel.component.borrow();
        if store.name != component {
            return Err(Error::NotFound);
        }
        Ok(store.info())
    }

    fn artifact(&mut self, _component: &str, label: &str) -> Result<Vec<u8>, Error> {
        let component = self.kernel.component.borrow();
        let label = match label {
            "" => component.ledger.active().ok_or(Error::NotFound)?,
            label => label,
        };
        Ok(component.bytes(label)?.as_slice().to_vec())
    }

    fn put(
        &mut self,
        _component: &str,
        artifact: &[u8],
        base: &str,
        commit: &str,
    ) -> Result<(), Error> {
        self.upload(artifact, base, commit)
    }

    fn build(&mut self, request: &[u8], timeout_ms: u64) -> Result<Vec<u8>, Error> {
        let builds = request.starts_with(b"POST /build ");
        if builds {
            self.builds += 1;
        }
        let mut stream =
            TcpStream::connect(("127.0.0.1", self.port)).map_err(|_| Error::Network)?;
        stream
            .set_read_timeout(Some(Duration::from_millis(timeout_ms)))
            .map_err(|_| Error::Network)?;
        stream.write_all(request).map_err(|_| Error::Network)?;
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .map_err(|_| Error::TimedOut)?;
        if builds {
            self.last_build = Some(response.clone());
        }
        Ok(response)
    }

    fn control(&mut self, component: &str, request: &str) -> Result<Vec<u8>, Error> {
        relay(self.host.as_mut(), component, request)
    }

    fn log(&mut self, text: &str) {
        println!("  | [edit] {text}");
    }
}

struct EditSmoke {
    kernel: Kernel,
    settings: Settings,
    pid: u64,
    host: Option<NotesHost>,
    service: Service,
    /// The build service's address and token, for a new edit service.
    builds: (String, String),
    env: StoreEnv,
    editing: bool,
    /// The kernel's journal of the session, and whether the edit service is new.
    journal: Journal,
    restarted: bool,
    restored: Option<String>,
}

fn failed(exit: Exit) -> String {
    format!("the host exited ({}): {}", exit.code, exit.why)
}

impl EditSmoke {
    fn start(&mut self) -> Result<(), String> {
        let env = self.kernel.env(self.pid, loader(&self.settings));
        self.host = Some(Host::start(env, "notes", self.settings.clone()).map_err(failed)?);
        Ok(())
    }

    fn send(&mut self, caller: u64, request: &str) -> Result<String, String> {
        let host = self.host.as_mut().ok_or("no host is running")?;
        let reply = host.handle(caller, request.as_bytes()).map_err(failed)?;
        Ok(String::from_utf8_lossy(&reply).into_owned())
    }

    /// A message to the edit service, as the kernel's edit session sends it: a
    /// new service gets the journal first, and each answer is journaled.
    fn editor(&mut self, caller: u64, message: &[u8]) -> String {
        self.env.host = self.host.take();
        // As the kernel, only before an open or a command, and only with a journal.
        let edits = matches!(ipc::decode_ipc(message), Ok(Ipc::Open { .. } | Ipc::Cmd(_)));
        if caller == 0 && edits && std::mem::take(&mut self.restarted) && !self.journal.is_empty() {
            let (service, env) = (&mut self.service, &mut self.env);
            let mut send = |m: &[u8]| Some(service.handle(env, 0, m).into_bytes());
            let restore = self.journal.restore(&mut send);
            println!("xtask: edit: {}", restore.notice);
            self.restarted = restore.again;
            self.restored = Some(restore.notice);
        }
        let reply = self.service.handle(&mut self.env, caller, message);
        if caller == 0
            && let Some(warning) = self.journal.record(message, reply.as_bytes())
        {
            println!("xtask: edit: {warning}");
        }
        let pinned = self.journal.session().map(|(_, label)| label.to_string());
        self.kernel.component.borrow_mut().uploads.pinned = pinned;
        self.host = self.env.host.take();
        reply
    }
}

impl LiveDriver for EditSmoke {
    fn call(&mut self, request: &str) -> Result<String, String> {
        self.send(0, request)
    }

    fn spoof(&mut self, request: &str) -> Result<String, String> {
        self.send(4242, request)
    }

    fn ledger(&mut self) -> livecore::ledger::Ledger {
        self.kernel.component.borrow().ledger.clone()
    }

    fn fetches(&mut self, label: &str) -> u64 {
        self.kernel.component.borrow().fetches(label)
    }

    fn host_pid(&mut self) -> u64 {
        self.pid
    }

    fn restart(&mut self) -> Result<(), String> {
        self.host = None;
        self.pid += 1;
        self.start()
    }

    fn log(&mut self, text: &str) {
        println!("xtask: {}", text.replacen("live: ", "edit: ", 1));
    }
}

impl Driver for EditSmoke {
    fn edit(&mut self, line: &str) -> Result<String, String> {
        if let Some(rest) = line.strip_prefix(":edit ") {
            let mut words = rest.split(' ');
            let component = words.next().unwrap_or("");
            let message = ipc::encode_ipc(&Ipc::Open {
                component,
                label: words.next(),
                at: None,
            });
            let reply = self.editor(0, &message);
            self.editing |= reply.starts_with("ok ");
            return Ok(reply);
        }
        if let Some(component) = line.strip_prefix(":fault ") {
            // As the kernel's :fault with a build service: open the trapped version
            // at its line.
            let located = self.kernel.component.borrow().ledger.last_located.clone();
            let Some(located) = located else {
                return Ok(format!("err {component} has no trap that names a line"));
            };
            let message = ipc::encode_ipc(&Ipc::Open {
                component,
                label: Some(&located.label),
                at: Some(At {
                    path: &located.at.path,
                    line: located.at.line,
                }),
            });
            let reply = self.editor(0, &message);
            self.editing |= reply.starts_with("ok ");
            return Ok(reply);
        }
        if line == ":quit" {
            self.editing = false;
            return Ok(String::from("ok left the editor"));
        }
        if !self.editing {
            return Err(format!("{line:?} typed outside edit mode"));
        }
        Ok(self.editor(0, &ipc::encode_ipc(&Ipc::Cmd(line))))
    }

    fn apply(&mut self, answer: &str) -> Result<Applied, String> {
        let component = self.kernel.component.borrow().clone();
        let shown = match uploads::proposal(&component, SCREEN.0 - 2, SCREEN.1) {
            Ok(shown) => shown,
            Err(why) => return Ok(Applied::Refused(why)),
        };
        let outcome = match answer {
            "y" | "yes" => self.send(0, &format!("ctl swap {}", shown.label))?,
            _ => String::from("not applied"),
        };
        Ok(Applied::Asked {
            view: shown.lines.join("\n"),
            outcome,
        })
    }

    fn revert(&mut self, args: &str) -> Result<String, String> {
        let n = args
            .split(' ')
            .nth(1)
            .map_or(Ok(1), str::parse)
            .map_err(|_| "bad n")?;
        let target = self
            .kernel
            .component
            .borrow()
            .ledger
            .revert_target(n)
            .map(String::from);
        match target {
            Some(label) => self.send(0, &format!("ctl swap {label}")),
            None => Ok(format!("err notes has no version {n} back")),
        }
    }

    fn component(&mut self) -> Component {
        self.kernel.component.borrow().clone()
    }

    fn builds(&mut self) -> u64 {
        self.env.builds
    }

    fn settle_sync(&mut self) -> Result<Vec<SyncJob>, String> {
        loop {
            let job = self
                .kernel
                .component
                .borrow()
                .uploads
                .outbox
                .peek()
                .cloned();
            let Some(job) = job else { break };
            let message = ipc::encode_ipc(&Ipc::Sync {
                seq: job.seq,
                job: SyncRequest {
                    component: "notes",
                    label: &job.label,
                    outcome: job.outcome,
                    generation: job.generation,
                    source: &job.source,
                    commit: &job.commit,
                },
            });
            let reply = self.editor(0, &message);
            let sha = ipc::parse_ack(reply.as_bytes(), job.seq)?;
            self.kernel
                .component
                .borrow_mut()
                .uploads
                .outbox
                .ack(job.seq, &sha);
        }
        Ok(self
            .kernel
            .component
            .borrow()
            .uploads
            .outbox
            .synced()
            .cloned()
            .collect())
    }

    fn holders(&mut self) -> Option<(Vec<String>, Vec<String>)> {
        None
    }

    fn spoof_edit(&mut self, request: &str) -> Result<String, String> {
        Ok(self.editor(4242, request.as_bytes()))
    }

    fn put_boot_artifact(&mut self, label: &str) -> Result<(), Error> {
        let bytes = self
            .kernel
            .component
            .borrow()
            .bytes(label)?
            .as_slice()
            .to_vec();
        let commit = "0".repeat(kabi::live::COMMIT_LEN);
        self.env.upload(&bytes, label, &commit)
    }

    fn relay(&mut self, request: &str) -> Result<String, Error> {
        relay(self.host.as_mut(), "notes", request)
            .map(|reply| String::from_utf8_lossy(&reply).into_owned())
    }

    fn restart_editor(&mut self) -> Result<(), String> {
        self.service = Service::new(&self.builds.0, &self.builds.1);
        self.restarted = true;
        Ok(())
    }

    fn restored(&mut self) -> Option<String> {
        self.restored.clone()
    }
}

/// A build service on a fresh snapshot of the working tree at `target/edit-test/<phase>`.
pub fn service(root: &Path, phase: &str) -> Result<BuildService, String> {
    let dest = root
        .join("target")
        .join("edit-test")
        .join(phase)
        .join("repo");
    let repo = repo::snapshot(root, &dest)?;
    let compiler = CargoCompiler {
        target_dir: buildsvc::target_dir(root),
    };
    buildsvc::start(repo, Box::new(compiler))
}

/// Runs the scenario on the `notes` versions among `modules`.
pub fn run(root: &Path, modules: &[Module]) -> Result<(), String> {
    let mut component = Component::new("notes");
    for module in modules {
        if let Some(label) = module
            .path
            .strip_prefix("comp/notes/")
            .and_then(|p| p.strip_suffix(".wasm"))
        {
            component.add_version(label, Bytes::shared(module.bytes.clone()))?;
        }
    }
    component.uploads.outbox.enable();
    let builds = service(root, "smoke")?;
    let kernel = Kernel::new(component);
    let host = format!("127.0.0.1:{}", builds.port);
    let mut smoke = EditSmoke {
        kernel: kernel.clone(),
        settings: test_settings()?,
        pid: 3,
        host: None,
        service: Service::new(&host, &builds.token),
        builds: (host.clone(), builds.token.clone()),
        env: StoreEnv {
            kernel,
            port: builds.port,
            builds: 0,
            last_build: None,
            host: None,
        },
        editing: false,
        journal: Journal::new(),
        restarted: false,
        restored: None,
    };
    smoke.start()?;
    let started = std::time::Instant::now();
    let result = scenario::run(&mut smoke);
    for line in smoke.kernel.log.borrow().iter() {
        println!("  | {line}");
    }
    result?;
    buildsvc::verify::after_edit_case(&builds)?;
    println!(
        "xtask: the edit scenario passed on the host ({} steps, {} s)",
        scenario::STEPS.len(),
        started.elapsed().as_secs()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_edit_test_plan_starts_on_version_2_with_calls_only_probation() {
        let settings = test_settings().unwrap();
        assert_eq!(
            (
                settings.version.as_str(),
                settings.probation_calls,
                settings.probation_ms
            ),
            ("2", 3, 0)
        );
    }
}
