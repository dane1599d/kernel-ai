//! Spike S04-2 (ADR 0004), boot case `test=fs-ipc`: a stub `fs` service, built for
//! the soft-float ring-3 target `fs` will ship on. It answers raw 8 KiB calls and
//! `kai-fs 1`-shaped file requests over today's copied IPC (at most 8 KiB a
//! message), holding one file of up to 64 MiB in memory as `fs`'s block cache
//! would. As a write completes each 128 KiB record it checksums the record, as
//! kaifs would before writing it, with the kind the client opened the file with;
//! a read can verify each record it starts, as `fs` would when loading one from
//! the disk. Built only on the never-merged branch spike/fs-ipc.

#![no_std]
#![no_main]

extern crate alloc;

#[path = "../fsipc_proto.rs"]
mod proto;

use alloc::vec::Vec;
use kabi::MAX_MESSAGE;
use kabi::handle::RecvInfo;
use minicbor::encode::write::Cursor;
use minicbor::{Decoder, Encoder};
use proto::*;
use sha2::{Digest, Sha256};
use user::{println, sys};

user::entry!(main);

/// The largest file the client may open: the spike's 64 MiB.
const FILE_MAX: usize = 64 << 20;

struct File {
    data: Vec<u8>,
    /// The checksum kept for each record, and the kind they were made with.
    sums: Vec<[u8; 32]>,
    csum: u64,
    /// The kind each read verifies with, as if loading records from the disk.
    verify: u64,
}

fn sum(kind: u64, record: &[u8]) -> [u8; 32] {
    let mut out = [0; 32];
    match kind {
        CSUM_SHA256 => out = Sha256::digest(record).into(),
        CSUM_SEAHASH => out[..8].copy_from_slice(&seahash::hash(record).to_le_bytes()),
        _ => {}
    }
    out
}

impl File {
    fn open(&mut self, len: usize, csum: u64) -> Result<(), u32> {
        if len > FILE_MAX || !len.is_multiple_of(RECORD) {
            return Err(OUT_OF_RANGE);
        }
        self.data.clear();
        self.data.resize(len, 0);
        self.sums.clear();
        self.sums.resize(len / RECORD, [0; 32]);
        self.csum = csum;
        self.verify = CSUM_NONE;
        Ok(())
    }

    fn write(&mut self, offset: usize, bytes: &[u8]) -> Result<(), u32> {
        let end = offset.checked_add(bytes.len()).ok_or(OUT_OF_RANGE)?;
        let target = self.data.get_mut(offset..end).ok_or(OUT_OF_RANGE)?;
        target.copy_from_slice(bytes);
        if self.csum != CSUM_NONE {
            // Every record whose last byte this write supplied is complete.
            for record in offset / RECORD..end / RECORD {
                let bytes = &self.data[record * RECORD..(record + 1) * RECORD];
                self.sums[record] = sum(self.csum, bytes);
            }
        }
        Ok(())
    }

    fn read(&self, offset: usize, len: usize) -> Result<&[u8], u32> {
        let end = offset.checked_add(len).ok_or(OUT_OF_RANGE)?;
        let bytes = self.data.get(offset..end).ok_or(OUT_OF_RANGE)?;
        if self.verify != CSUM_NONE {
            // Records that begin inside this read are loaded, and so checked, here.
            for record in offset.div_ceil(RECORD)..end.div_ceil(RECORD) {
                let start = record * RECORD;
                let whole = &self.data[start..start + RECORD];
                if self.verify == self.csum && sum(self.verify, whole) != self.sums[record] {
                    return Err(BAD_CHECKSUM);
                }
                if self.verify != self.csum {
                    core::hint::black_box(sum(self.verify, whole));
                }
            }
        }
        Ok(bytes)
    }

    fn csum_bench(&self, kind: u64) -> u64 {
        let start = sys::time_ns();
        for record in self.data.as_chunks::<RECORD>().0 {
            core::hint::black_box(sum(kind, core::hint::black_box(record)));
        }
        sys::time_ns() - start
    }
}

