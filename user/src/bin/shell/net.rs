//! The agent's network rows in the OS (P5 task 11): `net_fetch` through the
//! `fetch` service, and `net_lookup`'s resolve on the shell's own `NetDest`.
//!
//! The shell's plan line holds its network ceiling (`kabi::plan::SHELL_NET`).
//! Each call narrows it to exactly the one host consent grounded
//! (`match:exact`) and moves that handle to `fetch` with the request, so the
//! kernel, not the shell's word, bounds what the request can reach. `fetch`
//! holds no handle of its own. A redirect to another host comes back
//! unfollowed: the line named the first host, not the second, so it is an
//! error here rather than a second fetch.
//!
//! A fetch blocks the line for at most [`FETCH_MS`], as `ask` and `cc` do: the
//! shell has no threads. Ctrl-C cancels the call.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use kabi::handle::{Handle, HandleInfo, Kind, Rights};
use kainet::fetch::{Error as FetchError, Request};
use kainet::http::Method;
use kainet::wire::{self, Ask};
use spin::Once;
use user::sys;

/// How long one fetch may take: `fetch`'s own 30 s, and its queue.
const FETCH_MS: u64 = 35_000;

/// The shell's `NetDest` ceiling, found once among its startup handles: the
/// one that may resolve.
static CEILING: Once<Option<Handle>> = Once::new();

fn ceiling() -> Result<Handle, String> {
    let found = CEILING.call_once(|| {
        let mut list = [HandleInfo::default(); 32];
        let count = sys::startup(&mut list).ok()?.min(list.len());
        list[..count]
            .iter()
            .find(|info| {
                info.kind == Kind::NetDest as u32 && info.rights & Rights::RESOLVE.bits() != 0
            })
            .map(|info| Handle(info.handle))
    });
    found.ok_or_else(|| String::from("the shell holds no network handle"))
}

/// `host` as a host name alone, lower case: nothing that could add a field to
/// the narrowing spec (`a.example,addrs:any`) or an authority to the URL.
fn host_name(host: &str) -> Result<String, String> {
    netdest::normalize(host).ok_or_else(|| format!("{host:?} is not a host name"))
}

/// A handle for exactly `host` (already [`host_name`]d), narrowed from the
/// ceiling with `rights`.
fn narrowed(host: &str, rights: u32) -> Result<Handle, String> {
    let spec = format!("proto:tcp,ports:443,hosts:{host},match:exact");
    sys::netdest_narrow(ceiling()?, &spec, rights)
        .map_err(|_| format!("{host} is outside the shell's network ceiling"))
}

/// GET `path` from `host` over HTTPS: the body, as text.
pub fn fetch(host: &str, path: &str) -> Result<String, String> {
    let route = crate::routes::named("fetch").ok_or("the shell has no route to fetch")?;
    let host = host_name(host)?;
    let host = host.as_str();
    let path = path_of(path)?;
    let ask = Ask {
        request: Request {
            url: format!("https://{host}{path}"),
            method: Method::Get,
            range: None,
        },
        hops: 0,
    };
    let rights = Rights::RESOLVE.bits() | Rights::CONNECT.bits() | Rights::TRANSFER.bits();
    let dest = narrowed(host, rights)?;
    let mut reply = alloc::vec![0u8; wire::MAX_ANSWER];
    let mut received = [Handle(0); 1];
    let called = sys::call_h(
        route,
        ask.text().as_bytes(),
        &mut reply,
        FETCH_MS,
        &[dest],
        &mut received,
    );
    let (len, count) = match called {
        Ok(answered) => answered,
        Err(error) => {
            // A call refused before the move (no route, bad arguments) leaves
            // the handle here; one that was moved is gone, and this close
            // finds nothing.
            let _ = sys::handle_close(dest);
            return Err(format!("fetch did not answer: {error:?}"));
        }
    };
    let shm = (count == 1).then_some(received[0]);
    let head = match answer_head(&reply[..len.min(reply.len())]) {
        Ok(head) => head,
        Err(why) => {
            if let Some(shm) = shm {
                let _ = sys::handle_close(shm);
            }
            return Err(why);
        }
    };
    let body = match shm {
        Some(shm) => take_body(shm, head.len)?,
        None => Vec::new(),
    };
    if let Some(location) = head.location {
        return Err(format!(
            "{host} sent it on to {location}, which the request did not name"
        ));
    }
    if !(200..300).contains(&head.status) {
        return Err(format!("{host} answered {}", head.status));
    }
    let len = head.len.min(body.len());
    Ok(String::from_utf8_lossy(&body[..len]).into_owned())
}

