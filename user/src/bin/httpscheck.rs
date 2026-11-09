//! Test program for xtask's HTTPS mock in the boot cases (P5 task 12's
//! harness), started by the kernel's `https-mock` case with NET (for NET_TIME
//! and the boot bundle's `netroots/`) and a `NetDest` moved in. It fetches
//! through `kainet` on `user::tls`, so a pass shows the mock's names resolve,
//! its connections reach the host, and its test CA is trusted:
//! - `get=URL`: exits 0 on a 200, and prints its first line;
//! - `refused=URL`: exits 0 if the handle refuses the URL's host, a name it
//!   does not cover, before anything is sent;
//! - `tls=URL`: exits 0 if the handshake is refused: a certificate for another
//!   name, or from a CA the client does not trust.
//!
//! And as a caller of the `fetch` service (P5 task 12, the `fetch-service`
//! case), with a route to it, the way comphost and the shell call it:
//! - `via=fetch`: for each request, narrows a handle to exactly the URL's
//!   host from its own and moves it in with `kainet::wire`'s request. A GET
//!   is a 200 with its body; a redirect within the host is followed; a
//!   redirect to another host comes back unfollowed, and is then fetched on a
//!   handle for that host with `hops=1`; a host the handle does not name is
//!   refused.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use kabi::handle::{Handle, HandleInfo, Kind, Rights};
use kainet::fetch::Error as FetchError;
use kainet::http::{Fetch, HttpError, Limits, Method, TlsConnect, Url};
use kainet::tls::TlsError;
use kainet::wire::{self, Head};
use user::tls::{self, NetDial};
use user::{arg, args, println, sys};

user::entry!(main);

/// The most body bytes read.
const MAX_BODY: u64 = 64 * 1024;
/// How long one fetch may take.
const DEADLINE_NS: u64 = 60_000_000_000;
/// The most bytes of the body printed.
const SHOWN: usize = 80;

/// The `NetDest` handle the case moved in: the one that may resolve (NET's
/// own handle, for the LLM server, may only connect).
fn dest() -> Option<Handle> {
    let mut list = [HandleInfo::default(); 16];
    let count = sys::startup(&mut list).ok()?.min(list.len());
    list[..count]
        .iter()
        .find(|info| info.kind == Kind::NetDest as u32 && info.rights & Rights::RESOLVE.bits() != 0)
        .map(|info| Handle(info.handle))
}

/// GET `url` on `dest`: its status and body.
fn get(dest: Handle, url: &str) -> Result<(u16, Vec<u8>), HttpError> {
    let url = Url::parse(url).map_err(HttpError::Url)?;
    let config = tls::config().map_err(|e| HttpError::Tls(TlsError::Tls(e)))?;
    let mut connect = TlsConnect {
        config,
        dial: NetDial {
            dest,
            bulk: false,
            deadline_ns: sys::time_ns().saturating_add(DEADLINE_NS),
        },
    };
    let clock = || sys::time_ns();
    // The handle decides which hosts; the URL must still be https on 443.
    let allow = |_: &str, port: u16| port == 443;
    let fetch = Fetch {
        limits: Limits::new(MAX_BODY, clock().saturating_add(DEADLINE_NS)),
        allow: &allow,
        clock: &clock,
    };
    let mut body = Vec::new();
    let got = fetch.run(&mut connect, Method::Get, &url, None, &mut |bytes| {
        body.extend_from_slice(bytes);
        Ok(())
    })?;
    Ok((got.head.status, body))
}

/// The body's first line, at most [`SHOWN`] bytes, printable.
fn first_line(body: &[u8]) -> String {
    body.iter()
        .take_while(|&&b| b != b'\n')
        .take(SHOWN)
        .map(|&b| {
            if b == b' ' || b.is_ascii_graphic() {
                b as char
            } else {
                '?'
            }
        })
        .collect()
}

/// What `fetch` answered for one request: its head and body, or its error.
type Fetched = Result<(Head, Vec<u8>), FetchError>;

