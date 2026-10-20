//! A stand-in for the kernel and the build service, for the service tests.

use super::*;
use crate::journal::{Journal, Restore};
use crate::service::{Env, Service};
use crate::wire::{self, BuildResult, Built, Synced};
use alloc::vec;
use kabi::Error;
use kabi::live::{Info, put_label};

pub(super) const COMMIT: &str = "1a2b3c4d5e6f0123456789abcdef0123456789ab";
pub(super) const TOKEN: &str = "0123456789abcdef0123456789abcdef";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Host {
    Builds,
    Fails,
    WrongLabel,
    WrongSource,
    Forbidden,
    Garbage,
}

pub(super) struct Fake {
    active: &'static str,
    next: &'static str,
    pub(super) artifacts: Vec<(&'static str, Vec<u8>)>,
    pub(super) puts: Vec<(String, Vec<u8>, String, String)>,
    pub(super) put_result: Result<(), Error>,
    pub(super) host: Host,
    pub(super) requests: Vec<Vec<u8>>,
    pub(super) controls: Vec<(String, String)>,
    pub(super) control_reply: Result<&'static str, Error>,
    /// The background build the kernel holds.
    pub(super) job: Option<Job>,
    /// Polls a new build answers Busy before its result is in.
    pub(super) busy_polls: u32,
    /// What a new build's exchange comes to, when it fails.
    pub(super) exchange: Result<(), Error>,
    /// Each poll: whether it took.
    pub(super) polls: Vec<bool>,
    /// Polls answered Fault, the reply kept, before the others.
    pub(super) fault_polls: u32,
    pub(super) logs: Vec<String>,
}

/// A background build in the kernel: its result, after `busy` more polls.
pub(super) struct Job {
    pub(super) reply: Result<Vec<u8>, Error>,
    pub(super) busy: u32,
}

impl Fake {
    pub(super) fn new(lib: &str) -> Self {
        Self {
            active: "2",
            next: "e1",
            artifacts: vec![("2", artifact("notes", "2", &small(lib)))],
            puts: Vec::new(),
            put_result: Ok(()),
            host: Host::Builds,
            requests: Vec::new(),
            controls: Vec::new(),
            control_reply: Ok("ok try e1: import ok"),
            job: None,
            busy_polls: 0,
            exchange: Ok(()),
            polls: Vec::new(),
            fault_polls: 0,
            logs: Vec::new(),
        }
    }

    pub(super) fn respond(&self, request: &[u8]) -> Vec<u8> {
        let split = request.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let head = String::from_utf8_lossy(&request[..split]);
        let body = &request[split + 4..];
        let http = |status: &str, body: &[u8]| {
            let mut out = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .into_bytes();
            out.extend_from_slice(body);
            out
        };
        if self.host == Host::Forbidden || !head.contains(&format!("X-Kai-Token: {TOKEN}")) {
            return http("403 Forbidden", b"bad token");
        }
        if self.host == Host::Garbage {
            return http("200 OK", b"kai-built 9\n");
        }
        if head.starts_with("POST /sync ") {
            let job = wire::decode_sync(body).unwrap();
            let synced = Synced::Moved {
                reference: "refs/heads/kai/notes",
                sha: if job.commit.is_empty() {
                    COMMIT
                } else {
                    job.commit
                },
            };
            return http("200 OK", &wire::encode_synced(&synced));
        }
        let request = wire::decode_build(body).unwrap();
        assert_eq!(request.origin, wire::Origin::Founder);
        let built = match self.host {
            Host::Fails => wire::encode_built(&Built {
                result: BuildResult::Failed,
                base: "",
                commit: "",
                log: b"error[E0277]: cannot add-assign `&str` to `u32`\n --> x\ncomponents/notes/src/lib.rs:2:5: error[E0277]: cannot add-assign\nerror: could not compile `comp-notes`\n",
                artifact: &[],
            }),
            _ => {
                let label = if self.host == Host::WrongLabel { "e9" } else { request.label };
                let source = if self.host == Host::WrongSource {
                    small("other\n")
                } else {
                    request.bundle.to_vec()
                };
                wire::encode_built(&Built {
                    result: BuildResult::Ok,
                    base: request.base,
                    commit: COMMIT,
                    log: b"",
                    artifact: &artifact(request.component, label, &source),
                })
            }
        };
        http("200 OK", &built)
    }
}

impl Env for Fake {
    fn info(&mut self, component: &str) -> Result<Info, Error> {
        if component != "notes" {
            return Err(Error::NotFound);
        }
        let mut info = Info::empty();
        info.active_len = put_label(&mut info.active, self.active);
        info.next_len = put_label(&mut info.next, self.next);
        Ok(info)
    }

