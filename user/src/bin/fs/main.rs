//! `fs` (ADR 0004 §6-7, P2 task 8): the file system service, the only process
//! that holds a `Partition` handle, for `kai-data`. It mounts kaifs there
//! (formatting a blank disk) and answers `kai-fs 1` on the `Dir` and `File`
//! handles it mints ([`fsserve`]). The kernel's own requests on a plain route
//! are the `part` case's: `check`, and S04-2's transfer through kaifs.

#![no_std]
#![no_main]

extern crate alloc;

mod check;
mod dev;

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use fsserve::{Call, Server, Started};
use kabi::handle::{Handle, HandleInfo, Kind, MAX_STARTUP, RecvInfo, Rights};
use kabi::part::stub;
use kabi::{Error, MAX_MESSAGE};
use user::{println, sys};

/// The `Partition` handle, for [`alive`].
static PARTITION: AtomicU32 = AtomicU32::new(0);

/// Whether a caller still runs: only a definite NotFound counts as dead.
fn alive(pid: u64) -> bool {
    let part = Handle(PARTITION.load(Ordering::Relaxed));
    sys::part_peer_alive(part, pid) != Err(Error::NotFound)
}

user::entry!(main);

/// How long a receive waits before a slice of the GC.
const IDLE_MS: u64 = 100;
/// The block cache in front of the disk: 16 MiB of 4 KiB blocks for now, under
/// the memory the kernel sets aside for services beside the model; the plan's
/// 64 MiB ([`kaifs::cache::DEFAULT_BLOCKS`]) waits for that budget to grow.
const CACHE_BLOCKS: usize = 4096;
/// The kernel's S04-2 file, in `system`.
const BENCH_FILE: &str = "s04-2";

fn main() -> i32 {
    let (Some(part), Some(endpoint)) = (startup(Kind::Partition), startup(Kind::Endpoint)) else {
        println!("fs: no Partition or Endpoint handle; nothing to serve");
        return 2;
    };
    let info = match sys::part_info(part) {
        Ok(info) => info,
        Err(error) => {
            println!("fs: kai-data cannot be reached: {error:?}");
            return 2;
        }
    };
    let mut uuid = [0u8; 16];
    uuid[..8].copy_from_slice(&sys::time_ns().to_le_bytes());
    uuid[8..].copy_from_slice(&info.sectors.to_le_bytes());
    let disk = kaifs::cache::Cached::new(dev::PartDev::new(part, &info), CACHE_BLOCKS);
    let mut server = match Server::start(disk, uuid) {
        Ok((server, started)) => {
            let how = match started {
                Started::Formatted => alloc::string::String::from("formatted it"),
                Started::Mounted { generation } => alloc::format!("generation {generation}"),
            };
            println!(
                "fs: up; kai-data has {} sectors ({} MiB), {how}",
                info.sectors,
                info.sectors / 2048
            );
            server
        }
        Err(error) => {
            println!("fs: kai-data does not mount: {error:?}");
            return 2;
        }
    };
    PARTITION.store(part.0, Ordering::Relaxed);
    server.set_alive(alive);
    let mut bench = Bench::default();
    let mut request = alloc::vec![0u8; MAX_MESSAGE];
    loop {
        let mut from = RecvInfo::default();
        let len = match sys::recv_h(&mut request, IDLE_MS, &mut from) {
            Ok(len) => len,
            Err(Error::TimedOut) => {
                server.idle(sys::time_ns());
                continue;
            }
            Err(_) => continue,
        };
        // `kai-fs 1` takes no handles yet; any a request carried are dropped.
        for &raw in &from.handles[..(from.handle_count as usize).min(from.handles.len())] {
            let _ = sys::handle_close(Handle(raw));
        }
        let request = &request[..len];
        if from.badge == 0 {
            let reply = match from.caller_pid {
                0 => bench.answer(&mut server, part, request),
                _ => b"err plain routes to fs are the kernel's".to_vec(),
            };
            let _ = sys::reply_h(&reply, &[]);
            continue;
        }
        let answer = server.call(&Call {
            caller: from.caller_pid,
            badge: from.badge,
            rights: Rights::from_bits_truncate(from.rights),
            request,
            now: sys::time_ns(),
        });
        // The kernel, as a test's client, takes no handles: it names objects by
        // their badges.
        let mint = answer.mint.filter(|_| from.caller_pid != 0);
        let minted = mint.map(|m| {
            let kind = if m.file { Kind::File } else { Kind::Dir };
            sys::endpoint_mint(endpoint, kind, m.rights, m.badge)
        });
        match minted {
            None => {
                let _ = sys::reply_h(&answer.reply, &[]);
            }
            Some(Ok(handle)) => {
                // A reply that fails left the handle here: close it.
                if sys::reply_h(&answer.reply, &[handle]).is_err() {
                    let _ = sys::handle_close(handle);
                }
            }
            Some(Err(error)) => {
                println!("fs: ENDPOINT_MINT failed: {error:?}");
                let failed = fsproto::Reply::Failed(fsproto::Error::Io);
                let _ = sys::reply_h(&fsproto::encode_reply(&failed), &[]);
            }
        }
        // A writer that never pauses must not starve retention and the GC.
        server.idle(sys::time_ns());
    }
}

