//! An app's `kai:net` requests (P5 tasks 12 and 13): comphost takes what the
//! shown version's `fetch` queued, asks the shell whether the app may reach
//! the URL's host, narrows the app's ceiling to that one host, exactly, for
//! the call, and has the `fetch` service make the request on that handle. The answer
//! goes to the app through `responses.on-response`, a call of its own like a
//! window's input.
//!
//! - **The ceiling** is the `NetDest` the app's plan line names
//!   (`netdest=`), which the kernel gave this process at start; the
//!   supervisor passes its text too, which comphost names to the shell. The
//!   kernel refuses any narrowing wider than the handle, whatever the text
//!   says.
//! - **The ask** goes to `desktop` (`deskcore::net_wire`), which knows this
//!   process by the kernel's name for it, and the shell answers `granted
//!   <netdest>`, `asked` (a card waits for the person: ask again) or
//!   `refused <why>`. An ask that stays unanswered for [`ASK_LIMIT_MS`] is a
//!   timeout.
//! - **One request at a time, and no wait.** A plain call to `fetch` blocks
//!   until the request is made, up to 30 s, and comphost runs the app on its
//!   one thread. So it starts the request (`kainet::later`: `fetch` answers
//!   `started` at once and keeps the answer) and takes the answer on a later
//!   pass. Each of those calls waits at most [`CALL_MS`]: while `fetch` is
//!   busy with a request, ours or another caller's, the call is withdrawn and
//!   tried again [`LOOK_MS`] later.
//! - **A redirect to another host** comes back unfollowed, with its
//!   `location`: comphost asks about that host by its own name and calls
//!   again with the hops so far, so one limit of five holds across calls.
//! - **Answers go to the instance that asked**: a request is kept with the
//!   serial of the version that queued it, and an answer for one no longer
//!   shown is dropped.

use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use deskcore::net_wire;
use kabi::handle::{Handle, HandleInfo, Kind, Rights};
use kainet::fetch::{Error as FetchError, MAX_BODY, Request};
use kainet::http::{Method, Range, Url};
use kainet::later::{self, Message};
use kainet::wire::{self, Ask};
use livecore::host::{Env, Host};
use user::{println, sys};
use wasmhost::LiveInstance;
use wasmhost::net::{NetError, NetRequest, NetResponse};

/// How often an ask the person has not answered is asked again.
const ASK_EVERY_MS: u64 = 500;
/// How long a request may wait for the shell and the person, in all.
const ASK_LIMIT_MS: u64 = 120_000;
/// `desktop`'s wait for one ask.
const DESKTOP_MS: u64 = 2_000;
/// The most one call to `fetch` waits, a start or a take: the app's next
/// input is not held up longer.
const CALL_MS: u64 = 20;
/// How often a start or a take is tried again.
pub const LOOK_MS: u64 = 50;
/// How long a request may take from its first start to its answer: `fetch`'s
/// own 30 s for it, and room for the calls ahead of it.
const CALL_LIMIT_MS: u64 = 60_000;
/// Requests taken from the app and not yet answered; the app's own queue
/// holds the rest (`wasmhost::net::MAX_QUEUED`).
const MAX_HELD: usize = 8;

/// Where a request is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Asking the shell about its host.
    Ask,
    /// Granted: starting it on `fetch`.
    Start,
    /// Started as `id`: taking its answer.
    Take { id: u32 },
}

/// One request on its way.
struct Job {
    /// The version that asked.
    serial: u64,
    ticket: u32,
    request: Request,
    /// Redirects followed so far, across calls.
    hops: u8,
    step: Step,
    /// When the next try may go, and when this step's first did.
    next_ms: u64,
    since_ms: u64,
}

/// The `NetDest` the plan line's `netdest=` gave this process, its ceiling:
/// the one with RESOLVE. The NET capability, which holding it needs, also
/// starts a process with NET's own handle to the LLM server (CONNECT alone),
/// which is not the app's: it is closed here.
fn ceiling_handle() -> Option<Handle> {
    let mut list = [HandleInfo::default(); kabi::handle::MAX_STARTUP];
    let count = sys::startup(&mut list).ok()?.min(list.len());
    let mut ceiling = None;
    for info in list[..count]
        .iter()
        .filter(|info| info.kind() == Some(Kind::NetDest))
    {
        match info.rights & Rights::RESOLVE.bits() != 0 && ceiling.is_none() {
            true => ceiling = Some(Handle(info.handle)),
            false => {
                let _ = sys::handle_close(Handle(info.handle));
            }
        }
    }
    ceiling
}

