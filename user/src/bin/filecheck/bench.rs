//! `step=bench` (lane S-io): kaifs end to end from a ring-3 client, the way an
//! app reaches it, where the `part` case's client is the kernel. A 32 MiB file
//! goes to `fs` in one action and comes back, first in 7 KiB messages and then,
//! with a bulk `Shm` reserved, 1 MiB a round trip (`user::kaifs::bulk`). Each
//! way logs its MB/s and how much of its time went waiting on `fs` (the round
//! trips), apart from the client's own work (making and checking the bytes).
//! `fs` logs its own split of the same phases (`fs: phase ...`).

use super::{ask, expect, file_rights, mark, open};
use alloc::format;
use alloc::string::String;
use fsproto::{How, Reply, Request};
use kabi::handle::Handle;
use user::{kaifs, println, sys};

/// The file each run moves: S04-2's 32 MiB.
const FILE_BYTES: usize = 32 << 20;
/// What the client hands `user::kaifs` at a time.
const PIECE: usize = 1 << 20;

pub fn run(home: Handle) -> Result<(), String> {
    measure(home, "7 KiB messages", "bench-small.bin")?;
    kaifs::bulk::reserve(fsproto::MAX_SHM_DATA).map_err(|e| format!("no bulk Shm: {e:?}"))?;
    measure(home, "1 MiB Shm", "bench-bulk.bin")?;
    mark(home, "bench.ok")
}

/// The byte at `offset`, a pattern `fs` cannot guess.
fn byte(offset: usize) -> u8 {
    (offset.wrapping_mul(2_654_435_761) >> 13) as u8
}

fn fill(offset: usize, out: &mut [u8]) {
    for (i, b) in out.iter_mut().enumerate() {
        *b = byte(offset + i);
    }
}

/// MB/s for `bytes` in `ns`.
fn rate(bytes: usize, ns: u64) -> u64 {
    bytes as u64 * 1_000 / ns.max(1)
}

fn measure(home: Handle, how: &str, name: &str) -> Result<(), String> {
    let file = open(home, name, How::CreateFile, file_rights())?;
    let mut buf = alloc::vec![0u8; PIECE];
    // Time inside `user::kaifs` and the commit: waiting on `fs`.
    let mut waited = [0u64; 2];
    let started = sys::time_ns();
    expect(home, Request::Begin { action: 1 }, Reply::Done)?;
    for at in (0..FILE_BYTES).step_by(PIECE) {
        fill(at, &mut buf);
        let t = sys::time_ns();
        kaifs::write_at(file, at as u64, &buf)?;
        waited[0] += sys::time_ns() - t;
    }
    let t = sys::time_ns();
    match ask(home, Request::Commit)? {
        (Reply::Committed { .. }, _) => {}
        other => return Err(format!("bench commit: {other:?}")),
    }
    waited[0] += sys::time_ns() - t;
    let wrote = sys::time_ns() - started;
    let started = sys::time_ns();
    for at in (0..FILE_BYTES).step_by(PIECE) {
        let t = sys::time_ns();
        let got = kaifs::read_at(file, at as u64, &mut buf)?;
        waited[1] += sys::time_ns() - t;
        if got != PIECE {
            return Err(format!("bench read {got} bytes at {at}"));
        }
        if buf.iter().enumerate().any(|(i, &b)| b != byte(at + i)) {
            return Err(format!("bench read back different bytes at {at}"));
        }
    }
    let read = sys::time_ns() - started;
    let _ = sys::handle_close(file);
    let ms = |ns: u64| ns / 1_000_000;
    println!(
        "filecheck: bench in {how}: {} MiB, write {} MB/s ({} ms of {} ms waiting on fs), read {} MB/s ({} ms of {} ms)",
        FILE_BYTES >> 20,
        rate(FILE_BYTES, wrote),
        ms(waited[0]),
        ms(wrote),
        rate(FILE_BYTES, read),
        ms(waited[1]),
        ms(read)
    );
    Ok(())
}
