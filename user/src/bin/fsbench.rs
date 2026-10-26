//! Spike S04-2 (ADR 0004), boot case `test=fs-ipc`: the client of the stub `fs`,
//! started with one route to it. Through that route it makes:
//!
//! - 10,000 raw calls of 8 KiB each way, 10,000 of 8 KiB in with a 16-byte
//!   answer, and 10,000 of 16 bytes in with 8 KiB back;
//! - 1,000 more 8 KiB echo calls, each timed, for p50 and p99;
//! - a 64 MiB file written in 7 KiB `kai-fs 1` messages and read back, every byte
//!   checked, once with no checksum, once with seahash-64 and once with SHA-256
//!   kept per 128 KiB record (and verified on the read, as on a cold cache);
//! - both checksums over the whole file inside `fs`.
//!
//! It prints each figure, then ADR 0004's verdict: at least 50 MB/s each way keeps
//! copied IPC in v0.1. It exits 0 when every reply was right, whatever the
//! figures. Built only on the never-merged branch spike/fs-ipc.

#![no_std]
#![no_main]

extern crate alloc;

#[path = "../fsipc_proto.rs"]
mod proto;

use alloc::vec::Vec;
use kabi::MAX_MESSAGE;
use kabi::handle::Handle;
use minicbor::encode::write::Cursor;
use minicbor::{Decoder, Encoder};
use proto::*;
use user::{println, sys};

user::entry!(main);

const CALLS: usize = 10_000;
const TIMED_CALLS: usize = 1_000;
const FILE_LEN: usize = 64 << 20;
const TIMEOUT_MS: u64 = 10_000;
/// ADR 0004's S04-2 bar, in bytes a second each way.
const BAR: u64 = 50_000_000;
/// The file's bytes repeat every 1 MiB; each 7 KiB piece starts with its offset.
const PATTERN: usize = 1 << 20;

struct Client {
    route: Handle,
    request: Vec<u8>,
    reply: Vec<u8>,
    pattern: Vec<u8>,
}

/// Bytes a second, from bytes moved in `ns` nanoseconds.
fn rate(bytes: u64, ns: u64) -> u64 {
    (u128::from(bytes) * 1_000_000_000 / u128::from(ns.max(1))) as u64
}

fn mb(rate: u64) -> u64 {
    rate / 1_000_000
}

impl Client {
    fn call(&mut self, len: usize) -> Result<usize, &'static str> {
        sys::call_route(
            self.route,
            &self.request[..len],
            &mut self.reply,
            TIMEOUT_MS,
        )
        .map_err(|_| "a call failed")
    }

    /// `count` raw calls of `op`, each sending `len` bytes; the nanoseconds taken.
    fn raw(&mut self, op: u8, len: usize, want: usize, count: usize) -> Result<u64, &'static str> {
        self.request[0] = op;
        let start = sys::time_ns();
        for _ in 0..count {
            if self.call(len)? != want {
                return Err("a raw reply had the wrong length");
            }
        }
        Ok(sys::time_ns() - start)
    }

    /// One `kai-fs 1`-shaped request; the reply's value and the range of its data
    /// in `self.reply`.
    fn file(
        &mut self,
        op: u32,
        a: u64,
        b: u64,
        data_len: usize,
    ) -> Result<(u64, usize, usize), &'static str> {
        let len = {
            let mut encoder = Encoder::new(Cursor::new(&mut self.request[..]));
            encoder
                .array(5)
                .and_then(|e| e.u32(VERSION))
                .and_then(|e| e.u32(op))
                .and_then(|e| e.u64(a))
                .and_then(|e| e.u64(b))
                .map_err(|_| "cannot encode a request")?;
            if op == OP_WRITE {
                let at = (a as usize) % PATTERN;
                encoder
                    .bytes(&self.pattern[at..at + data_len])
                    .map_err(|_| "cannot encode a request")?;
                let end = encoder.writer().position();
                // Each piece starts with its own offset, so a misplaced one shows.
                self.request[end - data_len..end - data_len + 8].copy_from_slice(&a.to_le_bytes());
                end
            } else {
                encoder.bytes(&[]).map_err(|_| "cannot encode a request")?;
                encoder.writer().position()
            }
        };
        let got = self.call(len)?;
        let mut decoder = Decoder::new(&self.reply[..got]);
        let (status, value, data) = (|| -> Result<(u32, u64, &[u8]), minicbor::decode::Error> {
            decoder.array()?;
            Ok((decoder.u32()?, decoder.u64()?, decoder.bytes()?))
        })()
        .map_err(|_| "a reply did not decode")?;
        if status != OK {
            println!("fs-ipc: op {op} at {a} answered status {status}");
            return Err("the stub answered an error");
        }
        let start = data.as_ptr() as usize - self.reply.as_ptr() as usize;
        Ok((value, start, data.len()))
    }

    /// Writes the whole file in 7 KiB pieces; the nanoseconds taken.
    fn write_file(&mut self) -> Result<u64, &'static str> {
        let start = sys::time_ns();
        let mut offset = 0;
        while offset < FILE_LEN {
            let len = DATA_MAX.min(FILE_LEN - offset);
            self.file(OP_WRITE, offset as u64, 0, len)?;
            offset += len;
        }
        Ok(sys::time_ns() - start)
    }

    /// Reads the whole file back in 7 KiB pieces, checking every byte; the
    /// nanoseconds taken, checks included.
    fn read_file(&mut self) -> Result<u64, &'static str> {
        let start = sys::time_ns();
        let mut offset = 0;
        while offset < FILE_LEN {
            let len = DATA_MAX.min(FILE_LEN - offset);
            let (_, at, got) = self.file(OP_READ, offset as u64, len as u64, 0)?;
            let data = &self.reply[at..at + got];
            let expected = &self.pattern[offset % PATTERN..offset % PATTERN + len];
            if got != len
                || data[..8] != (offset as u64).to_le_bytes()
                || data[8..] != expected[8..]
            {
                println!("fs-ipc: the piece at {offset} came back wrong");
                return Err("a read returned the wrong bytes");
            }
            offset += len;
        }
        Ok(sys::time_ns() - start)
    }
}

