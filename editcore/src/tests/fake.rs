//! A stand-in for the kernel and the build service, for the service tests.

use super::*;
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
        }
    }

    fn respond(&self, request: &[u8]) -> Vec<u8> {
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
        self.requests.push(request.to_vec());
        Ok(self.respond(request))
    }

    fn control(&mut self, component: &str, request: &str) -> Result<Vec<u8>, Error> {
        self.controls.push((component.into(), request.into()));
        self.control_reply.map(|reply| reply.as_bytes().to_vec())
    }

    fn log(&mut self, _text: &str) {}
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
