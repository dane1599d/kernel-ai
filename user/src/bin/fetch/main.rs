//! The `fetch` service (P5 task 12): the HTTPS requests of apps, through
//! comphost, and of the agent, through the shell, each made on the caller's
//! own `NetDest`, moved in with the call. `fetch` holds no destination of its
//! own: it closes NET's startup handle at once, and each caller's handle when
//! that call ends. The kernel decides where a handle reaches. `fetch` asks it
//! with RESOLVE_ON before it follows a redirect, and a redirect to a host the
//! handle does not reach comes back unfollowed, for the caller to ask about
//! that host and call again with a handle for it (`kainet::fetch::serve_within`).
//!
//! A call is `kainet::wire`'s request line with one `NetDest` (RESOLVE and
//! CONNECT) moved in. The answer is `kainet::wire`'s, and a body, if any,
//! comes in an `Shm` of [`BODY_PAGES`] that `fetch` makes and moves back, so
//! it never writes into memory a caller sized. One call is served at a time,
//! each within [`DEADLINE_NS`]; the callers queue in the kernel.
//!
//! A caller that cannot wait that long (comphost, which runs an app on its one
//! thread) starts a request instead (`kainet::later`): `fetch` answers
//! `started` at once, makes the request, and keeps the answer for the caller's
//! `take`. That costs `fetch` what it keeps: at most [`MAX_KEPT`] answers, one
//! a caller, each with its body's `Shm` (64 KiB), for at most [`KEEP_NS`]
//! unless taken. While it makes a started request, other calls wait in the
//! kernel as they do for a plain one.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use kabi::handle::{Handle, HandleInfo, Kind, MAX_TRANSFER, RecvInfo, Rights};
use kainet::fetch::{self, Error, MAX_BODY};
use kainet::http::TlsConnect;
use kainet::later::{self, Message};
use kainet::wire::{self, Ask, Head};
use rustls::ClientConfig;
use user::tls::{self, NetDial};
use user::{println, sys};

user::entry!(main);

/// The body's `Shm`: [`MAX_BODY`] bytes.
const BODY_PAGES: u64 = (MAX_BODY / PAGE) as u64;
const PAGE: usize = 4096;
/// How long one call may take, redirects within its handle included.
const DEADLINE_NS: u64 = 30_000_000_000;
/// How long a receive waits before it looks again.
const IDLE_MS: u64 = 60_000;
/// Answers kept for takes, every caller together: a caller keeps one.
const MAX_KEPT: usize = 8;
/// How long an answer is kept for its take.
const KEEP_NS: u64 = 60_000_000_000;

/// Closes every `NetDest` this process started with. Since #779 NET brings
/// none; kept so a plan line that adds one never leaves `fetch` a destination
/// of its own.
fn close_own_destinations() {
    let mut list = [HandleInfo::default(); 16];
    let count = sys::startup(&mut list).map_or(0, |n| n.min(list.len()));
    for info in &list[..count] {
        if info.kind() == Some(Kind::NetDest) {
            let _ = sys::handle_close(Handle(info.handle));
        }
    }
}

/// The one `NetDest` a call moved in, with RESOLVE and CONNECT.
fn destination(moved: &[Handle]) -> Option<Handle> {
    let [dest] = moved else {
        return None;
    };
    let info = sys::handle_info(*dest).ok()?;
    let needed = Rights::RESOLVE.bits() | Rights::CONNECT.bits();
    (info.kind() == Some(Kind::NetDest) && info.rights & needed == needed).then_some(*dest)
}

/// Lets go of the handles a call moved in.
fn close_all(moved: &[Handle]) {
    for handle in moved {
        let _ = sys::handle_close(*handle);
    }
}

