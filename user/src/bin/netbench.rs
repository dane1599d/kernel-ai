//! Spike S07-4's client (G37, `spike/netd-ring3` only): one download of
//! `bytes=` from `server=`'s `/blob`, by one of two paths, started by the
//! kernel's `netd-spike` case:
//! - `mode=kernel`: through the kernel's stack, on its startup `NetDest`, with a
//!   bulk stream and NET_RECV, as a ring-3 program reaches the network today;
//! - `mode=ring3`: from `netd` over the KRNG v1 ring that is its startup `Shm`:
//!   it keeps [`SLOTS`] requests out, each a span of the ring's data area, and
//!   counts what each response says is there.
//!
//! It prints its MB/s from the first byte of the body, and exits 0 once every
//! byte came.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicU32, Ordering};
use kabi::Error;
use kabi::handle::{Handle, HandleInfo, Kind};
use kabi::net::{STREAM_BULK, STREAM_CLOSED, STREAM_CONNECTED, STREAM_FAILED, STREAM_READABLE};
use kabi::ring::{self, Request};
use user::{arg, args, println, sys};

user::entry!(main);

const RING_ENTRIES: u32 = 256;
const RING_DATA: usize = 1 << 20;
/// Requests kept out at once, each a sixteenth of the data area.
const SLOTS: u32 = 16;
const RSP_HEAD: usize = 20;
const REQ_HEAD: usize = 12;

fn main() -> i32 {
    let args = args();
    let total: u64 = arg(&args, "bytes")
        .and_then(|n| n.parse().ok())
        .unwrap_or(0);
    let mut list = [HandleInfo::default(); 8];
    let count = sys::startup(&mut list).unwrap_or(0).min(list.len());
    let find = |kind: Kind| {
        list[..count]
            .iter()
            .find(|info| info.kind == kind as u32)
            .map(|info| Handle(info.handle))
    };
    let result = match arg(&args, "mode") {
        Some("kernel") => match (find(Kind::NetDest), arg(&args, "server").and_then(endpoint)) {
            (Some(dest), Some((addr, port))) => kernel(dest, addr, port, total),
            _ => Err("no NetDest or server="),
        },
        Some("ring3") => match find(Kind::Shm) {
            Some(shm) => ring3(shm, total),
            None => Err("no ring"),
        },
        _ => Err("no mode="),
    };
    match result {
        Ok((first_ns, end_ns)) => {
            let ms = ((end_ns - first_ns) / 1_000_000).max(1);
            let tenths = total * 10 / ms / 1000;
            println!(
                "netbench: {total} bytes in {ms} ms = {}.{} MB/s",
                tenths / 10,
                tenths % 10
            );
            0
        }
        Err(why) => {
            println!("netbench: {why}");
            1
        }
    }
}

/// Through the kernel's stack: the times of the body's first byte and last.
fn kernel(dest: Handle, addr: [u8; 4], port: u16, total: u64) -> Result<(u64, u64), &'static str> {
    let stream = sys::net_connect(dest, addr, port, STREAM_BULK).map_err(|_| "cannot connect")?;
    let state = sys::net_poll(stream, STREAM_CONNECTED, 20_000).map_err(|_| "no connection")?;
    if state & STREAM_CONNECTED == 0 {
        return Err("not connected");
    }
    let request = alloc::format!(
        "GET /blob HTTP/1.0\r\nHost: {}.{}.{}.{}\r\n\r\n",
        addr[0],
        addr[1],
        addr[2],
        addr[3]
    );
    sys::net_send(stream, request.as_bytes()).map_err(|_| "cannot send the request")?;
    let mut buffer = vec![0u8; kabi::net::STREAM_MAX_CALL];
    let mut head = alloc::vec::Vec::new();
    let mut body: u64 = 0;
    let mut first = None;
    while body < total {
        match sys::net_recv(stream, &mut buffer) {
            Ok(0) => break,
            Ok(n) if first.is_none() => {
                head.extend_from_slice(&buffer[..n]);
                if let Some(end) = head.windows(4).position(|w| w == b"\r\n\r\n") {
                    first = Some(sys::time_ns());
                    body += (head.len() - end - 4) as u64;
                }
            }
            Ok(n) => body += n as u64,
            Err(Error::Busy) => {
                let want = STREAM_READABLE | STREAM_CLOSED;
                let state = sys::net_poll(stream, want, 1000).map_err(|_| "poll failed")?;
                if state & STREAM_FAILED != 0 {
                    return Err("the stream failed");
                }
            }
            Err(_) => return Err("receive failed"),
        }
    }
    let end = sys::time_ns();
    let _ = sys::net_close(stream);
    match (first, body >= total) {
        (Some(first), true) => Ok((first, end)),
        _ => Err("the body ended short"),
    }
}