/// The host of `url`, if `fetch` would take it.
fn host_of(url: &str) -> Result<String, NetError> {
    Url::parse(url)
        .map(|url| url.host)
        .map_err(|why| NetError::BadUrl(String::from(why)))
}

fn net_error(error: FetchError) -> NetError {
    match error {
        FetchError::Refused(why) => NetError::Refused(why),
        FetchError::BadUrl(why) => NetError::BadUrl(why),
        FetchError::Tls(why) => NetError::Tls(why),
        FetchError::Http(why) => NetError::Http(why),
        FetchError::TooLarge => NetError::TooLarge,
        FetchError::Timeout => NetError::Timeout,
        FetchError::Truncated => NetError::Truncated,
    }
}

/// The first `len` bytes of the body's `Shm`, which is then let go.
fn body_of(shm: Handle, len: usize) -> Result<Vec<u8>, NetError> {
    let read = sys::shm_map(shm, false).map(|address| {
        let len = len.min(MAX_BODY);
        // SAFETY: `fetch` made the `Shm` with room for `MAX_BODY` bytes and
        // moved it here; the kernel mapped it read-only at `address`, and
        // nothing writes it now. The bytes are copied out before the unmap.
        let bytes = unsafe { core::slice::from_raw_parts(address as *const u8, len) }.to_vec();
        let _ = sys::shm_unmap(address);
        bytes
    });
    let _ = sys::handle_close(shm);
    read.map_err(|error| NetError::Http(format!("the body could not be read ({error:?})")))
}

pub struct Net {
    ceiling: Handle,
    ceiling_text: String,
    desktop: Handle,
    fetch: Handle,
    /// The next start's id, comphost's own.
    next_id: u32,
    jobs: VecDeque<Job>,
    reply: Vec<u8>,
    /// Whether the log has said that the first answer reached the app.
    said: bool,
}

/// What became of a job this pass.
enum Next {
    Wait,
    Answer(Result<NetResponse, NetError>),
}

impl Net {
    /// The app's network side, when its plan line names a ceiling: its
    /// `NetDest`, that destination's text among `args`, a route to `desktop`
    /// first and to `fetch` second.
    pub fn new(args: &str) -> Option<Self> {
        let ceiling_text = args
            .split_whitespace()
            .find_map(|w| w.strip_prefix("netdest="))?
            .to_string();
        let ceiling = ceiling_handle()?;
        let desktop = user::route(0)?;
        let fetch = user::route(1)?;
        Some(Self {
            ceiling,
            ceiling_text,
            desktop,
            fetch,
            next_id: 0,
            jobs: VecDeque::new(),
            reply: alloc::vec![0u8; kabi::MAX_MESSAGE],
            said: false,
        })
    }

    /// How long until the next pass is due, for the host's wait; `None` when
    /// nothing waits. A request waiting or on its way is use: the app does
    /// not nap until it has its answer.
    pub fn wait_ms<E: Env<Instance = LiveInstance>>(
        &self,
        host: &mut Host<E>,
        now_ms: u64,
    ) -> Option<u64> {
        if waiting(host) {
            return Some(0);
        }
        let job = self.jobs.front()?;
        Some(job.next_ms.saturating_sub(now_ms))
    }

    /// Takes the shown version's requests, moves the oldest on, and gives the
    /// app any answer; whether one was given.
    pub fn pump<E: Env<Instance = LiveInstance>>(
        &mut self,
        host: &mut Host<E>,
        now_ms: u64,
    ) -> bool {
        let mut answers = self.take(host, now_ms);
        if let Some(answer) = self.step(now_ms) {
            answers.push(answer);
        }
        if answers.is_empty() {
            return false;
        }
        let shown = crate::started::shown_serial(host);
        let given = crate::ui::serve_shown(host, |core| {
            for (serial, ticket, outcome) in answers {
                if Some(serial) == shown {
                    core.net_response(ticket, outcome)?;
                }
            }
            Ok(())
        });
        match given {
            None => println!("comphost: a network answer did not reach the app"),
            Some(_) if !self.said => {
                self.said = true;
                println!("comphost: the first network answer reached the app");
            }
            Some(_) => {}
        }
        true
    }

