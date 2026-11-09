//! An update over the network (P5 task 10; ADR 0008 §9 and §12): `updated`
//! streams the next release into the idle slot from the update repository, on
//! the `NetDest` its plan line gives it, and `updatenet` does the checking.
//!
//! - The metadata is the repository's, refreshed by `kaituf` from the root this
//!   slot was built with (or the newer one `/system/trust` keeps), on the Pages
//!   host; the release it names must be newer than every release this machine
//!   has committed, fit the idle slot, and not be staged already.
//! - The image comes in 2 MiB chunks by `Range` from the release host (through
//!   GitHub's redirect to its release-assets host), each checked against the
//!   signed chunk list before it is written. The slot is marked Updating first,
//!   and a record of the last chunk checked (a file in `/system/updates`, when
//!   `kai-data` is there) lets a later run resume where this one stopped.
//! - Before the slot becomes a trial the whole image is read back and hashed, the
//!   metadata is asked again (a long download can span a newer release), and the
//!   update snapshot of the system volume is taken, as `step=update` does for
//!   the update disk; then the restart is set, at once or in the restart window.
//!
//! `step=online` takes `online=test:PORT` from the plan line, which names xtask's
//! HTTPS mock (the same hosts as `pkg`'s `Hosts::mock`) and its test CA, a file in
//! the image's repository folder (`bootfiles=pkgrepo`). In slot R (`:reinstall`)
//! `into=A` or `into=B` names the slot to write. The shipping system's repository
//! waits for the follow-up that gives `updated` a request for it.

use crate::disk::{Trusted, trusted};
use crate::folders::{self, Folders};
use crate::window::Staged;
use crate::{pause, record, restore, set, slot_of};
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use bootslot::{Nv, Record, Slot};
use core::sync::atomic::{AtomicU32, Ordering};
use kabi::handle::{Handle, HandleInfo, Kind, MAX_STARTUP};
use kabi::update::{MAX_IO, SECTOR};
use kainet::chunks::MAX_LIST;
use kainet::http::{TlsConnect, Url};
use kaipkg::online::{Hosts, Online};
use updatenet::net::Over;
use updatenet::plan::{self, Job, Machine};
use updatenet::slot::{Checkpoint, Mark, SlotIo, SlotPartial};
use updatenet::stage::{self, Boot, Stop};
use user::net::{NetDial, roots_of, tls_config};
use user::{println, sys};

/// How long one request may take, redirects included: a full handshake on an
/// emulated CPU takes seconds.
const REQUEST_NS: u64 = 120_000_000_000;
/// The boot cases' test CA, a file of the image's repository folder.
const TEST_CA: &str = "pkgrepo/tls-test-ca.der";
/// The longest certificate read.
const CA_MAX: usize = 4096;
/// The record of the last chunk checked, in `/system/updates`.
const MARK_FILE: &str = "online.mark";
/// A mark is the image's SHA-256 and the number of chunks done.
const MARK_BYTES: usize = 32 + 8;

/// The `Update` handle, for the wall clock: `fn` pointers hold no state.
static UPDATE: AtomicU32 = AtomicU32::new(0);

fn now_s() -> Option<u64> {
    sys::trust_time(Handle(UPDATE.load(Ordering::Relaxed))).ok()
}

/// The port in `online=test:PORT`.
fn test_port(args: &str) -> Option<u16> {
    args.split_whitespace()
        .find_map(|w| w.strip_prefix("online=test:"))
        .and_then(|port| port.parse().ok())
        .filter(|port| *port > 0)
}

/// In slot R, the slot `into=` names.
fn into(args: &str) -> Option<Slot> {
    match args
        .split_whitespace()
        .find_map(|w| w.strip_prefix("into="))?
    {
        "A" => Some(Slot::A),
        "B" => Some(Slot::B),
        _ => None,
    }
}

/// The `NetDest` handle among the startup handles.
fn netdest() -> Option<Handle> {
    let mut list = [HandleInfo::default(); MAX_STARTUP];
    let count = sys::startup(&mut list).ok()?.min(MAX_STARTUP);
    list[..count]
        .iter()
        .find(|info| info.kind() == Some(Kind::NetDest))
        .map(|info| Handle(info.handle))
}

/// The test CA, read from the image's repository folder.
fn test_ca() -> Result<Vec<u8>, String> {
    let mut buf = vec![0u8; CA_MAX];
    let len =
        sys::boot_read(TEST_CA, &mut buf).map_err(|e| format!("BOOT_READ {TEST_CA}: {e:?}"))?;
    if len > CA_MAX {
        return Err(format!("{TEST_CA} is {len} bytes, over {CA_MAX}"));
    }
    buf.truncate(len);
    Ok(buf)
}

/// The slot `slot`, as `updatenet` writes and reads it: by byte offset, on
/// sectors, in the kernel's largest I/O.
struct Slots {
    handle: Handle,
    slot: u64,
    buf: Vec<u8>,
}

