//! Test client for the `fs` service (P2 task 8, `test=files`, `test=files-restart`).
//! Started with the FILES capability alone, it works through the `Dir` handle
//! for `/home/user` it gets at startup, in `kai-fs 1`, and leaves a marker file
//! the kernel's half of the case waits for:
//!
//! - `step=write` (boot 1 of `fs-reboot`): makes `notes/plan.txt` and a 20 KiB
//!   file in one action, checks that create never overwrites and that `..`
//!   never leaves the `Dir`, then `write.ok`;
//! - `step=read` (boot 2, same disk): reads both back byte for byte, `read.ok`;
//! - `step=restart`: writes `restart.txt` and keeps its `File` handle, `ready`;
//!   the kernel then kills `fs` mid-action, and once `restarted` appears this
//!   reads `restart.txt` through the handle it kept, `restart.ok`.
//!
//! - `step=actions` and `step=churn boot=N`: the power cut's workload ([`cut`]).
//!
//! It prints a line for each step that passed, or `filecheck: FAIL ...`, then
//! idles: the supervisor would restart an exited service.

#![no_std]
#![no_main]

extern crate alloc;

mod cut;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use fsproto::{Error, How, Reply, Request};
use kabi::handle::{Handle, HandleInfo, Kind, MAX_STARTUP, MAX_TRANSFER, Rights};
use user::{println, sys};

user::entry!(main);

const CALL_MS: u64 = 10_000;
const BIG: usize = 20 * 1024;
const PLAN: &[u8] = b"buy milk";

fn main() -> i32 {
    let mut args = [0u8; 128];
    let len = sys::args(&mut args).unwrap_or(0);
    let args = core::str::from_utf8(&args[..len]).unwrap_or("");
    let step = args
        .split_whitespace()
        .find_map(|w| w.strip_prefix("step="));
    let boot = args
        .split_whitespace()
        .find_map(|w| w.strip_prefix("boot="))
        .and_then(|n| n.parse().ok())
        .unwrap_or(0);
    let Some(home) = startup_dir() else {
        println!("filecheck: FAIL no Dir handle at startup");
        idle();
    };
    let result = match step {
        Some("write") => write(home),
        Some("read") => read(home),
        Some("restart") => restart(home),
        Some("actions") => cut::actions(home),
        Some("churn") => cut::churn(home, boot),
        _ => Err(String::from(
            "no step=write, read, restart, actions or churn",
        )),
    };
    if let Err(why) = result {
        println!("filecheck: FAIL {why}");
    }
    idle()
}

fn idle() -> ! {
    loop {
        sys::sleep_ms(60_000);
    }
}

fn startup_dir() -> Option<Handle> {
    let mut handles = [HandleInfo::default(); MAX_STARTUP];
    let count = sys::startup(&mut handles).ok()?;
    handles[..count.min(MAX_STARTUP)]
        .iter()
        .find(|h| h.kind() == Some(Kind::Dir))
        .map(|h| Handle(h.handle))
}

/// One request on `on`; the reply and the handle it moved, if any.
fn ask(on: Handle, request: Request) -> Result<(Reply, Option<Handle>), String> {
    let bytes = fsproto::encode_request(&request);
    let mut reply = alloc::vec![0u8; fsproto::MAX_MESSAGE];
    let mut moved = [Handle::NONE; MAX_TRANSFER];
    let (len, came) = sys::call_h(on, &bytes, &mut reply, CALL_MS, &[], &mut moved)
        .map_err(|e| format!("CALL_H: {e:?}"))?;
    let reply = fsproto::decode_reply(&reply[..len]).map_err(|e| format!("a reply: {e:?}"))?;
    Ok((reply, (came > 0).then_some(moved[0])))
}

/// `ask`, wanting `want`.
fn expect(on: Handle, request: Request, want: Reply) -> Result<(), String> {
    let what = format!("{request:?}");
    match ask(on, request)? {
        (got, _) if got == want => Ok(()),
        (got, _) => Err(format!("{what} gave {got:?}")),
    }
}

fn open(dir: Handle, path: &str, how: How, rights: Rights) -> Result<Handle, String> {
    let request = Request::Open {
        path: path.into(),
        rights: rights.bits(),
        how,
    };
    match ask(dir, request)? {
        (Reply::Opened(..), Some(handle)) => Ok(handle),
        other => Err(format!("opening {path}: {other:?}")),
    }
}

fn refused(dir: Handle, path: &str, how: How, want: Error) -> Result<(), String> {
    let request = Request::Open {
        path: path.into(),
        rights: 0,
        how,
    };
    expect(dir, request, Reply::Failed(want)).map_err(|e| format!("not refused: {e}"))
}

fn dir_rights() -> Rights {
    Rights::READ | Rights::WRITE | Rights::ENUMERATE | Rights::CREATE | Rights::DELETE
}

fn file_rights() -> Rights {
    Rights::READ | Rights::WRITE
}

fn read_all(file: Handle, len: usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    while out.len() < len {
        let request = Request::Read {
            off: out.len() as u64,
            len: (len - out.len()).min(fsproto::MAX_DATA) as u32,
        };
        match ask(file, request)? {
            (Reply::Data(data), _) if !data.is_empty() => out.extend_from_slice(&data),
            other => return Err(format!("a read at {}: {other:?}", out.len())),
        }
    }
    Ok(out)
}

fn pattern() -> Vec<u8> {
    (0..BIG).map(|i| (i * 31 % 251) as u8).collect()
}

