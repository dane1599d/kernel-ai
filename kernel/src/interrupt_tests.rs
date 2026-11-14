//! `test=shell-interrupt`: the person's Stop for the shell's line. A holding
//! server takes a call from `handles mode=stop` started as `shell`; the
//! kernel's [`process::interrupt`] (what SHELL_INTERRUPT does for `desktop`)
//! ends that call as Interrupted, and the flag does not outlive it. From ring 3
//! the call is refused without `cap::INPUT`; before the test starts one, the
//! kernel finds no shell to interrupt. A Stop pressed once the shell is idle
//! is cleared when it takes its next line.

use crate::fpu_tests::wait_exit;
use crate::process::{self, SpawnRequest};
use crate::{ipc, log, threads};
use kabi::{ExitReason, Limits, cap};

const LIMITS: Limits = Limits {
    max_pages: 256,
    max_burst_ms: 10_000,
    caps: 0,
    stack_pages: 0,
};
/// How long the holder may take to have the call in hand.
const HELD_WAIT_MS: u64 = 10_000;
const POLL_MS: u64 = 10;
/// The kernel's next line to the shell.
const LINE_MS: u64 = 5_000;

fn start(
    name: &'static str,
    args: &'static str,
    caps: u64,
    uses: &'static [&'static str],
) -> Result<u64, &'static str> {
    process::spawn(&SpawnRequest {
        name,
        program: "handles",
        args: args.as_bytes(),
        limits: Limits { caps, ..LIMITS },
        spawner: 0,
        supervisor: false,
        uses,
    })
    .map_err(|_| "shell-interrupt: cannot start a handles process")
}

pub fn check() -> Result<(), &'static str> {
    if process::interrupt("shell").is_some() {
        return Err("shell-interrupt: a shell runs before the test started one");
    }
    let holder = start("sihold", "mode=hold", cap::SERVE, &[])?;
    let shell = start("shell", "mode=stop", cap::CALL | cap::SERVE, &["sihold"])?;
    let held = (0..HELD_WAIT_MS / POLL_MS).any(|_| {
        let serving = ipc::serving_deadline(holder).is_some();
        if !serving {
            threads::sleep_ms(POLL_MS);
        }
        serving
    });
    if !held {
        return Err("shell-interrupt: the holder never took the shell's call");
    }
    if process::interrupt("shell") != Some(shell) {
        return Err("shell-interrupt: the interrupt did not find the shell");
    }
    // Once the stopped call ended the shell serves: a Stop pressed now, with
    // no line running, must not reach its next line.
    let serving = (0..HELD_WAIT_MS / POLL_MS).any(|_| {
        let bound = ipc::server_of("shell").is_some();
        if !bound {
            threads::sleep_ms(POLL_MS);
        }
        bound
    });
    if !serving {
        return Err("shell-interrupt: the shell never served after its stopped call");
    }
    process::interrupt("shell").ok_or("shell-interrupt: the shell was gone")?;
    let answer = ipc::call("shell", b"line", LINE_MS)
        .map_err(|_| "shell-interrupt: the shell did not take the next line")?;
    if answer != b"clean" {
        return Err("shell-interrupt: a stale Stop reached the next line");
    }
    for pid in [shell, holder] {
        if wait_exit(pid)? != (ExitReason::Exited, 0) {
            return Err("shell-interrupt: the stopped shell's checks failed");
        }
    }
    log!(
        "shell-interrupt: Stop ended the shell's call in flight, never outlived it, and only the person's keys may ask"
    );
    Ok(())
}
