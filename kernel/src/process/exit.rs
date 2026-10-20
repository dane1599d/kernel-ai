//! Every way out of a process ends here: its IPC calls fail, its frames are freed,
//! and its supervisor hears why.

use super::{
    KILL, KILL_DETAIL, Process, SLOT_OF_TID, SPACES, State, killed, programs, request_kill,
    slot_of, supervision, with_table,
};
use crate::{log, memory, threads};
use core::sync::atomic::Ordering;
use kabi::ExitReason;
use x86_64::instructions::interrupts;

/// Ends the calling process. Runs on its own thread with interrupts on and no locks
/// held; never returns.
pub fn exit_current() -> ! {
    debug_assert!(
        interrupts::are_enabled(),
        "exit_current with interrupts off"
    );
    let tid = threads::current_tid();
    let slot = slot_of(tid).expect("exit_current on a kernel thread");
    let reason =
        ExitReason::from_raw(u64::from(KILL[slot].load(Ordering::SeqCst)).saturating_sub(1))
            .filter(|_| killed(slot))
            .unwrap_or(ExitReason::Killed);
    let detail = KILL_DETAIL[slot].load(Ordering::SeqCst);
    let process = with_table(|table| {
        table[slot].state = State::Exiting;
        table[slot]
    });
    crate::ipc::on_process_exit(process.pid);
    threads::leave_address_space();
    let space = SPACES[slot].lock().take();
    let freed = space.map_or(0, |space| memory::with(|m| m.destroy_user_space(space)));
    report_exit(&process, reason, detail, freed);
    SLOT_OF_TID[tid].store(0, Ordering::Release);
    with_table(|table| table[slot] = Process::FREE);
    supervision::on_exit(&process, reason, detail);
    threads::exit()
}

fn report_exit(process: &Process, reason: ExitReason, detail: u64, freed: u64) {
    let name = process.name.as_str();
    let pid = process.pid;
    match (reason, process.crash) {
        (ExitReason::Crashed, Some(crash)) => {
            let fault = crate::faults::vector_name(crash.vector);
            let source = programs::find(process.program.as_str())
                .and_then(|program| program.describe(crash.rip))
                .unwrap_or_default();
            if crash.vector == 14 {
                log!(
                    "process {name} (pid {pid}) killed: {fault} at {:#x} (code {:#x}), rip {:#x} {source}",
                    crash.address,
                    crash.error_code,
                    crash.rip
                );
            } else {
                log!(
                    "process {name} (pid {pid}) killed: {fault} (code {:#x}), rip {:#x} {source}",
                    crash.error_code,
                    crash.rip
                );
            }
        }
        (ExitReason::Runaway, _) => {
            log!("process {name} (pid {pid}) killed: ran {detail} ms without blocking")
        }
        (ExitReason::OutOfMemory, _) => {
            log!("process {name} (pid {pid}) killed: over its {detail}-page memory quota")
        }
        (ExitReason::Exited, _) => log!("process {name} (pid {pid}) exited with code {detail}"),
        (other, _) => log!("process {name} (pid {pid}) {}", other.name()),
    }
    if freed != 0 {
        log!("process {name} (pid {pid}): {freed} pages freed");
    }
}

/// The `exit` system call.
pub fn exit_with(slot: usize, code: u64) -> ! {
    request_kill(slot, ExitReason::Exited, code);
    exit_current()
}