    /// The shown version's new requests; a standby's are dropped. Those
    /// `fetch` would never take, or past [`MAX_HELD`], are answered at once.
    fn take<E: Env<Instance = LiveInstance>>(
        &mut self,
        host: &mut Host<E>,
        now_ms: u64,
    ) -> Vec<(u64, u32, Result<NetResponse, NetError>)> {
        let mut taken = Vec::new();
        host.each_instance(|instance, _, shown| {
            if let LiveInstance::V2(core) = instance {
                let requests = core.take_net();
                if shown {
                    let serial = core.serial();
                    taken.extend(requests.into_iter().map(|(t, r)| (serial, t, r)));
                }
            }
        });
        let mut answers = Vec::new();
        for (serial, ticket, asked) in taken {
            let request = request_of(&asked);
            match host_of(&request.url) {
                Err(error) => answers.push((serial, ticket, Err(error))),
                Ok(_) if self.jobs.len() >= MAX_HELD => answers.push((
                    serial,
                    ticket,
                    Err(NetError::Http(String::from("too many requests wait"))),
                )),
                Ok(_) => self.jobs.push_back(Job {
                    serial,
                    ticket,
                    request,
                    hops: 0,
                    step: Step::Ask,
                    next_ms: now_ms,
                    since_ms: now_ms,
                }),
            }
        }
        answers
    }

    /// Moves the oldest request on: its ask, its start or its take.
    fn step(&mut self, now_ms: u64) -> Option<(u64, u32, Result<NetResponse, NetError>)> {
        let mut job = self.jobs.pop_front()?;
        let next = match job.step {
            _ if now_ms < job.next_ms => Next::Wait,
            Step::Ask => self.ask(&mut job, now_ms),
            Step::Start => self.start(&mut job, now_ms),
            Step::Take { id } => self.take_answer(&mut job, id, now_ms),
        };
        match next {
            Next::Wait => {
                self.jobs.push_front(job);
                None
            }
            Next::Answer(outcome) => Some((job.serial, job.ticket, outcome)),
        }
    }

    /// Asks the shell about the job's host; on a grant, starts it.
    fn ask(&mut self, job: &mut Job, now_ms: u64) -> Next {
        let host = match host_of(&job.request.url) {
            Ok(host) => host,
            Err(error) => return Next::Answer(Err(error)),
        };
        let ask = net_wire::encode_ask(&host, &self.ceiling_text);
        let answer = sys::call_route(self.desktop, ask.as_bytes(), &mut self.reply, DESKTOP_MS)
            .ok()
            .map(|len| {
                String::from_utf8_lossy(&self.reply[..len.min(self.reply.len())]).into_owned()
            });
        let answer = answer.unwrap_or_else(|| String::from(net_wire::WAIT));
        // The shell's answer names every host the app holds; the call is
        // made on this one alone.
        if answer.starts_with("granted ") {
            job.step = Step::Start;
            job.since_ms = now_ms;
            return self.start(job, now_ms);
        }
        if let Some(why) = answer.strip_prefix("refused ") {
            println!("comphost: the shell refused {host}: {why}");
            return Next::Answer(Err(NetError::Refused(host)));
        }
        // `wait` or `asked`: the person may not have answered yet.
        if now_ms.saturating_sub(job.since_ms) >= ASK_LIMIT_MS {
            println!("comphost: no answer came for {host}");
            return Next::Answer(Err(NetError::Timeout));
        }
        job.next_ms = now_ms.saturating_add(ASK_EVERY_MS);
        Next::Wait
    }

    /// One call to `fetch`, waiting at most [`CALL_MS`]: its reply, and the
    /// handle that moved back with it. `send`, if the call did not take it,
    /// is closed: each try narrows a handle of its own.
    fn call_fetch(
        &mut self,
        text: &str,
        send: Option<Handle>,
    ) -> Result<(String, Option<Handle>), kabi::Error> {
        let sending: &[Handle] = match &send {
            Some(handle) => core::slice::from_ref(handle),
            None => &[],
        };
        let mut received = [Handle::NONE; 1];
        let called = sys::call_h(
            self.fetch,
            text.as_bytes(),
            &mut self.reply,
            CALL_MS,
            sending,
            &mut received,
        );
        match called {
            Ok((len, came)) => {
                let text = String::from_utf8_lossy(&self.reply[..len.min(self.reply.len())]);
                Ok((text.into_owned(), (came > 0).then_some(received[0])))
            }
            Err(error) => {
                // A call refused before it was sent leaves the handle here; one
                // sent and withdrawn took it with it.
                if let Some(handle) = send
                    && sys::handle_info(handle).is_ok_and(|i| i.kind() == Some(Kind::NetDest))
                {
                    let _ = sys::handle_close(handle);
                }
                Err(error)
            }
        }
    }

    /// Whether a job's calls to `fetch` have taken too long: its answer.
    fn overdue(job: &Job, now_ms: u64) -> Option<Next> {
        (now_ms.saturating_sub(job.since_ms) >= CALL_LIMIT_MS).then(|| {
            println!("comphost: fetch did not answer in time");
            Next::Answer(Err(NetError::Timeout))
        })
    }

