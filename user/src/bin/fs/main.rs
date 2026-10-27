//! `fs` (ADR 0004): the file system service, the only process that holds a
//! `Partition` handle, for `kai-data`. This is P2 task 3's stub: it serves the
//! `part` case's `check` and one file in 128 KiB records over the real PART_*
//! calls ([`kabi::part::stub`]), so S04-2's figures can be re-run on the disk.
//! Task 8 replaces it with the `kai-fs 1` server on kaifs.

#![no_std]
#![no_main]

extern crate alloc;

mod check;
mod file;

use kabi::MAX_MESSAGE;
use kabi::handle::{Handle, HandleInfo, Kind, MAX_STARTUP, RecvInfo};
use kabi::part::stub;
use user::{println, sys};

user::entry!(main);

fn main() -> i32 {
    let Some(part) = partition() else {
        println!("fs: no Partition handle; nothing to serve");
        return 2;
    };
    let info = match sys::part_info(part) {
        Ok(info) => info,
        Err(error) => {
            println!("fs: kai-data cannot be reached: {error:?}");
            return 2;
        }
    };
    println!(
        "fs: up; kai-data has {} sectors ({} MiB)",
        info.sectors,
        info.sectors / 2048
    );
    let mut file = file::File::new(part, info.sectors);
    let mut request = alloc::vec![0u8; MAX_MESSAGE];
    let mut reply = alloc::vec![0u8; MAX_MESSAGE];
    loop {
        let mut from = RecvInfo::default();
        let Ok(len) = sys::recv_h(&mut request, 0, &mut from) else {
            continue;
        };
        let len = answer(part, &mut file, &request[..len], &mut reply);
        let _ = sys::reply_h(&reply[..len], &[]);
    }
}

/// The one `Partition` handle among the startup handles.
fn partition() -> Option<Handle> {
    let mut handles = [HandleInfo::default(); MAX_STARTUP];
    let count = sys::startup(&mut handles).ok()?;
    handles[..count.min(MAX_STARTUP)]
        .iter()
        .find(|h| h.kind() == Some(Kind::Partition))
        .map(|h| Handle(h.handle))
}

/// Answers one request into `reply`; the reply's length.
fn answer(part: Handle, file: &mut file::File, request: &[u8], reply: &mut [u8]) -> usize {
    let text = |reply: &mut [u8], s: &str| {
        let n = s.len().min(reply.len());
        reply[..n].copy_from_slice(&s.as_bytes()[..n]);
        n
    };
    if request == stub::CHECK {
        let line = check::run(part);
        return text(reply, &line);
    }
    let offset = |r: &[u8]| {
        r.get(1..9)
            .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
    };
    let result = match request.first() {
        Some(&stub::WRITE) => match offset(request) {
            Some(at) => file.write(at, &request[9..]).map(|()| text(reply, "ok")),
            None => Err("a write needs an offset"),
        },
        Some(&stub::READ) => {
            let len = request
                .get(9..13)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize);
            match (offset(request), len) {
                (Some(at), Some(len)) if len <= reply.len() => {
                    file.read(at, &mut reply[..len]).map(|()| len)
                }
                _ => Err("a read needs an offset and a length that fits"),
            }
        }
        Some(&stub::FLUSH) => file.flush().map(|()| text(reply, "ok")),
        _ => Err("unknown request"),
    };
    match result {
        Ok(len) => len,
        Err(why) => {
            let mut line = alloc::string::String::from("err ");
            line.push_str(why);
            text(reply, &line)
        }
    }
}
