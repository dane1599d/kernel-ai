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
    TestCase {
        expect: &[
            "thread-pin: a pinned thread stayed on its CPU, an unpinned one migrated, and ring 3 can pin",
        ],
        ..case("thread-pin", "test=thread-pin")
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
    // P3 task 4: the raised table limits hold 40 processes at once.
    TestCase {
        expect: &["many-processes: 40 idle processes ran at once beside the shell and llm"],
        ..case("many-processes", "test=many-processes")
    },
    // P3 task 2: desktop claims the screen; Ctrl+Alt+F12 switches it.
    TestCase {
        expect: &[
            "desktop: claimed",
            "desktop: key 0x4 1",
            "desktop: lost the screen to the console",
            "desktop: the screen is back",
            "display-claim: desktop claimed the screen, Ctrl+Alt+F12 gave it to the console and back, and only desktop could hold it",
        ],
        ..case("display-claim", "test=display-claim")
    },
    // P3 task 6: windows open, move and close.
    TestCase {
        expect: &[
            "deskdemo: opened Left and Right",
            "desktop: window 1 \"Left\" opened at 48,52 size 302x234",
            "desktop: window 2 \"Right\" opened at 80,84 size 302x234",
            "desktop: window 1 has focus",
            "desktop: window 1 moved to 248,52 size 302x234",
            "deskdemo: closed Left",
            "desktop: window 1 closed",
            "desktop: window 2 has focus",
            "desk-windows: dragged Left 200 px and pressed its close control",
        ],
        ..case("desk-windows", "test=desk-windows")
    },
    // P3 task 6: desktop killed and restarted; its client reconnects.
    TestCase {
        expect: &[
            "deskdemo: opened Left and Right",
            "display: its holder ended; the console has the screen",
            "deskdemo: reopened Left and Right after desktop restarted",
            "desk-restart: desktop was killed and came back, and deskdemo kept running",
        ],
        ..case("desk-restart", "test=desk-restart")
    },
    // P3 task 8: an ask is answered only on the console, whatever the lease.
    TestCase {
        keys: &[
            &["y", "ret"],
            &["n"],
            &["y", "ret"],
            &["n"],
            &["n"],
            &["y", "ret"],
        ],
        expect: &[
            "display: the console took the screen for an answer",
            "desk-ask: an ask after keys leased to the console was answered only on the console",
            "desk-ask: an ask after keys with desktop was answered only on the console",
            "display: the answer came; the screen went back to its holder",
            "desk-ask: with the console showing already, the ask kept the screen until its answer",
            "desk-ask: the restarted desktop got the keys, with no lease left over",
        ],
        ..case("desk-ask", "test=desk-ask")
    },
    // P3 task 11: an app suspended and resumed through its host.
    TestCase {
        expect: &[
            "notes: suspended version 2",
            "notes: resumed version 2",
            "app-suspend: notes gave ",
        ],
        ..case("app-suspend", "test=app-suspend")
    },
    // Lane M: anonymous memory comes back on release and on exit.
    TestCase {
        expect: &["mem: 1024 committed pages came back on release and 512 on exit; refusals held"],
        ..case("mem", "test=mem")
    },
    // P3 task 6's exit case: the OS boots to the desktop, and its screen is the
    // golden one deskcore::paint's host test paints.
    TestCase {
        keys: &[crate::screen::KEYS],
        expect: &[
            "desktop: drew the desk at 1280x800",
            "desk-boot: desktop booted to the desk and the screendump was taken",
        ],
        screen: Some(crate::screen::Expect {
            size: Some((1280, 800)),
            cursor: false,
            golden: Some(crate::screen::Golden {
                sha256: "514ded10f7fe4dd26d45737cbd9831ff457c6cf5ff59631ecc042c33342a51bb",
                mask: desk_stats,
            }),
        }),
        ..case("desk-boot", "test=desk-boot")
    },
    // P3 task 7: Ctrl+Space focuses the AI panel, and Enter queues its line.
    TestCase {
        expect: &[
            "desktop: focus Panel",
            "desktop: the panel queued a request (2 characters)",
            "desk-panel: typed Ctrl+Space, \"hi\" and Enter",
        ],
        ..case("desk-panel", "test=desk-panel")
    },
    // P3 task 9: an app's kai:ui window on the desktop, closed from its control.
    TestCase {
        expect: &[
            "comphost: window 1 \"Notes\" shown at 640x420",
            "desktop: window 1 \"Notes\" opened at 48,52 size 642x454",
            "comphost: window 1 closed",
            "desktop: window 1 closed",
            "ui-window: Notes opened its window and pressed its close control",
        ],
        ..case("ui-window", "test=ui-window")
    },
    // P3 task 7: asks answered on desktop's card, and on the console when it dies.
    TestCase {
        expect: &[
            "ask: 1 went to desktop's card",
            "desktop: ask 1 shows on the card",
            "desktop: ask 1 answered yes",
            "desk-card: an ask went to desktop's card, and y on it allowed",
            "desktop: ask 2 answered no",
            "desk-card: a letter and Enter did not answer the card, and n denied",
            "ask: 3 came back from the card; the console asks it",
            "desk-card: with desktop killed, the ask came back and the console answered it",
        ],
        ..case("desk-card", "test=desk-card")
    },
    // P3 task 11: a standing instruction fired by Notes' window events.
    TestCase {
        expect: &[
            "desk-standing: the rule was allowed on the card, and the shell holds it",
            "desktop: event minimize notes-window",
            "desk-standing: minimizing Notes fired the rule, and Notes is suspended",
            "desktop: event restore notes-window",
            "desk-standing: restoring Notes resumed it, with its note intact",
        ],
        ..case("desk-standing", "test=desk-standing")
    },
    // P3 task 8: the kernel's terminal in a window, and an ask typed there.
    TestCase {
        keys: &[
            &[
                "w", "h", "a", "t", "spc", "i", "s", "spc", "t", "h", "e", "spc", "s", "c", "h",
                "e", "d", "u", "l", "i", "n", "g", "spc", "p", "o", "l", "i", "c", "y", "ret", "y",
            ],
            &["n"],
            &[
                "w", "h", "a", "t", "spc", "i", "s", "spc", "t", "h", "e", "spc", "s", "c", "h",
                "e", "d", "u", "l", "i", "n", "g", "spc", "p", "o", "l", "i", "c", "y", "ret",
            ],
        ],
        expect: &[
            "term: opened its window",
            "desktop: the terminal window has the keys",
            "term: shows ? ",
            "desk-term: an ask typed in the terminal window was answered only on the console",
            "desk-term: with the focus elsewhere the keys went to desktop, and nothing asked",
        ],
        ..case("desk-term", "test=desk-term")
    },
    // P3 task 12: resource profiles' kernel half.
    TestCase {
        expect: &[
            "profile: a quota stopped a commit, a class showed in SCHED_STATS, and the floors held",
        ],
        ..case("profile", "test=profile")
    },
    // P1 task 14: shared pages, and spike S01-1's M6.
    TestCase {
        expect: &[
            "shm: M6: revoke a 256-page Shm",
            "shm: SHM_CREATE, MAP and UNMAP work, and a revoke leaves holders sharing nothing",
        ],
        ..case("shm", "test=shm")
    },
    // P1 task 14: scriptrun through a Job, and spike S01-1's M3 and M5.
    TestCase {
        expect: &[
            "handles: M3: scriptrun per request",
            "handles: M3: scriptrun pooled",
            "handles: a killed pooled child left the pool, and the next claim got a new one",
            "scriptrun: M5: ",
            "scriptrun: a Job started scriptrun with only the handles moved in, and its exit came back",
        ],
        ..case("scriptrun", "test=scriptrun")
    },
];

/// The top bar's model state and memory, which change while the desk is still:
/// `deskcore::paint::stats_area`, 320 px wide at the right of the 28 px bar,
/// above its 1 px rule.
fn desk_stats(width: u32, _height: u32) -> (u32, u32, u32, u32) {
    (width.saturating_sub(320), 0, 320, 27)
}
