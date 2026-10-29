//! `test=mem` (lane M, for P3 task 11's suspend): anonymous memory comes back.
//! `handles mode=mem` reserves 64 Ki pages (256 MiB of address space), commits
//! and touches 1024 of them, and releases them: the release must free exactly
//! those pages, and the kernel's free frames must rise by at least that many.
//! Then the refusals (zero bytes, a hole, a second release, a commit after
//! one), and a process that exits with 512 pages still committed: its exit
//! frees them too.

use crate::fpu_tests::wait_exit;
use crate::process::{self, SpawnRequest};
use crate::{ipc, log, memory};
use kabi::{ExitReason, Limits, cap};

const NAME: &str = "mem";
const RESERVE: u64 = 64 * 1024;
const COMMIT: u64 = 1024;
const LEFT: u64 = 512;
const LIMITS: Limits = Limits {
    max_pages: 4096,
    max_burst_ms: 30_000,
    caps: cap::SERVE,
    stack_pages: 0,
};

fn free_frames() -> u64 {
    memory::with(|m| m.free_frames())
}

fn ask(line: &str) -> Result<alloc::string::String, &'static str> {
    let reply =
        ipc::call(NAME, line.as_bytes(), 30_000).map_err(|_| "mem: the holder did not answer")?;
    let text = core::str::from_utf8(&reply).unwrap_or("");
    if !text.starts_with("ok") {
        log!("mem: {line} -> {text}");
        return Err("mem: a line was refused");
    }
    Ok(text.into())
}

pub fn check() -> Result<(), &'static str> {
    let pid = process::spawn(&SpawnRequest {
        name: NAME,
        program: "handles",
        args: b"mode=mem",
        limits: LIMITS,
        spawner: 0,
        supervisor: false,
        uses: &[],
    })
    .map_err(|_| "mem: cannot start the holder")?;
    let result = rounds();
    let _ = ask("exit");
    if wait_exit(pid)? != (ExitReason::Exited, 0) {
        return Err("mem: the holder failed");
    }
    let freed_on_exit = result?;
    let after_exit = free_frames();
    if after_exit < freed_on_exit + LEFT {
        log!("mem: free frames {after_exit} after the exit, {freed_on_exit} before it");
        return Err("mem: an exit with committed pages did not free them");
    }
    log!("mem: {COMMIT} committed pages came back on release and {LEFT} on exit; refusals held");
    Ok(())
}

/// The release round and the refusals; free frames just before the exit.
fn rounds() -> Result<u64, &'static str> {
    ask(&alloc::format!("reserve {RESERVE}"))?;
    let before = free_frames();
    ask(&alloc::format!("commit {COMMIT}"))?;
    let committed = free_frames();
    let freed = ask("release")?;
    let after = free_frames();
    log!("mem: free frames {before} before, {committed} committed, {after} released ({freed})");
    if freed != alloc::format!("ok {COMMIT}") {
        return Err("mem: the release did not free every committed page");
    }
    if after < committed + COMMIT {
        return Err("mem: free frames did not rise by the committed pages");
    }
    ask("misuse")?;
    ask(&alloc::format!("reserve {LEFT}"))?;
    ask(&alloc::format!("commit {LEFT}"))?;
    Ok(free_frames())
}