/// `request` through the `fetch` service, on a handle narrowed from `dest` to
/// exactly `host`.
fn via_fetch(dest: Handle, host: &str, request: &str) -> Result<Fetched, String> {
    let route = user::route(0).ok_or("no route to fetch")?;
    let spec = alloc::format!("proto:tcp,ports:443,hosts:{host},match:exact");
    let rights = Rights::RESOLVE.bits() | Rights::CONNECT.bits() | Rights::TRANSFER.bits();
    let narrowed = sys::netdest_narrow(dest, &spec, rights)
        .map_err(|e| alloc::format!("narrowing to {host}: {e:?}"))?;
    let mut reply = alloc::vec![0u8; wire::MAX_ANSWER];
    let mut received = [Handle(0); 1];
    let (len, count) = sys::call_h(
        route,
        request.as_bytes(),
        &mut reply,
        90_000,
        &[narrowed],
        &mut received,
    )
    .map_err(|e| alloc::format!("calling fetch: {e:?}"))?;
    let text = core::str::from_utf8(&reply[..len.min(reply.len())])
        .map_err(|_| "a reply that is not UTF-8")?;
    let head = match wire::parse_answer(text).map_err(String::from)? {
        Ok(head) => head,
        Err(error) => return Ok(Err(error)),
    };
    let mut body = Vec::new();
    if count == 1 {
        let shm = received[0];
        let address =
            sys::shm_map(shm, false).map_err(|e| alloc::format!("mapping the body: {e:?}"))?;
        let len = head.len.min(kainet::fetch::MAX_BODY);
        // SAFETY: `fetch` made the `Shm` of MAX_BODY bytes and moved it here;
        // it is mapped read-only at `address`, `len` is at most its size, and
        // the slice is copied out before the unmap.
        body.extend_from_slice(unsafe { core::slice::from_raw_parts(address as *const u8, len) });
        let _ = sys::shm_unmap(address);
        let _ = sys::handle_close(shm);
    }
    Ok(Ok((head, body)))
}

/// The `fetch-service` case's calls; whether each got what it should.
fn fetch_script(dest: Handle) -> bool {
    let checks: [(&str, &str, &str); 4] = [
        (
            "updates.kai.test",
            "get https://updates.kai.test/hello",
            "200",
        ),
        (
            "updates.kai.test",
            "get https://updates.kai.test/old",
            "200 within",
        ),
        (
            "releases.kai.test",
            "get https://releases.kai.test/dl",
            "302 off",
        ),
        (
            "updates.kai.test",
            "get https://elsewhere.kai.test/hello",
            "refused",
        ),
    ];
    let mut good = true;
    for (host, request, expect) in checks {
        let got = via_fetch(dest, host, request);
        let ok = match (&got, expect) {
            (Ok(Ok((head, body))), "200") => {
                println!(
                    "httpscheck: fetch {request}: {}: {}",
                    head.status,
                    first_line(body)
                );
                head.status == 200 && !body.is_empty()
            }
            (Ok(Ok((head, body))), "200 within") => {
                println!(
                    "httpscheck: fetch {request}: {} from {}: {}",
                    head.status,
                    head.url,
                    first_line(body)
                );
                head.status == 200 && head.url == "https://updates.kai.test/hello"
            }
            (Ok(Ok((head, _))), "302 off") => {
                let location = head.location.clone().unwrap_or_default();
                println!(
                    "httpscheck: fetch {request}: {} unfollowed, to {location}",
                    head.status
                );
                // As comphost does: a handle for the new host, and one hop taken.
                let next = alloc::format!("get {location} hops=1");
                match via_fetch(dest, "assets.kai.test", &next) {
                    Ok(Ok((head, body))) => {
                        println!(
                            "httpscheck: fetch {next}: {}: {}",
                            head.status,
                            first_line(&body)
                        );
                        head.status == 200
                    }
                    other => {
                        println!("httpscheck: fetch {next}: {other:?}");
                        false
                    }
                }
            }
            (Ok(Err(FetchError::Refused(host))), "refused") => {
                println!("httpscheck: fetch {request}: refused, the handle does not name {host}");
                true
            }
            (other, _) => {
                println!("httpscheck: fetch {request}: {other:?}, not {expect}");
                false
            }
        };
        good &= ok;
    }
    good
}

fn main() -> i32 {
    let args = args();
    let Some(dest) = dest() else {
        println!("httpscheck: no NetDest handle to fetch on");
        return 1;
    };
    if let Some(url) = arg(&args, "get") {
        return match get(dest, url) {
            Ok((200, body)) => {
                println!("httpscheck: GET {url}: 200: {}", first_line(&body));
                0
            }
            Ok((status, _)) => {
                println!("httpscheck: GET {url}: {status}, not 200");
                1
            }
            Err(error) => {
                println!("httpscheck: GET {url}: {error:?}");
                1
            }
        };
    }
    if let Some(url) = arg(&args, "refused") {
        return match get(dest, url) {
            Err(HttpError::Refused(host)) => {
                println!("httpscheck: GET {url}: refused, the handle does not name {host}");
                0
            }
            other => {
                println!("httpscheck: GET {url}: {other:?}, not refused");
                1
            }
        };
    }
    if let Some(url) = arg(&args, "tls") {
        return match get(dest, url) {
            Err(HttpError::Tls(TlsError::Tls(error))) => {
                println!("httpscheck: GET {url}: the handshake was refused: {error:?}");
                0
            }
            other => {
                println!("httpscheck: GET {url}: {other:?}, not a refused handshake");
                1
            }
        };
    }
    if arg(&args, "via") == Some("fetch") {
        return match fetch_script(dest) {
            true => 0,
            false => 1,
        };
    }
    println!("httpscheck: no get=, refused=, tls= or via=fetch");
    1
}