/// From `netd` over the ring: the times of the first response with bytes and
/// the last.
fn ring3(shm: Handle, total: u64) -> Result<(u64, u64), &'static str> {
    let va = sys::shm_map(shm, true).map_err(|_| "cannot map the ring")? as usize;
    let len = ring::size(RING_ENTRIES, RING_DATA);
    // `netd` makes the ring; wait for it.
    let mut attached = None;
    for _ in 0..2000 {
        // SAFETY: the ring object is mapped read-write at `va` for `len` bytes.
        if let Ok(krng) = unsafe { ring::Ring::attach(NonNull::new(va as *mut u8).unwrap(), len) } {
            attached = Some(krng);
            break;
        }
        sys::sleep_ms(5);
    }
    let krng = attached.ok_or("netd never made the ring")?;
    let (start, area) = krng.data_area();
    let (mut client, _) = krng.split();
    let span = area / SLOTS as usize;
    let req_head = (va + REQ_HEAD) as *const AtomicU32;
    let rsp_head = (va + RSP_HEAD) as *const AtomicU32;
    let ask = |client: &mut ring::Client, slot: u32| {
        let request = Request {
            tag: u64::from(slot),
            op: 1,
            offset: (start + slot as usize * span) as u64,
            len: span as u32,
            ..Request::default()
        };
        matches!(client.requests.push(&request), Ok(Ok(())))
    };
    for slot in 0..SLOTS {
        if !ask(&mut client, slot) {
            return Err("cannot make a request");
        }
    }
    if client.server_wants_requests() {
        let _ = sys::futex_wake(req_head, 1);
    }
    let mut got: u64 = 0;
    let mut first = None;
    while got < total {
        match client.responses.pop() {
            Err(_) => return Err("the ring is corrupt"),
            Ok(Some(response)) => {
                if response.status != 0 {
                    return Err("netd answered an error");
                }
                if response.len > 0 && first.is_none() {
                    first = Some(sys::time_ns());
                }
                got += u64::from(response.len);
                if !ask(&mut client, response.tag as u32) {
                    return Err("cannot make a request");
                }
                if client.server_wants_requests() {
                    let _ = sys::futex_wake(req_head, 1);
                }
            }
            Ok(None) => {
                // SAFETY: the ring's response head, in the mapping.
                let seen = unsafe { (*rsp_head).load(Ordering::Acquire) };
                client.want_responses(true);
                if client.responses.ready().unwrap_or(0) == 0 {
                    let _ = sys::futex_wait(rsp_head, seen, 100);
                }
                client.want_responses(false);
            }
        }
    }
    Ok((first.ok_or("no bytes came")?, sys::time_ns()))
}

fn endpoint(text: &str) -> Option<([u8; 4], u16)> {
    let (ip, port) = text.split_once(':')?;
    let mut octets = [0u8; 4];
    let mut parts = ip.split('.');
    for octet in &mut octets {
        *octet = parts.next()?.parse().ok()?;
    }
    Some((octets, port.parse().ok()?))
}