fn raw_phases(client: &mut Client) -> Result<(), &'static str> {
    let both = client.raw(RAW_ECHO, MAX_MESSAGE, MAX_MESSAGE, CALLS)?;
    let to_fs = client.raw(RAW_SINK, MAX_MESSAGE, RAW_SMALL, CALLS)?;
    let from_fs = client.raw(RAW_SOURCE, RAW_SMALL, MAX_MESSAGE, CALLS)?;
    let bytes = (CALLS * MAX_MESSAGE) as u64;
    for (what, ns) in [
        ("8 KiB each way", both),
        ("8 KiB to fs, 16 B back", to_fs),
        ("16 B to fs, 8 KiB back", from_fs),
    ] {
        println!(
            "fs-ipc: raw {CALLS} calls, {what}: {} ns a call, {} MB/s of 8 KiB messages",
            ns / CALLS as u64,
            mb(rate(bytes, ns))
        );
    }
    client.request[0] = RAW_ECHO;
    let mut times = Vec::with_capacity(TIMED_CALLS);
    for _ in 0..TIMED_CALLS {
        let start = sys::time_ns();
        client.call(MAX_MESSAGE)?;
        times.push(sys::time_ns() - start);
    }
    times.sort_unstable();
    println!(
        "fs-ipc: {TIMED_CALLS} timed 8 KiB echo calls: p50 {} ns, p99 {} ns, max {} ns",
        times[TIMED_CALLS / 2],
        times[TIMED_CALLS * 99 / 100],
        times[TIMED_CALLS - 1]
    );
    Ok(())
}

/// The file passes; each checksum kind with its write and read rates.
fn file_phases(client: &mut Client) -> Result<[(u64, u64, u64); 3], &'static str> {
    let mut rates = [(0, 0, 0); 3];
    for (n, kind) in [CSUM_NONE, CSUM_SEAHASH, CSUM_SHA256]
        .into_iter()
        .enumerate()
    {
        client.file(OP_OPEN, FILE_LEN as u64, kind, 0)?;
        let write = rate(FILE_LEN as u64, client.write_file()?);
        client.file(OP_VERIFY, 0, kind, 0)?;
        let read = rate(FILE_LEN as u64, client.read_file()?);
        let pieces = FILE_LEN.div_ceil(DATA_MAX);
        println!(
            "fs-ipc: 64 MiB file in {pieces} pieces of 7 KiB, checksum {}: write {} MB/s, read {} MB/s{}",
            csum_name(kind),
            mb(write),
            mb(read),
            if kind == CSUM_NONE {
                " (warm cache)"
            } else {
                " (each record verified)"
            }
        );
        rates[n] = (kind, write, read);
    }
    for kind in [CSUM_SEAHASH, CSUM_SHA256] {
        let (ns, _, _) = client.file(OP_CSUM_BENCH, 0, kind, 0)?;
        println!(
            "fs-ipc: {} inside fs over 64 MiB in 128 KiB records: {} MB/s",
            csum_name(kind),
            mb(rate(FILE_LEN as u64, ns))
        );
    }
    Ok(rates)
}

fn run() -> Result<(), &'static str> {
    let route = user::route(0).ok_or("no route to fs")?;
    let mut pattern = alloc::vec![0u8; PATTERN + DATA_MAX];
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    for byte in pattern.iter_mut() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *byte = x as u8;
    }
    let mut client = Client {
        route,
        request: alloc::vec![0u8; MAX_MESSAGE],
        reply: alloc::vec![0u8; MAX_MESSAGE],
        pattern,
    };
    raw_phases(&mut client)?;
    for (kind, write, read) in file_phases(&mut client)? {
        let pass = write >= BAR && read >= BAR;
        println!(
            "fs-ipc: bar 50 MB/s each way, checksum {}: write {} MB/s, read {} MB/s: {}",
            csum_name(kind),
            mb(write),
            mb(read),
            if pass { "pass" } else { "below the bar" }
        );
    }
    client.file(OP_QUIT, 0, 0, 0)?;
    Ok(())
}

fn main() -> i32 {
    match run() {
        Ok(()) => {
            println!("fs-ipc: done");
            0
        }
        Err(what) => {
            println!("fs-ipc: FAILED: {what}");
            1
        }
    }
}