/// What `request` comes to on the caller's `moved` handle: the head, and the
/// body if there is one.
fn serve(
    config: &Arc<ClientConfig>,
    request: &[u8],
    moved: &[Handle],
) -> (Result<Head, Error>, Vec<u8>) {
    let ask = match core::str::from_utf8(request).map_err(|_| "the request is not UTF-8") {
        Ok(text) => Ask::parse(text),
        Err(why) => Err(why),
    };
    let ask = match ask {
        Ok(ask) => ask,
        Err(why) => return (Err(Error::BadUrl(fetch::why(why))), Vec::new()),
    };
    let Some(dest) = destination(moved) else {
        return (
            Err(Error::Refused(
                "no destination was moved in with the call".to_string(),
            )),
            Vec::new(),
        );
    };
    // Whether the handle reaches a host: the kernel's word, through RESOLVE_ON.
    // A name that does not resolve is the dial's to report, not a refusal.
    let allow = |host: &str, port: u16| {
        let mut addrs = [[0u8; 4]; kabi::net::MAX_ADDRESSES];
        port == fetch::PORT
            && !matches!(
                sys::resolve_on(dest, host, &mut addrs),
                Err(kabi::Error::Permission)
            )
    };
    let clock = || sys::time_ns();
    let deadline = clock().saturating_add(DEADLINE_NS);
    // Every wait on the network ends by the call's deadline too, so a peer
    // that trickles bytes cannot hold `fetch`, and the callers behind it,
    // past it (K's review).
    let mut connect = TlsConnect {
        config: Arc::clone(config),
        dial: NetDial {
            dest,
            bulk: false,
            deadline_ns: deadline,
        },
    };
    match fetch::serve_within(
        &ask.request,
        ask.hops,
        &allow,
        &mut connect,
        &clock,
        deadline,
    ) {
        Ok(served) => (Ok(Head::of(&served)), served.response.body),
        Err(error) => (Err(error), Vec::new()),
    }
}

/// A new `Shm` holding `body`.
fn body_shm(body: &[u8]) -> Result<Handle, kabi::Error> {
    let shm = sys::shm_create(BODY_PAGES)?;
    let address = match sys::shm_map(shm, true) {
        Ok(address) => address,
        Err(error) => {
            let _ = sys::handle_close(shm);
            return Err(error);
        }
    };
    let len = body.len().min(MAX_BODY);
    // SAFETY: `fetch` made the `Shm` just now, [`BODY_PAGES`] pages, and the
    // kernel mapped all of them read-write at `address`; nothing else holds
    // it yet, and `len` is at most their size. The slice is not used after
    // the unmap.
    let mapped = unsafe { core::slice::from_raw_parts_mut(address as *mut u8, MAX_BODY) };
    mapped[..len].copy_from_slice(&body[..len]);
    let _ = sys::shm_unmap(address);
    Ok(shm)
}

/// What a request comes to, as `fetch` replies it: the answer's text and the
/// body's `Shm`, if there is a body. The caller's handles are closed: they are
/// used up with the request.
fn made(config: &Arc<ClientConfig>, request: &[u8], moved: &[Handle]) -> (String, Option<Handle>) {
    let (mut result, body) = match request.len() > wire::MAX_ASK {
        true => (
            Err(Error::BadUrl(fetch::why("the request is too long"))),
            Vec::new(),
        ),
        false => serve(config, request, moved),
    };
    close_all(moved);
    let shm = match (&result, body.is_empty()) {
        (Ok(_), false) => match body_shm(&body) {
            Ok(shm) => Some(shm),
            Err(error) => {
                println!("fetch: no Shm for a body: {error:?}");
                result = Err(Error::Http(fetch::why("fetch had no room for the body")));
                None
            }
        },
        _ => None,
    };
    (wire::answer(&result), shm)
}

/// Replies `answer`, moving `shm` with it; an `Shm` that did not move is let go.
fn reply(answer: &str, shm: Option<Handle>) {
    let sent = match shm {
        Some(shm) => sys::reply_h(answer.as_bytes(), &[shm]),
        None => sys::reply_h(answer.as_bytes(), &[]),
    };
    if let Err(error) = sent {
        println!("fetch: a reply failed: {error:?}");
        if let Some(shm) = shm {
            let _ = sys::handle_close(shm);
        }
    }
}

/// An answer kept for a caller's take (`kainet::later`).
struct Kept {
    caller: u64,
    id: u32,
    answer: String,
    shm: Option<Handle>,
    at_ns: u64,
}

