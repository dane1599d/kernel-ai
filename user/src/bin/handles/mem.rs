//! `mode=mem`: serves the kernel's anonymous-memory case (MEM_RESERVE,
//! MEM_COMMIT, MEM_RELEASE) one line at a time until `exit`. `reserve <pages>`
//! takes address space; `commit <pages>` commits that many pages from the
//! reservation's start and writes to each; `release` gives them back and
//! answers how many; `misuse` checks the refusals. Answers are `ok`, `ok <n>` or
//! `err <what>`.

use alloc::format;
use alloc::string::String;
use kabi::Error;
use user::sys;

const PAGE: u64 = 4096;

pub fn mem() -> Result<(), &'static str> {
    let mut buf = [0u8; 64];
    let mut reserved: Option<u64> = None;
    loop {
        let mut caller = 0;
        let len = sys::svc_recv(&mut buf, 60_000, &mut caller).map_err(|_| "mem: no request")?;
        let text = core::str::from_utf8(&buf[..len.min(buf.len())]).unwrap_or("");
        let mut words = text.split_whitespace();
        let word = words.next();
        let number = words.next().and_then(|w| w.parse::<u64>().ok());
        let reply: String = match (word, number, reserved) {
            (Some("exit"), ..) => {
                let _ = sys::svc_reply(b"ok");
                return Ok(());
            }
            (Some("reserve"), Some(pages), _) => match sys::mem_reserve(pages * PAGE) {
                Ok(addr) => {
                    reserved = Some(addr);
                    format!("ok {addr}")
                }
                Err(error) => format!("err reserve {error:?}"),
            },
            (Some("commit"), Some(pages), Some(addr)) => {
                match sys::mem_commit(addr, pages * PAGE) {
                    Ok(_) => {
                        // SAFETY: MEM_COMMIT mapped `pages` read-write pages at `addr`.
                        let bytes = unsafe {
                            core::slice::from_raw_parts_mut(
                                addr as *mut u8,
                                (pages * PAGE) as usize,
                            )
                        };
                        for page in bytes.chunks_mut(PAGE as usize) {
                            page[0] = 0xA5;
                        }
                        String::from("ok")
                    }
                    Err(error) => format!("err commit {error:?}"),
                }
            }
            (Some("release"), _, Some(addr)) => match sys::mem_release(addr) {
                Ok(freed) => {
                    reserved = None;
                    format!("ok {freed}")
                }
                Err(error) => format!("err release {error:?}"),
            },
            (Some("misuse"), ..) => match misuse() {
                Ok(()) => String::from("ok"),
                Err(what) => format!("err {what}"),
            },
            _ => String::from("err unknown line"),
        };
        let _ = sys::svc_reply(reply.as_bytes());
    }
}

/// The refusals: zero bytes, a hole, a second release, a commit after release.
fn misuse() -> Result<(), &'static str> {
    if sys::mem_reserve(0) != Err(Error::Invalid) {
        return Err("reserve(0) was not refused");
    }
    let addr = sys::mem_reserve(4 * PAGE).map_err(|_| "a small reserve failed")?;
    if sys::mem_commit(addr + 2 * PAGE, PAGE) != Err(Error::Invalid) {
        return Err("a commit leaving a hole was not refused");
    }
    if sys::mem_commit(addr, 5 * PAGE) != Err(Error::Invalid) {
        return Err("a commit past the reservation was not refused");
    }
    sys::mem_commit(addr, PAGE).map_err(|_| "a small commit failed")?;
    if sys::mem_release(addr) != Ok(1) {
        return Err("the release did not free its one page");
    }
    if sys::mem_release(addr) != Err(Error::NotFound) {
        return Err("a second release was not refused");
    }
    if sys::mem_commit(addr, PAGE) != Err(Error::NotFound) {
        return Err("a commit after the release was not refused");
    }
    Ok(())
}
