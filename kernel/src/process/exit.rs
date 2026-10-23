//! Every way out of a process ends here: each thread leaves on its own, and the
//! last one out fails the process's IPC calls, frees its frames and tells its
//! supervisor why.

use super::{
    KILL, KILL_DETAIL, Process, SLOT_OF_TID, State, killed, lock_space, programs, request_kill,
    slot_of, supervision, with_table, workers,
};
use crate::{log, memory, threads};
use core::sync::atomic::Ordering;
use kabi::ExitReason;
use x86_64::instructions::interrupts;

/// Ends the calling thread of a process, and the process with its last thread.
/// Runs on that thread with interrupts on and no locks held; never returns.
///
/// Teardown is safe once the count reaches 0: every other thread has already
/// loaded the kernel page tables here (THREAD_CR3 0, so no later switch loads the
/// process's) and left the count after that, so no CPU has the process's CR3 and,
/// with PCIDs off and user pages never global, no TLB holds its pages. A thread
/// still on its way off a CPU touches only its kernel stack and vector area.
pub fn exit_current() -> ! {
    debug_assert!(
        interrupts::are_enabled(),
        "exit_current with interrupts off"
    );
    let tid = threads::current_tid();
    let slot = slot_of(tid).expect("exit_current on a kernel thread");
    crate::fpu::leave_ring3();
    threads::leave_address_space();
    workers::forget(tid);
    // Before the count drops: once the last thread frees the slot, a new process
    // may take it, and nothing may charge or kill it on this thread's behalf.
    SLOT_OF_TID[tid].store(0, Ordering::Release);
    let (last, process) = with_table(|table| {
        let process = &mut table[slot];
        process.forget_thread(tid);
        process.live_threads -= 1;
        let last = process.live_threads == 0;
        if last {
            process.state = State::Exiting;
        }
        (last, *process)
    });
    if !last {
        // A thread leaving a killed process makes sure the others notice too, also
        // for kills from interrupt context (runaway) that could wake nobody.
        if killed(slot) {
            workers::wake_all(&process.thread_tids());
        }
        threads::exit()
    }
    let reason =
        ExitReason::from_raw(u64::from(KILL[slot].load(Ordering::SeqCst)).saturating_sub(1))
            .filter(|_| killed(slot))
            .unwrap_or(ExitReason::Killed);
    let detail = KILL_DETAIL[slot].load(Ordering::SeqCst);
    crate::ipc::on_process_exit(process.pid);
    let space = lock_space(slot).take();
    let freed = space.map_or(0, |space| memory::with(|m| m.destroy_user_space(space)));
    report_exit(&process, reason, detail, freed);
    crate::sched_control::on_process_exit(process.pid, reason);
    crate::handle::destroy(slot);
    crate::handle::grant::revoke_owned(process.pid);
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
