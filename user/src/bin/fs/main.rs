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
mod shm;

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use fsserve::{Call, Server, Started};
use kabi::handle::{Handle, HandleInfo, Kind, MAX_STARTUP, RecvInfo, Rights};
use kabi::part::stub;
use kabi::{Error, MAX_MESSAGE};
use user::{println, sys};

/// The `Partition` handle, for [`alive`], [`name_of`] and [`caps_of`].
static PARTITION: AtomicU32 = AtomicU32::new(0);

/// Whether a caller still runs: only a definite NotFound counts as dead.
fn alive(pid: u64) -> bool {
    let part = Handle(PARTITION.load(Ordering::Relaxed));
    sys::part_peer_alive(part, pid) != Err(Error::NotFound)
}

/// A caller's process name, from the kernel's table (PART_PEER_NAME): the one
/// folder of `/system/apps` it may open (`Server::set_name_of`). Names are
/// unique among live processes, and pids are never reused.
fn name_of(pid: u64, out: &mut [u8; kabi::MAX_NAME]) -> Option<usize> {
    let part = Handle(PARTITION.load(Ordering::Relaxed));
    sys::part_peer_name(part, pid, out).ok()
}

/// A caller's plan capability bits, from the kernel's table (PART_PEER_CAPS):
/// `fs` gives the holder of `cap::POLICY`, the shell alone, the person's
/// quarter of the space reserve (`Server::set_caps_of`). Any error, a pid gone
/// or a kernel without the call, counts as no caps.
fn caps_of(pid: u64) -> Option<u64> {
    let part = Handle(PARTITION.load(Ordering::Relaxed));
    sys::part_peer_caps(part, pid).ok()
}

user::entry!(main);

/// How long a receive waits before a slice of the GC.
const IDLE_MS: u64 = 100;
/// The block cache in front of the disk, in MiB, when the plan line names none:
/// the kernel names one from the admission budget (`cache_mib=`, #1036).
const CACHE_MIB: usize = 16;
/// The largest cache fs takes: ADR 0004's ([`kaifs::cache::DEFAULT_BLOCKS`]).
const MAX_CACHE_MIB: usize = 64;

/// The cache's size in MiB from fs's `cache_mib=` word, within 16 to 64.
fn cache_mib(args: &str) -> usize {
    args.split_whitespace()
        .find_map(|word| word.strip_prefix("cache_mib="))
        .and_then(|mib| mib.parse().ok())
        .unwrap_or(CACHE_MIB)
        .clamp(CACHE_MIB, MAX_CACHE_MIB)
}
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
    // kaifs, its block cache, then runs of up to 128 KiB a PART_* call.
    let runs = fsserve::batch::Batched::new(dev::PartDev::new(part, &info));
    let cache_mib = cache_mib(&user::args());
    let disk = kaifs::cache::Cached::new(runs, (cache_mib << 20) / kaifs::BLOCK);
    let mut server = match Server::start(disk, uuid) {
        Ok((server, started)) => {
            let how = match started {
                Started::Formatted => alloc::string::String::from("formatted it"),
                Started::Mounted { generation } => alloc::format!("generation {generation}"),
            };
            println!(
                "fs: up; kai-data has {} sectors ({} MiB), {how}; a {cache_mib} MiB block cache",
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
    server.set_name_of(name_of);
    server.set_caps_of(caps_of);
    // Retention counts real time when the firmware gave a date; without one,
    // kaifs's clock counts only the time `fs` has run.
    if let Ok(seconds) = sys::clock_now() {
        let wall = seconds.saturating_mul(1_000_000_000);
        server.set_wall_clock(wall.saturating_sub(sys::time_ns()));
    }
    fill_model(&mut server, part);
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
        let moved = &from.handles[..(from.handle_count as usize).min(from.handles.len())];
        let request = &request[..len];
        // A bulk request's one `Shm`, mapped for it; or a Rename's one `Dir`
        // of this server's, read by its badge (ENDPOINT_BADGE). Every handle
        // a request carried is dropped once it is answered.
        let mut mapped = match moved {
            [raw] if from.badge != 0 => shm::Mapped::of(request, Handle(*raw)),
            _ => None,
        };
        // A handle that cannot be a destination is the Rename's error, never
        // a rename within its own folder.
        let into = match moved {
            [] => None,
            [_] if mapped.is_some() => None,
            [raw] => Some(sys::endpoint_badge(Handle(*raw)).map_err(|e| match e {
                Error::Permission => fsproto::Error::Permission,
                _ => fsproto::Error::Invalid,
            })),
            _ => Some(Err(fsproto::Error::Invalid)),
        };
        let answer = (from.badge != 0).then(|| {
            let call = Call {
                caller: from.caller_pid,
                badge: from.badge,
                rights: Rights::from_bits_truncate(from.rights),
                request,
                now: sys::time_ns(),
            };
            match into {
                Some(into) => server.call_into(&call, into),
                None => server.call_shm(&call, mapped.as_mut().map(shm::Mapped::bytes)),
            }
        });
        drop(mapped);
        for &raw in moved {
            let _ = sys::handle_close(Handle(raw));
        }
        let Some(answer) = answer else {
            let reply = match from.caller_pid {
                0 => bench.answer(&mut server, part, request),
                _ => b"err plain routes to fs are the kernel's".to_vec(),
            };
            let _ = sys::reply_h(&reply, &[]);
            continue;
        };
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

/// Hands the kernel the model `kai-modelhint 1` names, if it reserved room for
/// one at boot (P2 task 10): its block runs in `kai-data`, 64 at a time. The
/// kernel reads, checks and publishes it; NotFound means it waits for none.
fn fill_model<D: kaifs::BlockDev>(server: &mut Server<D>, part: Handle) {
    let mut sector = [0u8; kabi::modelhint::SIZE];
    let hint = sys::part_read(part, 0, &mut sector)
        .ok()
        .and_then(|_| kabi::modelhint::Hint::decode(&sector));
    let Some(hint) = hint else {
        return;
    };
    let extents = match server.model_extents(&hint.sha256) {
        Ok(Some(extents)) => extents,
        found => {
            println!("fs: kai-modelhint names a model the store cannot give: {found:?}");
            // No runs: the kernel stops waiting, so llm's wait ends.
            let _ = sys::model_fill(part, &[], 0);
            return;
        }
    };
    let blocks: u64 = extents.iter().map(|&(_, n)| n).sum();
    if blocks * 4096 < hint.len {
        println!("fs: the model store's file is shorter than kai-modelhint says");
        let _ = sys::model_fill(part, &[], 0);
        return;
    }
    let mut offset = 0;
    for chunk in extents.chunks(64) {
        match sys::model_fill(part, chunk, offset) {
            Ok(filled) => offset = filled,
            // The boot used a kai-model disk or a boot module instead.
            Err(Error::NotFound) => return,
            Err(error) => {
                println!("fs: MODEL_FILL refused the model: {error:?}");
                return;
            }
        }
    }
    println!(
        "fs: the model store's model is filled, {offset} of {} bytes",
        hint.len
    );
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
