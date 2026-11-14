//! SHELL_INTERRUPT: the person's Stop (the Terminal) or Esc (the panel) for
//! the line the shell runs, from `desktop`, which holds their keys
//! ([`kabi::cap::INPUT`]). It raises the shell's interrupt flag, which its
//! blocking calls and its scripts already check.

use super::need;
use kabi::{Error, call, cap};

/// The process the call interrupts: the one running the person's lines.
const SHELL: &str = "shell";

/// [`call::SHELL_INTERRUPT`], or `None` for a number this area does not have.
pub(super) fn handle(slot: usize, number: u64, _: &[u64; 6]) -> Option<Result<u64, Error>> {
    (number == call::SHELL_INTERRUPT).then(|| interrupt(slot))
}

fn interrupt(slot: usize) -> Result<u64, Error> {
    need(slot, cap::INPUT)?;
    let pid = crate::process::interrupt(SHELL).ok_or(Error::NotFound)?;
    crate::quiet!("interrupt: the shell (pid {pid}) was asked to stop its line");
    Ok(pid)
}
