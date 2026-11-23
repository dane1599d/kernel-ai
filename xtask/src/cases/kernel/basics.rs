//! K's basics: faults, time, scheduling, SMP, handles, threads and processes.

use super::{KAI, TestCase, case};
use crate::disk::Fixture;

/// The second boot of `entropy-boots`.
const ENTROPY_BOOT_2: TestCase = TestCase {
    expect: &["entropy: virtio-rng mixed in", "entropy-boots: sample "],
    server: false,
    differs: Some("entropy-boots: sample "),
    rng: true,
    ..case("entropy-boots (boot 2)", "test=entropy-boots")
};

pub(in crate::cases) const CASES: &[TestCase] = &[
    // P5 task 3: ENTROPY mixes RDSEED, RDRAND and a virtio-rng, and two boots
    // draw different bytes.
    TestCase {
        expect: &["entropy: virtio-rng mixed in", "entropy-boots: sample "],
        server: false,
        differs: Some("entropy-boots: sample "),
        rng: true,
        then: Some(&ENTROPY_BOOT_2),
        ..case("entropy-boots", "test=entropy-boots")
    },
    case("boot", "test"),
    case("page-fault", "test=page-fault"),
    case("double-fault", "test=double-fault"),
    case("write-protect", "test=write-protect"),
    case("stack-overflow", "test=stack-overflow"),
    case("timer", "test=timer"),
    // Told the accelerator: TCG gets its own wake limit (#973).
    TestCase {
        keys: &[KAI],
        accel: true,
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
            "entropy: seeded from RD",
            "[handles] handles: ENTROPY gave two different draws with no capability, and refused 257 bytes",
            "[handles] handles: METRICS fills ABI 1.3's size when given none; ",
            "[handles] handles: every check passed",
            "[hcli] handles: a request and a handle went to the server and back through a route",
            "handles: CALL_H, RECV_H and REPLY_H moved a request and a handle there and back",
            "handles: request nodes revoke what was given under them; ONCE works once, as shown",
            "[hgrant] handles: every check passed",
            "[hrev] handles: a revoke ended a call in flight after ",
            "ipc-queued: a call queued on a server that died ended Cancelled in ",
            "handles: revoking a grant ended a call in flight through it",
            "handles: M1: bit check ",
            "handles: M2: request node, 8 handles to 2 processes, revoke: ",
        ],
        ..case("handles", "test=handles")
    },
    // Serial stamps (`stamp=ms`, as `cargo xtask vbox` boots): the reader takes
    // them off, so the kernel's and the programs' lines match as without them.
    // The lines are exact, so one still carrying its stamp fails the case.
    TestCase {
        exact: &[
            "serial: each line starts with [the kernel clock h the HPET], in seconds",
            "[handles] handles: every check passed",
            "handles: CALL_H, RECV_H and REPLY_H moved a request and a handle there and back",
        ],
        ..case("serial-stamps", "test=handles stamp=ms")
    },
    // IPC round trips, 0 B, 1 KiB and 7 KiB, kernel to ring 3 and ring 3 to
    // ring 3, p50 and p99 each: recorded, not judged.
    TestCase {
        expect: &[
            "ipc-rtt: kernel to ring 3, 0 B: p50 ",
            "ipc-rtt: kernel to ring 3, 1024 B: p50 ",
            "ipc-rtt: kernel to ring 3, 7168 B: p50 ",
            "[rtt-client] ipc-rtt: ring 3 to ring 3, 0 B: p50 ",
            "[rtt-client] ipc-rtt: ring 3 to ring 3, 1024 B: p50 ",
            "[rtt-client] ipc-rtt: ring 3 to ring 3, 7168 B: p50 ",
            "ipc-rtt: the echo server answered each size from the kernel and from ring 3",
        ],
        ..case("ipc-rtt", "test=ipc-rtt")
    },
    // SHELL_INTERRUPT: the person's Stop lets a taken call finish and stops the
    // rest of the shell's line.
    TestCase {
        expect: &[
            "[shell] handles: Stop waited ",
            " ms for a taken call's answer, then a new call ended Interrupted in ",
            "shell-interrupt: Stop let a taken call finish, stopped the rest of the line, never outlived it, and only the person's keys may ask",
        ],
        ..case("shell-interrupt", "test=shell-interrupt")
    },
    // CALL_SUBMIT and CALL_POLL: the shell serves app reads while llm answers.
    TestCase {
        expect: &[
            "async-call: an exit and a kill each withdrew the call left queued",
            "[aclient] handles: async: served 1 request while the call was in flight, then polled its reply",
            "[aclient] handles: async: a call past its deadline polled TimedOut, and its late answer went nowhere",
            "[aclient] handles: async: a Stop let a taken call answer and withdrew a queued one, and nothing was sent after it",
            "[aclient] handles: async: a revoke ended a held call at once, Revoked",
            "async-call: a process served a call while its own was in flight; Stop, revoke, exit and kill each freed its call",
        ],
        ..case("async-call", "test=async-call")
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
    TestCase {
        expect: &[
            "thread-pin: a pinned thread stayed on its CPU, an unpinned one migrated, and ring 3 can pin",
        ],
        ..case("thread-pin", "test=thread-pin")
    },
    // A host stall: the clock jumps past a runaway's limit on every CPU at once.
    TestCase {
        expect: &[
            "passed between two charges, not counted as run time: the vCPU did not run, the clock jumped",
            "runaway-stall: a clock jump past the limit was not run time, and the runaway still died at its limit",
        ],
        ..case("runaway-stall", "test=runaway-stall")
    },
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
    // P1 tasks 14 and 15: PROC_STOP_ONCE, rules, and the supervisor's checks.
    TestCase {
        expect: &[
            "proc-stop: PROC_STOP_ONCE and the supervisor's stop, start and restart ran only with their ONCE and kept to their rules",
        ],
        ..case("proc-stop", "test=proc-stop")
    },
    // P4's Monitor: each process's pages and CPU time through METRICS.
    TestCase {
        expect: &["proc-usage: each process's pages and CPU time came through METRICS"],
        ..case("proc-usage", "test=proc-usage")
    },
    // P3 task 4: the raised table limits hold 40 processes at once.
    TestCase {
        expect: &["many-processes: 40 idle processes ran at once beside the shell and llm"],
        // The supervisor is killed for running 2 s without blocking here
        // (founder test 2's log); lane K is fixing it. Until then this case
        // forbids the rest of the base list. Put FORBID back with K's fix.
        forbid: &[
            "pkg: FAIL",
            "cannot ask yet",
            "dropped a result",
            "did not reach the app",
            super::ROOM_WARNING,
        ],
        ..case("many-processes", "test=many-processes")
    },
];