    /// Narrows the ceiling to the job's host alone (`net_wire::call_dest`)
    /// and starts the request on `fetch`; tried again while `fetch` is busy.
    fn start(&mut self, job: &mut Job, now_ms: u64) -> Next {
        let host = match host_of(&job.request.url) {
            Ok(host) => host,
            Err(error) => return Next::Answer(Err(error)),
        };
        let rights = Rights::RESOLVE.bits() | Rights::CONNECT.bits() | Rights::TRANSFER.bits();
        let handle = match sys::netdest_narrow(self.ceiling, &net_wire::call_dest(&host), rights) {
            Ok(handle) => handle,
            Err(error) => {
                println!("comphost: the ceiling does not reach {host} ({error:?})");
                return Next::Answer(Err(NetError::Refused(host)));
            }
        };
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let ask = Ask {
            request: job.request.clone(),
            hops: job.hops,
        };
        let text = Message::Start { id, ask }.text();
        match self.call_fetch(&text, Some(handle)) {
            Ok((reply, body)) if reply == later::STARTED => {
                if let Some(body) = body {
                    let _ = sys::handle_close(body);
                }
                job.step = Step::Take { id };
                job.next_ms = now_ms.saturating_add(LOOK_MS);
                Next::Wait
            }
            // `fetch` refused the start itself: its answer says why.
            Ok((reply, body)) => self.answered(job, &reply, body, now_ms),
            Err(_) => Self::overdue(job, now_ms).unwrap_or_else(|| {
                job.next_ms = now_ms.saturating_add(LOOK_MS);
                Next::Wait
            }),
        }
    }

    /// Takes the answer to start `id`; tried again while `fetch` is busy.
    fn take_answer(&mut self, job: &mut Job, id: u32, now_ms: u64) -> Next {
        let text = Message::Take { id }.text();
        match self.call_fetch(&text, None) {
            Ok((reply, None)) if reply == later::NONE => {
                Self::overdue(job, now_ms).unwrap_or_else(|| {
                    job.next_ms = now_ms.saturating_add(LOOK_MS);
                    Next::Wait
                })
            }
            Ok((reply, body)) => self.answered(job, &reply, body, now_ms),
            Err(_) => Self::overdue(job, now_ms).unwrap_or_else(|| {
                job.next_ms = now_ms.saturating_add(LOOK_MS);
                Next::Wait
            }),
        }
    }

    /// What `fetch`'s answer comes to: the app's, or, for a redirect to
    /// another host, the same job again, to ask about that host.
    fn answered(&mut self, job: &mut Job, text: &str, body: Option<Handle>, now_ms: u64) -> Next {
        let head = match wire::parse_answer(text) {
            Ok(Ok(head)) => head,
            Ok(Err(error)) => {
                if let Some(body) = body {
                    let _ = sys::handle_close(body);
                }
                return Next::Answer(Err(net_error(error)));
            }
            Err(why) => {
                if let Some(body) = body {
                    let _ = sys::handle_close(body);
                }
                return Next::Answer(Err(NetError::Http(String::from(why))));
            }
        };
        let body = match body {
            Some(shm) => match body_of(shm, head.len) {
                Ok(body) => body,
                Err(error) => return Next::Answer(Err(error)),
            },
            None => Vec::new(),
        };
        if let Some(location) = head.location {
            // Not followed: a host of its own, asked about by its name.
            let to = host_of(&location).unwrap_or_default();
            println!("comphost: a redirect to {to} is asked about by its own name");
            job.request.url = location;
            job.hops = head.hops.saturating_add(1);
            job.step = Step::Ask;
            job.next_ms = now_ms;
            job.since_ms = now_ms;
            return Next::Wait;
        }
        Next::Answer(Ok(NetResponse {
            status: head.status,
            url: head.url,
            content_type: head.content_type,
            retry_after: head.retry_after,
            body,
        }))
    }
}

/// Whether the shown version has requests not yet taken.
fn waiting<E: Env<Instance = LiveInstance>>(host: &mut Host<E>) -> bool {
    let mut waiting = false;
    host.each_instance(|instance, _, shown| {
        if let (LiveInstance::V2(core), true) = (instance, shown) {
            waiting = core.net_waiting();
        }
    });
    waiting
}

/// The app's request as `fetch` takes it.
fn request_of(asked: &NetRequest) -> Request {
    Request {
        url: asked.url.clone(),
        method: match asked.head {
            true => Method::Head,
            false => Method::Get,
        },
        range: asked.range.map(|(first, last)| Range { first, last }),
    }
}
