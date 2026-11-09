//! Test service for resolving on a `NetDest` handle (P5 task 1), started by the
//! kernel's `net-setup` and `net-dest` cases with the handle moved in:
//! - `host=NAME expect=A.B.C.D`: RESOLVE_ON with its startup `NetDest`, and
//!   exits 0 if the first address is the expected one;
//! - `expect=permission`: exits 0 if the kernel refuses it (Permission);
//! - `narrow=SPEC`: first makes a narrower handle from it, and resolves on that;
//!   `expect=narrow-refused` exits 0 if the kernel refuses to narrow;
//! - `legacy`: uses the retired RESOLVE call instead, which always refuses.
//! - `any=pinned`: with a `hosts:any` handle (#760), exits 0 if narrowing it
//!   to `hosts:any` again is refused and narrowing to one exact host is not.
//!
//! And for TCP streams on it (P5 task 2, the `net-stream` case), with
//! `port=P` the echo server's port on 10.0.2.2:
//! - `stream=echo`: a stream and a bulk one each echo 8 KiB and a short line;
//!   9 KiB in one send is TooBig; port P+1, outside the handle, is refused; the
//!   17th stream is Busy; each closes.
//! - `stream=hold`: holds 3 connected streams and waits to be killed.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use core::net::Ipv4Addr;
use kabi::Error;
use kabi::handle::{Handle, HandleInfo, Kind, Rights};
use user::{arg, args, println, sys};

user::entry!(main);

/// The startup `NetDest` handle, if the kernel moved one in.
fn startup_dest() -> Option<Handle> {
    let mut list = [HandleInfo::default(); 16];
    let count = sys::startup(&mut list).ok()?.min(list.len());
    list[..count]
        .iter()
        .find(|info| info.kind == Kind::NetDest as u32)
        .map(|info| Handle(info.handle))
}

/// `any=pinned`: no narrowing of a `hosts:any` handle keeps `hosts:any`; one
/// exact host is fine.
fn any_pinned() -> i32 {
    let Some(dest) = startup_dest() else {
        println!("any: no NetDest handle");
        return 1;
    };
    let rights = (Rights::RESOLVE | Rights::CONNECT).bits();
    let again = sys::netdest_narrow(dest, "proto:tcp,ports:443,hosts:any", rights);
    let one = sys::netdest_narrow(
        dest,
        "proto:tcp,ports:443,hosts:kai.test,match:exact",
        rights,
    );
    match (again, one) {
        (Err(Error::Permission), Ok(_)) => {
            println!("hosts:any narrowed to one exact host, and not to hosts:any again");
            0
        }
        (again, one) => {
            println!("hosts:any narrowing gave {again:?} and {one:?}");
            1
        }
    }
}

fn main() -> i32 {
    let args = args();
    if arg(&args, "any") == Some("pinned") {
        return any_pinned();
    }
    if let Some(mode) = arg(&args, "stream") {
        let port = arg(&args, "port").and_then(|p| p.parse().ok()).unwrap_or(0);
        let Some(dest) = startup_dest() else {
            println!("stream: no NetDest handle");
            return 1;
        };
        let result = match mode {
            "echo" => stream_echo(dest, port),
            "hold" => stream_hold(dest, port),
            _ => Err(format!("no stream mode {mode}")),
        };
        return match result {
            Ok(()) => 0,
            Err(why) => {
                println!("stream: {why}");
                1
            }
        };
    }
    let (Some(host), Some(expect)) = (arg(&args, "host"), arg(&args, "expect")) else {
        println!(
            "usage: netcheck host=NAME expect=A.B.C.D|permission|narrow-refused [narrow=SPEC] [legacy]"
        );
        return 2;
    };
    let mut addresses = [[0u8; 4]; kabi::net::MAX_ADDRESSES];
    let answer = match (
        args.split_whitespace().any(|w| w == "legacy"),
        startup_dest(),
    ) {
        (true, _) => sys::resolve(host, &mut addresses),
        (false, None) => Err(Error::Permission),
        (false, Some(dest)) => {
            let dest = match arg(&args, "narrow") {
                None => dest,
                Some(spec) => {
                    let rights = (Rights::RESOLVE | Rights::INSPECT).bits();
                    match (sys::netdest_narrow(dest, spec, rights), expect) {
                        (Err(Error::Permission), "narrow-refused") => {
                            println!("narrowing to {spec} refused: it would allow more");
                            return 0;
                        }
                        (Ok(narrow), _) => narrow,
                        (Err(error), _) => {
                            println!("narrowing to {spec}: unexpected {error:?}");
                            return 1;
                        }
                    }
                }
            };
            sys::resolve_on(dest, host, &mut addresses)
        }
    };
    match (answer, expect) {
        (Err(Error::Permission), "permission") => {
            println!("resolve {host} refused: not under this handle's hosts");
            0
        }
        (Ok(count), expected) if count > 0 => {
            let got = format!("{}", Ipv4Addr::from(addresses[0]));
            println!("resolved {host} to {got} ({count} address(es))");
            i32::from(got != expected)
        }
        (other, _) => {
            println!("resolve {host}: unexpected {other:?}");
            1
        }
    }
}

const SERVER: [u8; 4] = [10, 0, 2, 2];
const WAIT_MS: u64 = 10_000;

