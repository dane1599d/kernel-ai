//! Test service for resolving on a `NetDest` handle (P5 task 1), started by the
//! kernel's `net-setup` and `net-dest` cases with the handle moved in:
//! - `host=NAME expect=A.B.C.D`: RESOLVE_ON with its startup `NetDest`, and
//!   exits 0 if the first address is the expected one;
//! - `expect=permission`: exits 0 if the kernel refuses it (Permission);
//! - `narrow=SPEC`: first makes a narrower handle from it, and resolves on that;
//!   `expect=narrow-refused` exits 0 if the kernel refuses to narrow;
//! - `legacy`: uses the retired RESOLVE call instead, which always refuses.
//! - `http=refused`: started with NET and a moved-in `NetDest`, but no
//!   `netdest=llm`: exits 0 if it holds no handle like the LLM's (CONNECT
//!   without RESOLVE) and HTTP is refused, since only the LLM's handle reaches
//!   the LLM and NET alone does not give it.
//! - `llm=held` / `llm=none`: started through a plan line's `netdest=llm`,
//!   exits 0 if its only `NetDest` is CONNECT exactly (with NET), or if it
//!   holds none (without NET in its ceiling).
//! - `any=pinned`: with a `hosts:any` handle (#760), exits 0 if narrowing it
//!   to `hosts:any` again, or to a suffix, is refused and narrowing to one
//!   exact host is not.
//!
//! And for TCP streams on it (P5 task 2, the `net-stream` case), with
//! `port=P` the echo server's port on 10.0.2.2:
//! - `stream=echo`: a stream and a bulk one each echo 8 KiB and a short line;
//!   9 KiB in one send is TooBig; port P+1, outside the handle, is refused; the
//!   17th stream is Busy; each closes.
//! - `stream=hold`: holds 3 connected streams and waits to be killed.
//! - `stream=revoke`: its `NetDest` was given under a grant node. It narrows
//!   it, echoes a line on a stream on each, then opens a third: 3 open is
//!   the kernel's sign to revoke the node. A poll waiting across the revoke,
//!   and from then on every use of the streams, the `NetDest` and the
//!   narrowing, answers Revoked; closing the streams still works (bug hunt
//!   2's H2-18).
//! - `stream=replace`: 17 streams in turn, each refused a HANDLE_REPLACE (a
//!   wider one, or a word with an undefined bit), which closes the handle
//!   and so the stream: the 17th connect is not Busy (bug hunt 2's H2-17).

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

/// `http=refused`: HTTP from a NET holder whose only `NetDest` is not the LLM's.
fn http_refused() -> i32 {
    let mut list = [HandleInfo::default(); 16];
    let count = sys::startup(&mut list).map_or(0, |n| n.min(list.len()));
    let dests = list[..count]
        .iter()
        .filter(|info| info.kind == Kind::NetDest as u32);
    let (mut kept, mut llm_like) = (0, 0);
    for info in dests {
        match info.rights & Rights::RESOLVE.bits() {
            0 => llm_like += 1,
            _ => kept += 1,
        }
    }
    if llm_like != 0 || kept == 0 {
        println!("http: started with {llm_like} LLM-like and {kept} other NetDest handles");
        return 1;
    }
    match sys::http(&kabi::HttpRequest::default()) {
        Err(Error::Permission) => {
            println!(
                "http without the LLM's own handle refused, with NET and {kept} other NetDest held"
            );
            0
        }
        other => {
            println!("http without the LLM's own handle gave {other:?}");
            1
        }
    }
}

/// `llm=held`: started from a plan line's `netdest=llm` with NET, so its only
/// `NetDest` is the LLM's, with exactly CONNECT. `llm=none`: the same grant
/// without NET in the ceiling, so it holds no `NetDest` at all.
fn llm_grant(expect: &str) -> i32 {
    let mut list = [HandleInfo::default(); 16];
    let count = sys::startup(&mut list).map_or(0, |n| n.min(list.len()));
    let rights: alloc::vec::Vec<u32> = list[..count]
        .iter()
        .filter(|info| info.kind == Kind::NetDest as u32)
        .map(|info| info.rights)
        .collect();
    let ok = match expect {
        "held" => rights.as_slice() == [Rights::CONNECT.bits()],
        "none" => rights.is_empty(),
        _ => false,
    };
    if !ok {
        println!("llm={expect}: started with NetDest rights {rights:?}");
        return 1;
    }
    println!("llm={expect}: the startup NetDest handles were as the ceiling allows");
    0
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
    let suffix = sys::netdest_narrow(dest, "proto:tcp,ports:443,hosts:co.uk", rights);
    let one = sys::netdest_narrow(
        dest,
        "proto:tcp,ports:443,hosts:kai.test,match:exact",
        rights,
    );
    match (again, suffix, one) {
        (Err(Error::Permission), Err(Error::Permission), Ok(_)) => {
            println!("hosts:any narrowed to one exact host, not to a suffix or hosts:any again");
            0
        }
        (again, suffix, one) => {
            println!("hosts:any narrowing gave {again:?}, {suffix:?} and {one:?}");
            1
        }
    }
}