/// The answers kept for takes: at most [`MAX_KEPT`], one a caller, each for
/// [`KEEP_NS`].
#[derive(Default)]
struct Keep {
    kept: Vec<Kept>,
}

impl Keep {
    fn drop_one(kept: Kept) {
        if let Some(shm) = kept.shm {
            let _ = sys::handle_close(shm);
        }
    }

    /// Lets go of what is older than [`KEEP_NS`].
    fn expire(&mut self, now_ns: u64) {
        let (old, kept): (Vec<_>, Vec<_>) = core::mem::take(&mut self.kept)
            .into_iter()
            .partition(|k| now_ns.saturating_sub(k.at_ns) >= KEEP_NS);
        self.kept = kept;
        old.into_iter().for_each(Self::drop_one);
    }

    /// Keeps `kept`, in place of anything its caller had kept.
    fn put(&mut self, kept: Kept) {
        if let Some(at) = self.kept.iter().position(|k| k.caller == kept.caller) {
            Self::drop_one(self.kept.remove(at));
        }
        if self.kept.len() >= MAX_KEPT {
            Self::drop_one(self.kept.remove(0));
        }
        self.kept.push(kept);
    }

    /// The answer kept for `caller`'s request `id`, taken.
    fn take(&mut self, caller: u64, id: u32) -> Option<Kept> {
        let at = self
            .kept
            .iter()
            .position(|k| k.caller == caller && k.id == id)?;
        Some(self.kept.remove(at))
    }
}

fn main() -> i32 {
    // The roots first: BOOT_READ of `netroots/` needs NET in this line's
    // ceiling (no handle: `fetch` holds a destination only during a call).
    // NET_TIME is asked during a call, on the caller's handle.
    let config = match tls::config() {
        Ok(config) => config,
        Err(error) => {
            println!("fetch: no TLS configuration: {error:?}");
            return 1;
        }
    };
    close_own_destinations();
    println!("fetch: ready");
    // Room for the longest start, and a byte to spare.
    let mut request = alloc::vec![0u8; later::MAX_MESSAGE + 1];
    let mut keep = Keep::default();
    loop {
        let mut info = RecvInfo::default();
        let len = match sys::recv_h(&mut request, IDLE_MS, &mut info) {
            Ok(len) => len,
            Err(kabi::Error::TimedOut) => {
                keep.expire(sys::time_ns());
                continue;
            }
            Err(error) => {
                println!("fetch: a receive failed: {error:?}");
                sys::sleep_ms(10);
                continue;
            }
        };
        keep.expire(sys::time_ns());
        let count = (info.handle_count as usize).min(MAX_TRANSFER);
        let moved: Vec<Handle> = info.handles[..count].iter().map(|&h| Handle(h)).collect();
        // `recv_h` gives the message's whole length but copies only what the
        // buffer holds: a longer message is refused whole, never read cut
        // short (#831).
        if len > later::MAX_MESSAGE {
            close_all(&moved);
            reply(&later::too_long(), None);
            continue;
        }
        let text = core::str::from_utf8(&request[..len]).unwrap_or("");
        match Message::parse(text) {
            // A plain request: answered when it is made.
            None => {
                let (answer, shm) = made(&config, &request[..len], &moved);
                reply(&answer, shm);
            }
            // Started at once, made, and kept for the caller's take.
            Some(Ok(Message::Start { id, ask })) => {
                reply(later::STARTED, None);
                let (answer, shm) = made(&config, ask.text().as_bytes(), &moved);
                keep.put(Kept {
                    caller: info.caller_pid,
                    id,
                    answer,
                    shm,
                    at_ns: sys::time_ns(),
                });
            }
            Some(Ok(Message::Take { id })) => {
                close_all(&moved);
                match keep.take(info.caller_pid, id) {
                    Some(kept) => reply(&kept.answer, kept.shm),
                    None => reply(later::NONE, None),
                }
            }
            Some(Err(why)) => {
                close_all(&moved);
                reply(&wire::answer(&Err(Error::BadUrl(fetch::why(why)))), None);
            }
        }
    }
}
