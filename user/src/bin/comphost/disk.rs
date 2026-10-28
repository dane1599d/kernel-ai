//! The app's saved state on disk (P4, app storage that survives a reboot):
//! with a plan line's `dir=apps`, comphost has a startup `Dir` for
//! `/system/apps`, keeps its app's committed checkpoints in `<app>/state.0`
//! and `state.1` in turn ([`livecore::saved`]), and starts from the newer one
//! when the kernel has none. Without `dir=apps`, or without `fs`, the app
//! simply keeps what the kernel keeps, as before.

use alloc::string::String;
use alloc::vec::Vec;
use fsproto::{How, Reply, Request};
use kabi::handle::{Handle, HandleInfo, Kind, MAX_STARTUP, MAX_TRANSFER, Rights};
use livecore::saved::{Record, SLOTS, newest};
use user::{println, sys};

/// How long a call to `fs` may take.
const CALL_MS: u64 = 10_000;
/// Rights on the app's folder and its slot files.
const RIGHTS: Rights = Rights::READ.union(Rights::WRITE).union(Rights::CREATE);

pub struct Disk {
    /// `/system/apps/<app>`.
    folder: Handle,
    /// The generation the next save writes.
    next: u64,
}

fn call(on: Handle, request: &Request) -> Result<(Reply, Option<Handle>), String> {
    let bytes = fsproto::encode_request(request);
    let mut reply = alloc::vec![0u8; fsproto::MAX_MESSAGE];
    let mut moved = [Handle::NONE; MAX_TRANSFER];
    let (len, came) = sys::call_h(on, &bytes, &mut reply, CALL_MS, &[], &mut moved)
        .map_err(|e| alloc::format!("fs: {e:?}"))?;
    let reply = fsproto::decode_reply(&reply[..len.min(reply.len())])
        .map_err(|e| alloc::format!("fs reply: {e:?}"))?;
    match reply {
        Reply::Failed(error) => Err(alloc::format!("fs: {error:?}")),
        reply => Ok((reply, (came > 0).then_some(moved[0]))),
    }
}

fn open(dir: Handle, name: &str, how: How) -> Result<Handle, String> {
    let request = Request::Open {
        path: String::from(name),
        rights: RIGHTS.bits(),
        how,
    };
    call(dir, &request)?
        .1
        .ok_or_else(|| alloc::format!("fs gave no handle for {name}"))
}

/// The startup `Dir` a plan line's `dir=` gave, if any.
fn startup_dir() -> Option<Handle> {
    let mut handles = [HandleInfo::default(); MAX_STARTUP];
    let count = sys::startup(&mut handles).ok()?;
    handles[..count.min(MAX_STARTUP)]
        .iter()
        .find(|h| h.kind() == Some(Kind::Dir))
        .map(|h| Handle(h.handle))
}

/// A whole file's bytes, at most `max`.
fn read(file: Handle, max: usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let request = Request::Read {
            off: out.len() as u64,
            len: fsproto::MAX_DATA as u32,
        };
        let Reply::Data(bytes) = call(file, &request)?.0 else {
            return Err(String::from("fs: not data"));
        };
        let done = bytes.len() < fsproto::MAX_DATA;
        out.extend_from_slice(&bytes);
        if done || out.len() > max {
            return Ok(out);
        }
    }
}

impl Disk {
    /// The app's folder beneath the startup `Dir`, made if new.
    pub fn open(app: &str) -> Option<Self> {
        let apps = startup_dir()?;
        let folder = open(apps, app, How::Existing)
            .or_else(|_| open(apps, app, How::Mkdir))
            .map_err(|why| println!("comphost: no folder for {app}'s saved state: {why}"))
            .ok()?;
        Some(Self { folder, next: 0 })
    }

    /// The newest saved record, at most `max` bytes of checkpoint; the next
    /// save follows it.
    pub fn load(&mut self, max: usize) -> Option<Record> {
        let mut slots: [Option<Vec<u8>>; 2] = [None, None];
        for (n, name) in SLOTS.iter().enumerate() {
            if let Ok(file) = open(self.folder, name, How::Existing) {
                slots[n] = read(file, max + 64).ok();
                let _ = sys::handle_close(file);
            }
        }
        let record = newest([slots[0].as_deref(), slots[1].as_deref()], max)?;
        self.next = record.generation + 1;
        Some(record)
    }

    /// Writes a committed checkpoint to the slot after the last one.
    pub fn save(&mut self, schema: u32, checkpoint: &[u8]) {
        let record = Record {
            generation: self.next,
            schema,
            checkpoint: checkpoint.to_vec(),
        };
        match self.write(&record) {
            Ok(()) => self.next += 1,
            Err(why) => println!("comphost: could not save the state: {why}"),
        }
    }

    fn write(&mut self, record: &Record) -> Result<(), String> {
        let name = record.slot();
        let file = open(self.folder, name, How::Existing)
            .or_else(|_| open(self.folder, name, How::CreateFile))?;
        let bytes = record.encode();
        let written = bytes
            .chunks(fsproto::MAX_DATA)
            .enumerate()
            .try_for_each(|(n, chunk)| {
                let request = Request::Write {
                    off: (n * fsproto::MAX_DATA) as u64,
                    data: chunk.to_vec(),
                };
                call(file, &request).map(|_| ())
            })
            .and_then(|()| {
                let request = Request::Truncate {
                    size: bytes.len() as u64,
                };
                call(file, &request).map(|_| ())
            });
        let _ = sys::handle_close(file);
        written
    }
}