fn mark(home: Handle, name: &str) -> Result<(), String> {
    open(home, name, How::CreateFile, Rights::READ).map(|h| {
        let _ = sys::handle_close(h);
    })
}

fn write(home: Handle) -> Result<(), String> {
    let notes = open(home, "notes", How::Mkdir, dir_rights())?;
    let plan = open(notes, "plan.txt", How::CreateFile, file_rights())?;
    expect(
        plan,
        Request::Write {
            off: 0,
            data: PLAN.to_vec(),
        },
        Reply::Done,
    )?;
    refused(home, "notes/plan.txt", How::CreateFile, Error::Exists)?;
    refused(home, "..", How::Existing, Error::Invalid)?;
    handles(home, plan)?;
    refused(notes, "../../home", How::Existing, Error::Invalid)?;
    // A file over several messages, as one action: one commit.
    let big = open(notes, "big.bin", How::CreateFile, file_rights())?;
    expect(home, Request::Begin { action: 1 }, Reply::Done)?;
    for (at, piece) in pattern().chunks(fsproto::MAX_DATA).enumerate() {
        let off = (at * fsproto::MAX_DATA) as u64;
        expect(
            big,
            Request::Write {
                off,
                data: piece.to_vec(),
            },
            Reply::Done,
        )?;
    }
    match ask(home, Request::Commit)? {
        (Reply::Committed { .. }, _) => {}
        other => return Err(format!("the commit: {other:?}")),
    }
    if read_all(plan, PLAN.len())? != PLAN || read_all(big, BIG)? != pattern() {
        return Err(String::from("a file read back different before the reboot"));
    }
    mark(home, "write.ok")?;
    println!("filecheck: boot 1 wrote notes/plan.txt and a 20 KiB file through its Dir handle");
    Ok(())
}

/// What the kernel checks of `Dir` and `File` handles, seen from ring 3: a call
/// on one reaches `fs` with its own badge and exactly its rights, and a process
/// with no endpoint of its own cannot mint one.
fn handles(home: Handle, plan: Handle) -> Result<(), String> {
    let reader = open(home, "notes/plan.txt", How::Existing, Rights::READ)?;
    let (want, got) = match (ask(plan, Request::Stat)?, ask(reader, Request::Stat)?) {
        ((Reply::Stat(a), _), (Reply::Stat(b), _)) => (a.object, b.object),
        other => return Err(format!("STAT on two handles: {other:?}")),
    };
    if want != got {
        return Err(String::from(
            "two handles to one file reached different objects",
        ));
    }
    let write = Request::Write {
        off: 0,
        data: b"no".to_vec(),
    };
    expect(reader, write, Reply::Failed(Error::Permission))
        .map_err(|e| format!("a READ-only File wrote: {e}"))?;
    let _ = sys::handle_close(reader);
    // A Dir duplicated with fewer rights: fs is told the fewer rights.
    let lister = sys::handle_dup(home, Rights::READ | Rights::ENUMERATE)
        .map_err(|e| format!("HANDLE_DUP: {e:?}"))?;
    match ask(
        lister,
        Request::List {
            after: None,
            max: 4,
        },
    )? {
        (Reply::Listed { .. }, _) => {}
        other => return Err(format!("a READ|ENUMERATE Dir cannot list: {other:?}")),
    }
    refused(lister, "new.txt", How::CreateFile, Error::Permission)?;
    expect(
        lister,
        Request::Trash {
            name: "notes".into(),
        },
        Reply::Failed(Error::Permission),
    )?;
    let _ = sys::handle_close(lister);
    match sys::endpoint_mint(home, Kind::Dir, Rights::READ, 1) {
        Err(kabi::Error::BadHandle) => {}
        other => return Err(format!("ENDPOINT_MINT on a Dir gave {other:?}")),
    }
    Ok(())
}

fn read(home: Handle) -> Result<(), String> {
    let plan = open(home, "notes/plan.txt", How::Existing, Rights::READ)?;
    let big = open(home, "notes/./big.bin", How::Existing, Rights::READ)?;
    if read_all(plan, PLAN.len())? != PLAN || read_all(big, BIG)? != pattern() {
        return Err(String::from("boot 1's files read back different"));
    }
    mark(home, "read.ok")?;
    println!("filecheck: boot 2 read back what boot 1 wrote, byte for byte");
    Ok(())
}

fn restart(home: Handle) -> Result<(), String> {
    let file = open(home, "restart.txt", How::CreateFile, file_rights())?;
    expect(
        file,
        Request::Write {
            off: 0,
            data: b"v1".to_vec(),
        },
        Reply::Done,
    )?;
    mark(home, "ready")?;
    // fs is killed and restarted meanwhile; a call that finds it down fails
    // and is tried again.
    let restarted = Request::Open {
        path: "restarted".into(),
        rights: Rights::READ.bits(),
        how: How::Existing,
    };
    loop {
        if let Ok((Reply::Opened(..), moved)) = ask(home, restarted.clone()) {
            if let Some(h) = moved {
                let _ = sys::handle_close(h);
            }
            break;
        }
        sys::sleep_ms(100);
    }
    if read_all(file, 2)? != b"v1" {
        return Err(String::from("restart.txt is not what was committed"));
    }
    mark(home, "restart.ok")?;
    println!("filecheck: the File handle opened before fs restarted reads what was committed");
    Ok(())
}