/// A URL path: "/" when empty, else from '/' on, with no space or control
/// character (what follows is the server's to read, never the authority).
fn path_of(path: &str) -> Result<&str, String> {
    match path {
        "" => Ok("/"),
        path if !path.starts_with('/') => Err(String::from("a path starts with /")),
        path if path.chars().any(|c| c.is_whitespace() || c.is_control()) => {
            Err(String::from("a path has no spaces or control characters"))
        }
        path => Ok(path),
    }
}

/// `fetch`'s answer line, as a head or as its error in words.
fn answer_head(reply: &[u8]) -> Result<wire::Head, String> {
    let text =
        core::str::from_utf8(reply).map_err(|_| String::from("fetch's answer is not text"))?;
    match wire::parse_answer(text).map_err(String::from)? {
        Ok(head) => Ok(head),
        Err(error) => Err(failed(&error)),
    }
}

/// The first `len` bytes of the body `fetch` moved back in an `Shm`, copied
/// out; the `Shm` is closed. The kernel tells no `Shm`'s size, so the read
/// stops at the length `fetch`'s answer gave, never past [`MAX_BODY`], the
/// size `fetch` makes every body's `Shm`; and the handle must be an `Shm`.
///
/// [`MAX_BODY`]: kainet::fetch::MAX_BODY
fn take_body(shm: Handle, len: usize) -> Result<Vec<u8>, String> {
    let is_shm = sys::handle_info(shm).is_ok_and(|info| info.kind == Kind::Shm as u32);
    let mapped = if is_shm {
        sys::shm_map(shm, false).ok()
    } else {
        None
    };
    let body = mapped.map(|address| {
        let len = len.min(kainet::fetch::MAX_BODY);
        // SAFETY: `fetch` made the `Shm` of MAX_BODY bytes and moved it here; it
        // is mapped read-only at `address`, `len` is at most MAX_BODY, and the
        // slice is copied out before the unmap.
        let body = unsafe { core::slice::from_raw_parts(address as *const u8, len) }.to_vec();
        let _ = sys::shm_unmap(address);
        body
    });
    let _ = sys::handle_close(shm);
    body.ok_or_else(|| String::from("fetch's body could not be read"))
}

/// `fetch`'s error, in words for the result line.
fn failed(error: &FetchError) -> String {
    match error {
        FetchError::Refused(host) => format!("refused: {host} is not the host asked for"),
        FetchError::BadUrl(why) => format!("not a URL: {why}"),
        FetchError::Tls(why) => format!("the secure connection failed: {why}"),
        FetchError::Http(why) => format!("the server's answer was refused: {why}"),
        FetchError::TooLarge => String::from("the page is larger than 64 KiB"),
        FetchError::Timeout => String::from("the site did not answer in time"),
        FetchError::Truncated => String::from("the page was cut off"),
    }
}

/// The addresses `host` resolves to, on a handle for exactly it.
pub fn resolve(host: &str) -> Result<Vec<String>, String> {
    let host = host_name(host)?;
    let host = host.as_str();
    let dest = narrowed(host, Rights::RESOLVE.bits())?;
    let mut addresses = [[0u8; 4]; kabi::net::MAX_ADDRESSES];
    let found = sys::resolve_on(dest, host, &mut addresses);
    let _ = sys::handle_close(dest);
    let count = found.map_err(|e| format!("{host} did not resolve: {e:?}"))?;
    Ok(addresses[..count.min(addresses.len())]
        .iter()
        .map(|[a, b, c, d]| format!("{a}.{b}.{c}.{d}"))
        .collect())
}
