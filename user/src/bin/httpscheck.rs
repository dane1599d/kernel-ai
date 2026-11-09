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

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use kabi::handle::{Handle, HandleInfo, Kind, Rights};
use kainet::http::{Fetch, HttpError, Limits, Method, TlsConnect, Url};
use kainet::tls::TlsError;
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
        dial: NetDial { dest, bulk: false },
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
    println!("httpscheck: no get=, refused= or tls=");
    1
}