/// The startup handle of `kind`.
fn startup(kind: Kind) -> Option<Handle> {
    let mut handles = [HandleInfo::default(); MAX_STARTUP];
    let count = sys::startup(&mut handles).ok()?;
    handles[..count.min(MAX_STARTUP)]
        .iter()
        .find(|h| h.kind() == Some(kind))
        .map(|h| Handle(h.handle))
}

/// The kernel's S04-2 transfer: its writes go into one action that `F`
/// commits, as a client's large write would.
#[derive(Default)]
struct Bench {
    file: Option<u64>,
    open: bool,
}

impl Bench {
    fn answer<D: kaifs::BlockDev>(
        &mut self,
        server: &mut Server<D>,
        part: Handle,
        request: &[u8],
    ) -> Vec<u8> {
        if request == stub::CHECK {
            return check::run(part).into_bytes();
        }
        match self.kai_fs(server, request) {
            Ok(reply) => reply,
            Err(why) => alloc::format!("err {why}").into_bytes(),
        }
    }

    fn kai_fs<D: kaifs::BlockDev>(
        &mut self,
        server: &mut Server<D>,
        request: &[u8],
    ) -> Result<Vec<u8>, &'static str> {
        use fsproto::{Reply, Request};
        let badge = match self.file {
            Some(badge) => badge,
            None => *self.file.insert(
                server
                    .system_file(BENCH_FILE)
                    .map_err(|_| "no s04-2 file")?,
            ),
        };
        let offset = || {
            request
                .get(1..9)
                .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
                .ok_or("no offset")
        };
        let mut call = |request: Request| {
            let bytes = fsproto::encode_request(&request);
            let answer = server.call(&Call {
                caller: 0,
                badge,
                rights: fsserve::FILE_RIGHTS,
                request: &bytes,
                now: sys::time_ns(),
            });
            match fsproto::decode_reply(&answer.reply) {
                Ok(Reply::Failed(_)) | Err(_) => Err("kaifs refused it"),
                Ok(reply) => Ok(reply),
            }
        };
        match request.first() {
            Some(&stub::WRITE) => {
                if !self.open {
                    call(Request::Begin { action: 0 })?;
                    self.open = true;
                }
                let data = request[9..].to_vec();
                call(Request::Write {
                    off: offset()?,
                    data,
                })?;
                Ok(b"ok".to_vec())
            }
            Some(&stub::READ) => {
                let len = request.get(9..13).ok_or("no length")?;
                let len = u32::from_le_bytes(len.try_into().unwrap());
                match call(Request::Read {
                    off: offset()?,
                    len,
                })? {
                    Reply::Data(data) => Ok(data),
                    _ => Err("no data"),
                }
            }
            Some(&stub::FLUSH) => {
                if core::mem::take(&mut self.open) {
                    call(Request::Commit)?;
                }
                Ok(b"ok".to_vec())
            }
            _ => Err("unknown request"),
        }
    }
}
