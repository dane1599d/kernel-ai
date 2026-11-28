//! K's basics: faults, time, scheduling, SMP, handles, threads and processes.

use super::{KAI, TestCase, case};
use crate::disk::Fixture;

/// `boot-no-hpet`'s second boot (P6 G57): q35 with `hpet=off`, so the FADT's
/// PM timer is the reference clock. Its APIC timer calibration must come
/// within 3% of the first boot's, against the HPET: TCG's 10 ms calibration
/// already varies by over 1% between two HPET boots (6 of 354 pairs, #1660's
/// review).
const NO_HPET_BOOT: TestCase = TestCase {
    no_hpet: true,
    expect: &[
        "hpet: none; the ACPI PM timer at port 0x",
        "counts/ms, PM timer 3579",
        "boot-ok",
    ],
    differs: Some("timer: APIC "),
    near_pct: Some(3),
    ..case("boot-no-hpet (2: no HPET)", "test")
};

/// `iommu`'s second boot: the same IOMMU without `iommu=on`, so the kernel
/// reads the IVRS and leaves the unit alone (the opt-in default until spike
/// S07-2).
const IOMMU_DEFAULT_BOOT: TestCase = TestCase {
    iommu: true,
    expect: &[
        "iommu: 1 unit in the IVRS (0 entries not read)",
        "iommu: left off until iommu=on (spike S07-2); DMA is not confined",
        "boot-ok",
    ],
    forbid: &[
        "pkg: FAIL",
        "cannot ask yet",
        "dropped a result",
        "did not reach the app",
        "supervision: supervisor died",
        crate::cases::ROOM_WARNING,
        "a completion wait came back",
        "; left off",
        "iommu-check:",
    ],
    ..case("iommu (2: off by default)", "test")
};

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
    // P6 G57: a board with no HPET still boots, on the PM timer.
    TestCase {
        expect: &["counts/ms, HPET "],
        differs: Some("timer: APIC "),
        near_pct: Some(3),
        then: Some(&NO_HPET_BOOT),
        ..case("boot-no-hpet", "test")
    },
    // ADR 0007 section 5, v0.3: with iommu=check (opt-in until spike S07-2),
    // the kernel reads QEMU's IVRS, gives its one IOMMU a pass-through
    // device table, a command buffer and an event log, and `iommu=check`
    // proves amdvi's command and event-log formats against
    // QEMU's amd-iommu: a store that lands, invalidations that log nothing,
    // the Status bits, and a bad command decoded as ILLEGAL_COMMAND_ERROR.
    TestCase {
        iommu: true,
        expect: &[
            "iommu: 1 unit in the IVRS (0 entries not read)",
            "iommu: 00:02.0 at 0xfed80000: a completion wait came back in ",
            "iommu-check: status ok",
            "iommu-check: completion wait store ok",
            "iommu-check: invalidations ok",
            "iommu-check: illegal command event ok",
            "iommu-check: every check passed",
            "boot-ok",
        ],
        forbid: &[
            "pkg: FAIL",
            "cannot ask yet",
            "dropped a result",
            "did not reach the app",
            "supervision: supervisor died",
            crate::cases::ROOM_WARNING,
            "; left off",
            "iommu: off on the command line",
            "iommu: left off until iommu=on",
            "FAILED",
        ],
        then: Some(&IOMMU_DEFAULT_BOOT),
        ..case("iommu", "test iommu=check")
    },
    // P6 row 1 (ADR 0006): the nesting census. The kernel logs the VMX, SVM,
    // NPT and NRIPS bits it sees; nesting-census.yml reads them per runner.
    TestCase {
        expect: &["nesting: vendor="],
        ..case("nesting-census", "test")
    },
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
    // x2APIC with more CPUs than xAPIC's old limit here (P6 G48): every CPU
    // comes online, runs pinned threads and takes the shootdown's IPIs.
    TestCase {
        cpus: Some(12),
        expect: &["apic: x2APIC (apic=x2apic)", "smp: 12 CPU(s) online"],
        ..case("smp-x2apic", "test=smp apic=x2apic")
    },
    TestCase {
        cpus: Some(12),
        expect: &["apic: x2APIC (apic=x2apic)", "smp: 12 CPU(s) online"],
        ..case("tlb-x2apic", "test=tlb apic=x2apic")
    },
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
            "handles: a refused move's handle came back under its value with its rights, an insert meanwhile notwithstanding",
            "handles: revoking a grant ended a call in flight through it",
            "handles: Allows left no ONCE handle or request behind, and a rule node went with its shell",
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
    TestCase {
        expect: &[
            "console-race: a key reader whose wake came before it blocked looked again, in wait and in wait_unless",
            "console-race: an armed reader whose slot was taken did not sleep out its poll",
        ],
        ..case("threads", "test=threads")
    },
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
        // The kernel reads the accel word to pick its TCG latency limit.
        accel: true,
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
    // P3 task 4: the raised table limits hold 40 processes at once. The base
    // FORBID's "supervision: supervisor died" is what caught the supervisor
    // killed as a runaway while it started this 45-service plan (#874, fixed by
    // its START_SLICE_MS slicing): a restarted supervisor reattaches the 40
    // and the case's own check still passes (bug hunt 5 H5-6).
    TestCase {
        expect: &["many-processes: 40 idle processes ran at once beside the shell and llm"],
        ..case("many-processes", "test=many-processes")
    },
];