/// Answers one raw request in `reply`; its length.
fn raw(request: &[u8], reply: &mut [u8]) -> usize {
    match request[0] {
        RAW_ECHO => {
            reply[..request.len()].copy_from_slice(request);
            request.len()
        }
        RAW_SOURCE => {
            reply[..MAX_MESSAGE].fill(0x5A);
            MAX_MESSAGE
        }
        _ => {
            reply[..RAW_SMALL].fill(0);
            reply[..8].copy_from_slice(&(request.len() as u64).to_le_bytes());
            RAW_SMALL
        }
    }
}

/// Answers one CBOR request in `reply`: its length, and whether to stop.
fn file_request(file: &mut File, request: &[u8], reply: &mut [u8]) -> (usize, bool) {
    let mut decoder = Decoder::new(request);
    let parsed = (|| -> Result<(u32, u64, u64, &[u8]), minicbor::decode::Error> {
        decoder.array()?;
        let version = decoder.u32()?;
        let op = decoder.u32()?;
        let a = decoder.u64()?;
        let b = decoder.u64()?;
        let data = decoder.bytes()?;
        if version != VERSION {
            return Err(minicbor::decode::Error::message("version"));
        }
        Ok((op, a, b, data))
    })();
    let mut quit = false;
    let mut value = 0u64;
    let mut out: &[u8] = &[];
    let status = match parsed {
        Err(_) => BAD_REQUEST,
        Ok((op, a, b, data)) => {
            let result = match op {
                OP_OPEN => file.open(a as usize, b),
                OP_WRITE => file.write(a as usize, data),
                OP_READ if b as usize <= DATA_MAX => {
                    file.read(a as usize, b as usize).map(|bytes| {
                        out = bytes;
                    })
                }
                OP_VERIFY => {
                    file.verify = b;
                    Ok(())
                }
                OP_DIGEST => {
                    value = seahash::hash(&file.data);
                    Ok(())
                }
                OP_CSUM_BENCH => {
                    value = file.csum_bench(b);
                    Ok(())
                }
                OP_QUIT => {
                    quit = true;
                    Ok(())
                }
                _ => Err(BAD_REQUEST),
            };
            result.err().unwrap_or(OK)
        }
    };
    let mut encoder = Encoder::new(Cursor::new(&mut reply[..]));
    let encoded = encoder
        .array(3)
        .and_then(|e| e.u32(status))
        .and_then(|e| e.u64(value))
        .and_then(|e| e.bytes(out))
        .is_ok();
    let len = if encoded {
        encoder.writer().position()
    } else {
        0
    };
    (len, quit)
}

fn main() -> i32 {
    let mut file = File {
        data: Vec::new(),
        sums: Vec::new(),
        csum: CSUM_NONE,
        verify: CSUM_NONE,
    };
    if file.data.try_reserve_exact(FILE_MAX).is_err() {
        println!("fs-stub: cannot reserve the 64 MiB cache");
        return 1;
    }
    let mut request = alloc::vec![0u8; MAX_MESSAGE];
    let mut reply = alloc::vec![0u8; MAX_MESSAGE];
    let mut calls = 0u64;
    loop {
        let mut info = RecvInfo::default();
        let len = match sys::recv_h(&mut request, 60_000, &mut info) {
            Ok(0) => continue,
            Ok(len) => len.min(MAX_MESSAGE),
            Err(error) => {
                println!("fs-stub: receive failed: {error:?}");
                return 1;
            }
        };
        calls += 1;
        let body = &request[..len];
        let (reply_len, quit) = match body[0] {
            RAW_ECHO | RAW_SINK | RAW_SOURCE => (raw(body, &mut reply), false),
            _ => file_request(&mut file, body, &mut reply),
        };
        if let Err(error) = sys::reply_h(&reply[..reply_len], &[]) {
            println!("fs-stub: reply failed: {error:?}");
            return 1;
        }
        if quit {
            println!("fs-stub: answered {calls} calls");
            return 0;
        }
    }
}