fn main() -> i32 {
    let args = args();
    if arg(&args, "http") == Some("refused") {
        return http_refused();
    }
    if arg(&args, "any") == Some("pinned") {
        return any_pinned();
    }
    if let Some(expect) = arg(&args, "llm") {
        return llm_grant(expect);
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
            "revoke" => stream_revoke(dest, port),
            "replace" => stream_replace(dest, port),
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
            "usage: netcheck host=NAME expect=A.B.C.D|permission|private|narrow-refused [narrow=SPEC] [legacy]"
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
        (Err(Error::Private), "private") => {
            println!("resolve {host} refused: it leads only to private addresses");
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
/// `stream=revoke`'s wait for the kernel's revoke: NET_POLL's longest.
const REVOKE_WAIT_MS: u64 = 60_000;

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

/// `stream=revoke`: see the module's comment. `dest` was given under a grant
/// node the kernel revokes once this holds 3 streams.
fn stream_revoke(dest: Handle, port: u16) -> Result<(), String> {
    let spec = format!("proto:tcp,addrs:10.0.2.2,ports:{port}");
    let rights = (Rights::CONNECT | Rights::RESOLVE | Rights::DUPLICATE | Rights::INSPECT).bits();
    let narrowed =
        sys::netdest_narrow(dest, &spec, rights).map_err(|e| format!("narrowing: {e:?}"))?;
    let first = open(dest, port, 0)?;
    let second = open(narrowed, port, 0)?;
    let line = b"before the revoke\n";
    for stream in [first, second] {
        if echo(stream, line)? != line {
            return Err(String::from(
                "a stream echoed other bytes before the revoke",
            ));
        }
    }
    // The third makes 3 open: the kernel revokes the node then.
    let third =
        sys::net_connect(dest, SERVER, port, 0).map_err(|e| format!("the third stream: {e:?}"))?;
    // Nothing comes unasked: only the revoke ends this wait.
    match sys::net_poll(first, kabi::net::STREAM_READABLE, REVOKE_WAIT_MS) {
        Err(Error::Revoked) => {}
        other => return Err(format!("a poll across the revoke gave {other:?}")),
    }
    let mut buf = [0u8; 16];
    let mut addresses = [[0u8; 4]; 1];
    let uses = [
        ("a send on the stream", sys::net_send(first, line).map(drop)),
        (
            "a receive on the narrowing's stream",
            sys::net_recv(second, &mut buf).map(drop),
        ),
        (
            "a poll on the third stream",
            sys::net_poll(third, kabi::net::STREAM_CONNECTED, 0).map(drop),
        ),
        (
            "a connect on the NetDest",
            sys::net_connect(dest, SERVER, port, 0).map(drop),
        ),
        (
            "a connect on its narrowing",
            sys::net_connect(narrowed, SERVER, port, 0).map(drop),
        ),
        (
            "a narrowing of the NetDest",
            sys::netdest_narrow(dest, &spec, rights).map(drop),
        ),
        (
            "a narrowing of the narrowing",
            sys::netdest_narrow(narrowed, &spec, rights).map(drop),
        ),
        (
            "a resolve on the narrowing",
            sys::resolve_on(narrowed, "kai.test", &mut addresses).map(drop),
        ),
        (
            "a duplicate of the narrowing",
            sys::handle_dup(narrowed, Rights::CONNECT).map(drop),
        ),
    ];
    for (what, result) in uses {
        if result != Err(Error::Revoked) {
            return Err(format!("{what} after the revoke gave {result:?}"));
        }
    }
    // A revoked stream's handle still closes, either way.
    sys::net_close(first).map_err(|e| format!("closing the first stream: {e:?}"))?;
    sys::handle_close(second).map_err(|e| format!("closing the second stream: {e:?}"))?;
    sys::net_close(third).map_err(|e| format!("closing the third stream: {e:?}"))?;
    if sys::handle_info(first).map(drop) != Err(Error::BadHandle) {
        return Err(String::from("a closed revoked stream's handle stayed open"));
    }
    println!(
        "after the revoke, a waiting poll, the streams, the NetDest and its narrowing each answered Revoked, and the streams closed"
    );
    Ok(())
}

/// `stream=replace`: see the module's comment.
fn stream_replace(dest: Handle, port: u16) -> Result<(), String> {
    let undefined = Rights::from_bits_retain(1 << 31);
    for n in 1..=kabi::net::STREAMS_PER_PROCESS + 1 {
        let stream = sys::net_connect(dest, SERVER, port, 0)
            .map_err(|e| format!("stream {n} after {} refused replaces: {e:?}", n - 1))?;
        let asked = match n % 2 {
            0 => undefined,
            _ => Rights::READ | Rights::DUPLICATE,
        };
        match sys::handle_replace(stream, asked) {
            Err(Error::Invalid) => {}
            other => {
                return Err(format!(
                    "replacing stream {n} with {asked:?} gave {other:?}"
                ));
            }
        }
        if sys::handle_info(stream).map(drop) != Err(Error::BadHandle) {
            return Err(format!("a refused replace left stream {n}'s handle open"));
        }
    }
    println!(
        "17 streams were each refused a HANDLE_REPLACE, wider or with an undefined bit, which closed them: none was Busy"
    );
    Ok(())
}
