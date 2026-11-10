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

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::ToString;
use alloc::sync::Arc;
use alloc::vec::Vec;
use kabi::handle::{Handle, HandleInfo, Kind, MAX_TRANSFER, RecvInfo, Rights};
use kainet::fetch::{self, Error, MAX_BODY};
use kainet::http::TlsConnect;
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
    let mut request = alloc::vec![0u8; wire::MAX_ASK + 1];
    loop {
        let mut info = RecvInfo::default();
        let len = match sys::recv_h(&mut request, IDLE_MS, &mut info) {
            Ok(len) => len,
            Err(kabi::Error::TimedOut) => continue,
            Err(error) => {
                println!("fetch: a receive failed: {error:?}");
                sys::sleep_ms(10);
                continue;
            }
        };
        let count = (info.handle_count as usize).min(MAX_TRANSFER);
        let moved: Vec<Handle> = info.handles[..count].iter().map(|&h| Handle(h)).collect();
        let (mut result, body) = match len > wire::MAX_ASK {
            true => (
                Err(Error::BadUrl(fetch::why("the request is too long"))),
                Vec::new(),
            ),
            false => serve(&config, &request[..len], &moved),
        };
        // The caller's handle is used up with its call.
        for handle in &moved {
            let _ = sys::handle_close(*handle);
        }
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
        let answer = wire::answer(&result);
        let sent = match shm {
            Some(shm) => sys::reply_h(answer.as_bytes(), &[shm]),
            None => sys::reply_h(answer.as_bytes(), &[]),
        };
        if let Err(error) = sent {
            println!("fetch: a reply failed: {error:?}");
            // A body's `Shm` that did not move is still `fetch`'s: let it go.
            if let Some(shm) = shm {
                let _ = sys::handle_close(shm);
            }
        }
    }
}