    fn artifact(&mut self, _component: &str, label: &str) -> Result<Vec<u8>, Error> {
        self.artifacts
            .iter()
            .find(|(l, _)| *l == label)
            .map(|(_, bytes)| bytes.clone())
            .ok_or(Error::NotFound)
    }

    fn put(
        &mut self,
        component: &str,
        artifact: &[u8],
        base: &str,
        commit: &str,
    ) -> Result<(), Error> {
        self.put_result?;
        self.puts.push((
            component.into(),
            artifact.to_vec(),
            base.into(),
            commit.into(),
        ));
        Ok(())
    }

    fn build(&mut self, request: &[u8], _timeout_ms: u64) -> Result<Vec<u8>, Error> {
        if !wire::is_sync_request(request) {
            return Err(Error::Invalid);
        }
        self.requests.push(request.to_vec());
        Ok(self.respond(request))
    }

    fn build_start(&mut self, request: &[u8], _timeout_ms: u64) -> Result<(), Error> {
        if !wire::is_build_request(request) {
            return Err(Error::Invalid);
        }
        if self.job.is_some() {
            return Err(Error::Busy);
        }
        self.requests.push(request.to_vec());
        let reply = self.exchange.map(|()| self.respond(request));
        let busy = self.busy_polls;
        self.job = Some(Job { reply, busy });
        Ok(())
    }

    fn build_poll(&mut self, take: bool) -> Result<Vec<u8>, Error> {
        self.polls.push(take);
        let job = self.job.as_mut().ok_or(Error::NotFound)?;
        if self.fault_polls > 0 {
            self.fault_polls -= 1;
            return Err(Error::Fault);
        }
        if job.busy > 0 {
            job.busy -= 1;
            return Err(Error::Busy);
        }
        match &job.reply {
            Err(error) => {
                let error = *error;
                self.job = None;
                Err(error)
            }
            Ok(_) if !take => Ok(Vec::new()),
            Ok(_) => self
                .job
                .take()
                .map_or(Err(Error::NotFound), |job| job.reply),
        }
    }

    fn control(&mut self, component: &str, request: &str) -> Result<Vec<u8>, Error> {
        self.controls.push((component.into(), request.into()));
        self.control_reply.map(|reply| reply.as_bytes().to_vec())
    }

    fn log(&mut self, text: &str) {
        self.logs.push(text.into());
    }
}

pub(super) fn kernel(service: &mut Service, env: &mut Fake, request: &str) -> String {
    service.handle(env, 0, request.as_bytes())
}

pub(super) fn opened(lib: &str) -> (Service, Fake) {
    let mut service = Service::new("10.0.2.2:5000", TOKEN);
    let mut env = Fake::new(lib);
    let reply = kernel(&mut service, &mut env, "open notes");
    assert_eq!(
        reply,
        format!(
            "ok editing notes 2: 2 files; {LIB}, {} lines",
            lib.lines().count()
        )
    );
    (service, env)
}

pub(super) fn service() -> Service {
    Service::new("10.0.2.2:5000", TOKEN)
}

/// Sends `message` as the kernel does: an upload stored meanwhile is told to the
/// journal (COMP_PUT's hook), then the answer is journaled.
pub(super) fn say(
    service: &mut Service,
    env: &mut Fake,
    journal: &mut Journal,
    message: &str,
) -> String {
    let stored = env.puts.len();
    let reply = service.handle(env, 0, message.as_bytes());
    for (component, bytes, base, _) in &env.puts[stored..] {
        let label = livecore::artifact::parse(bytes).unwrap().manifest.version;
        journal.built(component, base, &label);
    }
    journal.record(message.as_bytes(), reply.as_bytes());
    reply
}

/// Gives a fresh service the journal's session, as the kernel does.
pub(super) fn restore(journal: &mut Journal, env: &mut Fake) -> (Service, Restore) {
    let mut fresh = service();
    let restore = journal.restore(&mut |m| Some(fresh.handle(env, 0, m).into_bytes()));
    (fresh, restore)
}

/// `status` until no build runs; the reply, without the line a moved cursor adds.
pub(super) fn status_until_done(service: &mut Service, env: &mut Fake) -> String {
    for _ in 0..20 {
        let reply = kernel(service, env, "cmd status");
        let shown = String::from_utf8_lossy(crate::ipc::split_moved(reply.as_bytes()).1);
        if !shown.lines().next().unwrap_or("").contains("building") {
            return shown.into_owned();
        }
    }
    panic!("the build never finished");
}

/// `build`, then `status` until it is done; the last build's answer.
pub(super) fn build_now(service: &mut Service, env: &mut Fake) -> String {
    let started = kernel(service, env, "cmd build");
    assert!(started.starts_with("ok building "), "{started}");
    let status = status_until_done(service, env);
    let (_, last) = status
        .split_once("\nlast build: ")
        .unwrap_or_else(|| panic!("no last build in {status:?}"));
    last.into()
}