impl SlotIo for Slots {
    fn write(&mut self, offset: u64, bytes: &[u8]) -> Result<(), &'static str> {
        if !offset.is_multiple_of(SECTOR) {
            return Err("a slot write starts on a sector");
        }
        let mut done = 0;
        while done < bytes.len() {
            let n = (bytes.len() - done).min(MAX_IO as usize);
            let padded = n.next_multiple_of(SECTOR as usize);
            // A last partial sector is written whole, zero past the image's end.
            self.buf[..n].copy_from_slice(&bytes[done..done + n]);
            self.buf[n..padded].fill(0);
            let sector = (offset + done as u64) / SECTOR;
            sys::slot_write(self.handle, self.slot, sector, &self.buf[..padded])
                .map_err(|_| "SLOT_WRITE failed")?;
            done += n;
            pause(sector + (n as u64).div_ceil(SECTOR));
        }
        Ok(())
    }

    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        if !offset.is_multiple_of(SECTOR) {
            return Err("a slot read starts on a sector");
        }
        let mut done = 0;
        while done < buf.len() {
            let n = (buf.len() - done).min(MAX_IO as usize);
            let padded = n.next_multiple_of(SECTOR as usize);
            let sector = (offset + done as u64) / SECTOR;
            sys::slot_read(self.handle, self.slot, sector, &mut self.buf[..padded])
                .map_err(|_| "SLOT_READ failed")?;
            buf[done..done + n].copy_from_slice(&self.buf[..n]);
            done += n;
            pause(sector + (n as u64).div_ceil(SECTOR));
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), &'static str> {
        sys::slot_flush(self.handle, self.slot)
            .map(|_| ())
            .map_err(|_| "SLOT_FLUSH failed")
    }
}

/// Where the last chunk checked is kept: a file in `/system/updates`, so a later
/// run (after a cut, or a reboot) resumes there; in memory with no `kai-data`.
struct Marks {
    dir: Option<Handle>,
    memory: Option<Mark>,
}

impl Checkpoint for Marks {
    fn load(&mut self) -> Result<Option<Mark>, &'static str> {
        let Some(dir) = self.dir else {
            return Ok(self.memory);
        };
        match folders::read(dir, MARK_FILE) {
            // A file of the wrong size is no mark: the download starts over.
            Ok(Some(bytes)) if bytes.len() == MARK_BYTES => {
                let sha256 = <[u8; 32]>::try_from(&bytes[..32]).map_err(|_| "a bad mark")?;
                let done = u64::from_le_bytes(
                    <[u8; 8]>::try_from(&bytes[32..]).map_err(|_| "a bad mark")?,
                );
                Ok(Some(Mark {
                    sha256,
                    done: usize::try_from(done).map_err(|_| "a bad mark")?,
                }))
            }
            Ok(_) => Ok(None),
            Err(_) => Err("the download's mark cannot be read"),
        }
    }

    fn save(&mut self, mark: Option<Mark>) -> Result<(), &'static str> {
        self.memory = mark;
        let Some(dir) = self.dir else {
            return Ok(());
        };
        let bytes = mark.map_or_else(Vec::new, |mark| {
            let mut bytes = Vec::with_capacity(MARK_BYTES);
            bytes.extend_from_slice(&mark.sha256);
            bytes.extend_from_slice(&(mark.done as u64).to_le_bytes());
            bytes
        });
        folders::write(dir, MARK_FILE, &bytes).map_err(|_| "the download's mark cannot be saved")
    }
}

/// The boot record, and the snapshot a failed first boot goes back to.
struct Lever<'a> {
    handle: Handle,
    dirs: &'a Folders,
}

impl Boot for Lever<'_> {
    fn set(&mut self, nv: &Nv) -> Result<(), &'static str> {
        set(self.handle, nv, false).map_err(|why| {
            println!("updated: {why}");
            "the boot record cannot be written"
        })
    }

    /// Before the new slot's first boot: if it fails before it commits, the old
    /// slot boots on this snapshot of `system` (ADR 0008 §10). `home` is never in it.
    fn before_trial(&mut self, job: &Job) -> Result<(), &'static str> {
        let Some(updates) = self.dirs.updates else {
            return Ok(());
        };
        let id = folders::update_snapshot(updates).map_err(|why| {
            println!("updated: {why}");
            "the update snapshot cannot be taken"
        })?;
        println!("updated: took update snapshot {id} of the system volume");
        let pending = bootslot::pending::Pending {
            snapshot: id,
            slot: job.slot,
            release_seq: job.release.seq,
        };
        restore::record_pending(updates, pending).map_err(|why| {
            println!("updated: {why}");
            "the pending update cannot be recorded"
        })
    }
}

/// What a stop says, as the step's failure: a refusal of the repository or the
/// image is `refused:`, as the update disk's are.
fn stopped(stop: &Stop) -> String {
    match stop {
        Stop::Bad(_) => format!("refused: {}", stop.why()),
        _ => stop.why(),
    }
}

