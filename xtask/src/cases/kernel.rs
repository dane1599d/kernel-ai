//! K, the kernel: faults, time, scheduling, SMP, threads and memory.

use super::{KAI, TestCase, case};
use crate::disk::Fixture;

pub(super) const CASES: &[TestCase] = &[
    case("boot", "test"),
    case("page-fault", "test=page-fault"),
    case("double-fault", "test=double-fault"),
    case("write-protect", "test=write-protect"),
    case("stack-overflow", "test=stack-overflow"),
    case("timer", "test=timer"),
    TestCase {
        keys: &[KAI],
        ..case("sched", "test=sched")
    },
    case("watchdog", "test=watchdog"),
    case("smp", "test=smp"),
    case("tlb", "test=tlb"),
    case("storm", "test=storm"),
    case("fpu", "test=fpu"),
    case("probe", "test=probe"),
    // P1 tasks 5, 12 and 13: handle tables, startup handles and need() on handles,
    // from the kernel and from ring 3; handle IPC through a route; grant nodes,
    // revoke and ONCE; spike S01-1's M1 and M2.
    TestCase {
        expect: &[
            "[handles] handles: every check passed",
            "[hcli] handles: a request and a handle went to the server and back through a route",
            "handles: CALL_H, RECV_H and REPLY_H moved a request and a handle there and back",
            "handles: request nodes revoke what was given under them; ONCE works once, as shown",
            "[hgrant] handles: every check passed",
            "[hrev] handles: a revoke ended a call in flight after ",
            "handles: revoking a grant ended a call in flight through it",
            "handles: M1: bit check ",
            "handles: M2: request node, 8 handles to 2 processes, revoke: ",
        ],
        ..case("handles", "test=handles")
    },
    // M10a-2's probe runs here too, so the case itself is gated under KVM and TCG;
    // `cargo xtask icount` checks its determinism under icount.
    TestCase {
        expect: &["icount-probe: digest "],
        ..case("icount-probe", "test=icount-probe")
    },
    // L6: worker threads in one address space.
    case("threads", "test=threads"),
    case("threads-kill", "test=threads-kill"),
    case("matvec", "test=matvec"),
    // The graft: the model window past 4 GiB, with RAM above 4 GiB for its copies.
    TestCase {
        disk: Some(Fixture::Gguf),
        memory_mib: Some(4608),
        expect: &["bigmap: 3 readers read past 4 GiB of the window"],
        ..case("bigmap", "test=bigmap")
    },
    // The graft: keys still echo while every vCPU computes in ring 3.
    TestCase {
        keys: &[KAI],
        ..case("compute-keys", "test=compute-keys")
    },
    // P1 task 15: PROC_STOP, and the supervisor's stop and start.
    TestCase {
        expect: &["proc-stop: PROC_STOP and the supervisor's stop and start kept to their rules"],
        ..case("proc-stop", "test=proc-stop")
    },
    // P1 task 14: shared pages, and spike S01-1's M6.
    TestCase {
        expect: &[
            "shm: M6: revoke a 256-page Shm",
            "shm: SHM_CREATE, MAP and UNMAP work, and a revoke leaves holders sharing nothing",
        ],
        ..case("shm", "test=shm")
    },
];