/// A stream to the echo server, connected.
fn open(dest: Handle, port: u16, flags: u64) -> Result<Handle, String> {
    let stream = sys::net_connect(dest, SERVER, port, flags)
        .map_err(|e| format!("connect to port {port}: {e:?}"))?;
    let state = sys::net_poll(stream, kabi::net::STREAM_CONNECTED, WAIT_MS)
        .map_err(|e| format!("poll: {e:?}"))?;
    match state & kabi::net::STREAM_CONNECTED != 0 {
        true => Ok(stream),
        false => Err(format!("port {port} did not connect (state {state:#x})")),
    }
}

/// Sends all of `data` and reads the same number of bytes back.
fn echo(stream: Handle, data: &[u8]) -> Result<alloc::vec::Vec<u8>, String> {
    let mut sent = 0;
    while sent < data.len() {
        sys::net_poll(stream, kabi::net::STREAM_WRITABLE, WAIT_MS)
            .map_err(|e| format!("poll: {e:?}"))?;
        sent += sys::net_send(stream, &data[sent..]).map_err(|e| format!("send: {e:?}"))?;
    }
    let mut back = alloc::vec::Vec::new();
    let mut chunk = [0u8; 4096];
    while back.len() < data.len() {
        let state = sys::net_poll(
            stream,
            kabi::net::STREAM_READABLE | kabi::net::STREAM_CLOSED,
            WAIT_MS,
        )
        .map_err(|e| format!("poll: {e:?}"))?;
        if state & kabi::net::STREAM_READABLE == 0 {
            return Err(format!(
                "{} of {} bytes came back (state {state:#x})",
                back.len(),
                data.len()
            ));
        }
        let n = sys::net_recv(stream, &mut chunk).map_err(|e| format!("recv: {e:?}"))?;
        back.extend_from_slice(&chunk[..n]);
    }
    Ok(back)
}

fn stream_echo(dest: Handle, port: u16) -> Result<(), String> {
    let big: alloc::vec::Vec<u8> = (0..8192u32).map(|i| (i * 7 + 3) as u8).collect();
    for (name, flags) in [("a stream", 0), ("a bulk stream", kabi::net::STREAM_BULK)] {
        let stream = open(dest, port, flags)?;
        for data in [&big[..], b"hello over a handle\n"] {
            if echo(stream, data)? != data {
                return Err(format!("{name} echoed other bytes"));
            }
        }
        let over = alloc::vec![0u8; kabi::net::STREAM_MAX_CALL + 1];
        if sys::net_send(stream, &over) != Err(Error::TooBig) {
            return Err(format!("{name} took a send over 8 KiB"));
        }
        sys::net_close(stream).map_err(|e| format!("close: {e:?}"))?;
        println!("{name} echoed 8192 and 20 bytes, refused 8193 in one send, and closed");
    }
    match sys::net_connect(dest, SERVER, port + 1, 0) {
        Err(Error::Permission) => println!("port {} is not on the handle: refused", port + 1),
        other => {
            return Err(format!(
                "port {} outside the handle gave {other:?}",
                port + 1
            ));
        }
    }
    let mut streams = alloc::vec::Vec::new();
    for _ in 0..kabi::net::STREAMS_PER_PROCESS {
        streams.push(
            sys::net_connect(dest, SERVER, port, 0)
                .map_err(|e| format!("stream {}: {e:?}", streams.len() + 1))?,
        );
    }
    match sys::net_connect(dest, SERVER, port, 0) {
        Err(Error::Busy) => println!("16 streams open; the 17th was Busy"),
        other => return Err(format!("the 17th stream gave {other:?}")),
    }
    // Half by NET_CLOSE, half by closing the handle: either frees the stream.
    for (i, stream) in streams.into_iter().enumerate() {
        match i % 2 {
            0 => sys::net_close(stream).map_err(|e| format!("close: {e:?}"))?,
            _ => sys::handle_close(stream).map_err(|e| format!("handle close: {e:?}"))?,
        }
    }
    let mut again = alloc::vec::Vec::new();
    for _ in 0..kabi::net::STREAMS_PER_PROCESS {
        again.push(
            sys::net_connect(dest, SERVER, port, 0)
                .map_err(|e| format!("reopening stream {}: {e:?}", again.len() + 1))?,
        );
    }
    println!("after closing them, by NET_CLOSE or by the handle, 16 more opened");
    for stream in again {
        sys::handle_close(stream).map_err(|e| format!("handle close: {e:?}"))?;
    }
    let bulks: alloc::vec::Vec<_> = (0..3)
        .map(|_| sys::net_connect(dest, SERVER, port, kabi::net::STREAM_BULK))
        .collect();
    match bulks.as_slice() {
        [Ok(_), Ok(_), Err(Error::Busy)] => println!("a third bulk stream of one process was Busy"),
        other => return Err(format!("three bulk streams gave {other:?}")),
    }
    Ok(())
}

fn stream_hold(dest: Handle, port: u16) -> Result<(), String> {
    let mut streams = alloc::vec::Vec::new();
    for _ in 0..3 {
        streams.push(open(dest, port, 0)?);
    }
    println!("holding 3 streams");
    loop {
        sys::sleep_ms(1_000);
    }
}