/// The machine as `plan` asks about it: the record, which slot runs, and the room
/// the idle slot has.
fn machine(record: &Record, running: Slot, into: Option<Slot>, room: u64) -> Machine<'_> {
    Machine {
        record,
        running,
        into,
        room,
    }
}

/// `step=online`: stages the repository's release into the idle slot (in R, the
/// slot `into=` names), and restarts into it at once when `restart_now`;
/// otherwise its one-shot is set and the restart waits (`window.rs`).
pub fn update(handle: Handle, restart_now: bool, args: &str) -> Result<Staged, String> {
    let port = test_port(args).ok_or("step=online takes online=test:PORT")?;
    UPDATE.store(handle.0, Ordering::Relaxed);
    let Trusted {
        dirs,
        mut cache,
        now,
    } = trusted(handle)?;
    let dest = netdest().ok_or("updated has no NetDest handle")?;
    let info = sys::slot_info(handle, 1).map_err(|e| format!("SLOT_INFO: {e:?}"))?;
    let active = slot_of(info.active)?;
    let mut nv = Nv {
        record: record(handle)?,
        one_shot: Some(active),
    };
    let into = into(args);
    let slot = machine(&nv.record, active, into, 0)
        .slot()
        .map_err(|why| format!("refused: {}", why.why()))?;
    let room = sys::slot_info(handle, slot as u64)
        .map_err(|e| format!("SLOT_INFO: {e:?}"))?
        .sectors
        * SECTOR;

    let ca = test_ca()?;
    let roots = roots_of(&ca).map_err(|e| format!("the test CA is refused: {e:?}"))?;
    let config = tls_config(roots, now_s).map_err(|e| format!("no TLS configuration: {e:?}"))?;
    let clock = || sys::time_ns();
    let hosts = Hosts::mock();
    let online = |hosts: &Hosts| {
        let dial = NetDial {
            dest,
            https_port: port,
        };
        let connect = TlsConnect {
            config: config.clone(),
            dial,
        };
        Online::new(hosts.clone(), connect, &clock, REQUEST_NS)
    };

    let mut repository = online(&hosts);
    let (image, list) = plan::refresh(&mut repository, &mut cache, now)
        .map_err(|why| format!("refused: {}", why.why()))?;
    let release = plan::release_of(&image).map_err(|why| format!("refused: {}", why.why()))?;
    println!(
        "updated: the online repository verifies: slot.img is release {}, {} bytes",
        release.seq, release.length
    );
    let job = plan::plan(
        &image,
        &list,
        &machine(&nv.record, active, into, room),
        &hosts,
    )
    .map_err(|why| format!("refused: {}", why.why()))?;
    let mut over = Over::of(&mut repository);
    let (status, bytes) = over
        .small(&job.list_url, MAX_LIST as u64)
        .map_err(|e| format!("the chunk list: {e:?}"))?;
    if status != 200 {
        return Err(format!("the chunk list answered {status}"));
    }
    let chunks = job
        .chunks(&bytes)
        .map_err(|why| format!("refused: {}", why.why()))?;
    let url = Url::parse(&job.image_url).map_err(|e| format!("the image's address: {e:?}"))?;

    let mut slots = Slots {
        handle,
        slot: job.slot as u64,
        buf: vec![0u8; MAX_IO as usize],
    };
    let mut marks = Marks {
        dir: dirs.updates,
        memory: None,
    };
    let mut partial = SlotPartial::new(&mut slots, &mut marks, &job.release);
    let mut lever = Lever {
        handle,
        dirs: &dirs,
    };
    // The metadata again, from a connection of its own, once the image has
    // checked out: a long download can span a newer release.
    let mut recheck = || plan::refresh(&mut online(&hosts), &mut cache, now);
    let staged = stage::stage(
        &job,
        &chunks,
        &mut nv,
        &mut lever,
        &mut partial,
        &mut recheck,
        &mut |range, sink| over.range(&url, range, sink),
    )
    .map_err(|stop| stopped(&stop))?;
    println!(
        "updated: wrote release {} into {:?} over TLS and checked it: {} chunks kept, {} fetched",
        staged.release_seq, staged.slot, staged.report.kept, staged.report.fetched
    );
    if dirs.trust.is_some() {
        println!("updated: the verified metadata is in /system/trust");
    }
    let (other, release_seq) = (staged.slot, staged.release_seq);
    bootslot::restart_into(&mut nv, other);
    if restart_now {
        println!("updated: restarting into {other:?}");
    }
    let restarted = set(handle, &nv, restart_now);
    if restarted.is_err() {
        // The slot stays a trial it is not restarted into; with the pending
        // record cleared, nothing is restored later (as `step=update` does).
        if let Some(updates) = dirs.updates {
            let _ = folders::write(updates, restore::PENDING, b"");
        }
    }
    restarted.map(|()| Staged {
        slot: other,
        release_seq,
    })
}
