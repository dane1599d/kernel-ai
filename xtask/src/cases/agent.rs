//! A, the agent: the llm service, the shell, the terminal, scripts and tune.

use super::{TestCase, case};
use crate::blklog::Data;
use crate::disk::Fixture;

/// A fresh `kai-data` disk, for the shell's `/system/state`.
const DATA: Data = Data {
    nvme: false,
    log: None,
    gpt: true,
};

/// Boot 2 of `keep-running`: the shell replays `/system/state/keep`.
const KEEP_RUNNING_2: TestCase = TestCase {
    expect: &[
        "keep-running: boot 2 kept notes-window running from /system/state/keep",
        "keep-running: user=1: ok notes-window keep-running off",
        "keep-running: boot 2 let notes-window go, and it napped",
    ],
    data: Some(DATA),
    ..case("keep-running (boot 2)", "test=keep-running reread")
};

/// "ask hi" and Enter, at the terminal, which sends it through the shell to llm.
const ASK_HI: &[&str] = &["a", "s", "k", "spc", "h", "i", "ret"];

/// "xyz", cleared with Ctrl-C, then "pol"; while it shows at the prompt the kernel
/// logs a line, which must land above it. Then "cy rrxz", edited into "policy rr"
/// (Home; Up, PgDn and Esc do nothing; Right three times, "i"; End, Backspace takes
/// the z; Left, Delete takes the x, where a Backspace would take an r), "ask hello"
/// and "show me the cpus please" (plain English, so the shell asks the LLM for a
/// script, which only reads and runs at once), each followed by Enter. Once that
/// reply shows, "hello" (a word the AI answers) and "go back to interactive" (a
/// script that changes the policy, so it asks). Then, once the terminal shows the
/// question, "no" and Enter. Kept unformatted so each row is one of those steps.
#[rustfmt::skip]
const TERMINAL_SCRIPT: &[&[&str]] = &[
    &["x", "y", "z", "ctrl-c", "p", "o", "l"],
    &[
        "c", "y", "spc", "r", "r", "x", "z",
        "home", "up", "pgdn", "esc", "right", "right", "right", "i",
        "end", "backspace", "left", "delete", "ret",
        "a", "s", "k", "spc", "h", "e", "l", "l", "o", "ret",
        "s", "h", "o", "w", "spc", "m", "e", "spc", "t", "h", "e", "spc",
        "c", "p", "u", "s", "spc", "p", "l", "e", "a", "s", "e", "ret",
    ],
    &[
        "h", "e", "l", "l", "o", "ret",
        "g", "o", "spc", "b", "a", "c", "k", "spc", "t", "o", "spc",
        "i", "n", "t", "e", "r", "a", "c", "t", "i", "v", "e", "ret",
    ],
    &["u", "n", "d", "o", "ret"],
];

/// "show me the cpus please" and Enter: plain English, so the shell asks llm for a
/// script.
#[rustfmt::skip]
const SHOW_CPUS: &[&str] = &[
    "s", "h", "o", "w", "spc", "m", "e", "spc", "t", "h", "e", "spc",
    "c", "p", "u", "s", "spc", "p", "l", "e", "a", "s", "e", "ret",
];

pub(super) const CASES: &[TestCase] = &[
    case("net", "test=net"),
    case("shell", "test=shell"),
    case("scripts", "test=scripts"),
    // P1 task 7: the one door, with the mock server's calls keyed by request.
    TestCase {
        expect: &["act-plain: plain requests became the right calls, and undo put the policy back"],
        ..case("act-plain", "test=act-plain")
    },
    TestCase {
        expect: &["act-run: the run tier ran with no prompt and wrote the result line"],
        ..case("act-run", "test=act-run")
    },
    TestCase {
        expect: &["act-never: never-tier calls were refused and changed nothing"],
        ..case("act-never", "test=act-never")
    },
    TestCase {
        expect: &["act-vague: a vague line ran as Taken as, and a line not in English asked"],
        ..case("act-vague", "test=act-vague")
    },
    TestCase {
        expect: &[
            "act-undo: undo put back, undo again redid, and a typed switch was not overwritten",
        ],
        ..case("act-undo", "test=act-undo")
    },
    // P3 task 12: resource profiles through the supervisor's PROC_CLASS_SET and
    // PROC_QUOTA_SET.
    TestCase {
        expect: &[
            "profile-switch: gaming sent the apps back, quiet lowered a quota, undo put it back, and the shell kept its own",
        ],
        ..case("profile-switch", "test=profile-switch")
    },
    TestCase {
        expect: &["act-rollback: a script that switched and then threw left nothing changed"],
        ..case("act-rollback", "test=act-rollback")
    },
    TestCase {
        expect: &[
            "act-ask: the menu's Allow ran a call once through its ONCE; none, a typed y, a reused one and one for other arguments did not",
        ],
        ..case("act-ask", "test=act-ask")
    },
    TestCase {
        expect: &[
            "act-inject: 8 injected calls: 5 held for a question and declined, 1 refused, 2 left as text; nothing changed",
        ],
        ..case("act-inject", "test=act-inject")
    },
    TestCase {
        expect: &[
            "act-rule: Always allow wrote a rule node, the next stop ran under it, rules_remove took it away, and a service named as a process stopped as a service",
        ],
        ..case("act-rule", "test=act-rule")
    },
    TestCase {
        expect: &[
            "pkg-local: install notes ran from the image's repository and undo put the previous set back",
        ],
        ..case("pkg-local", "test=pkg-local")
    },
    TestCase {
        expect: &[
            "act-convo: a second turn read \"do it again\", \"undo that\" and \"the same for\" from memory, still asked, and `new` forgot it",
        ],
        ..case("act-convo", "test=act-convo")
    },
    TestCase {
        keys: TERMINAL_SCRIPT,
        expect: &[
            crate::mock_llm::REPLY,
            // The read-only script ran without a question, showing only its output.
            "= CPUs: ",
            "kai> hello",
            crate::mock_llm::HELLO,
            // So did the switch, which says what it did and how to undo it.
            "= Done: policy interactive (was round-robin). undo",
            "kai> undo",
            "= Undone: policy back to round-robin",
            // screenlog copies the typed text the screen shows. It matches the
            // `policy rrxz` row too, so only the kernel's policy check proves the edits.
            "|kai> policy rr",
            "kai> xyz^C",
        ],
        ..case("terminal", "test=terminal screenlog")
    },
    // M10a-3: kernel-owned trials on mix A, with no LLM.
    TestCase {
        expect: &[
            "tune-revert: the guardrail reverted every CPU within 100 ms + 1 tick, before the watchdog",
            "tune-revert: an ejection during the trial restored interactive on every CPU, not round-robin",
            "tune-revert: (d) a policy switch cancelled the trial and won",
            "tune-revert: no workload process was killed as a runaway",
        ],
        ..case("tune-revert", "test=tune-revert workload=A seed=1")
    },
    // A CPU held off its ticks is not a policy fault, also when a trial's revert
    // lands during the hold (the race that made tune-revert flaky).
    TestCase {
        expect: &[
            "tune-stall: (a) a 300 ms hold on cpu ",
            "with threads waiting; their waits count from now",
            "tune-stall: (b) three held-off reverts left every CPU on interactive",
        ],
        ..case("tune-stall", "test=tune-stall workload=A seed=1")
    },
    // M10a-4: `tune` at the shell with the mock server's canned answers.
    TestCase {
        expect: &[
            "tune: (a) the good pick's verdict is the judge's",
            "tune: (b) round-robin was rolled back",
            "tune: (c) an invalid answer kept the policy and started nothing",
            "tune: (d) the shell was killed mid-trial; every CPU is back on interactive",
        ],
        ..case("tune", "test=tune workload=A seed=1")
    },
    // M10a-4's fixture variant: the generated tune model on the disk, no server;
    // the local answer must be the host engine's (local_golden=).
    TestCase {
        disk: Some(Fixture::LocalTune),
        server: false,
        expect: &[
            "local-tune: the local model's tune answer equals the host engine's, byte for byte",
            "local-tune: the shell's tune ran end to end on the local model",
        ],
        ..case("tune-local", "test=tune-local workload=A seed=1")
    },
    // L4's exit test: the model on the disk answers, inside Kernel AI, with no
    // server. xtask adds the golden answer's exact line and the health text.
    TestCase {
        disk: Some(Fixture::LocalAsk),
        server: false,
        keys: &[ASK_HI],
        expect: &[
            "kai> ask hi",
            "local-ask: the kernel's ask got the golden answer",
            "local-ask: answered locally; the restarted llm remapped the model, not reloaded it",
        ],
        ..case("local-ask", "test=local-ask")
    },
    // L5's exit test: the model on the disk writes the script for a plain-English
    // request, and the shell checks it, with no server. xtask adds the shell's
    // reply, worked out on the host, as an exact line.
    TestCase {
        disk: Some(Fixture::LocalScript),
        server: false,
        keys: &[SHOW_CPUS],
        expect: &[
            "kai> show me the cpus please",
            "local-script: the local model's script got the host's outcome, twice, with no server",
        ],
        ..case("local-script", "test=local-script")
    },
    // P4 task 7: Settings' keep-running switch reaches the supervisor through
    // the shell alone, holds a napping app awake, and survives a reboot.
    TestCase {
        expect: &[
            "keep-running: keep from the Settings app itself: err only the shell keeps apps running",
            "keep-running: user=0: err a change needs a key or a click",
            "keep-running: user=1: ok notes-window keep-running on",
            "keep-running: boot 1 kept notes-window running from a Settings click, shell only",
        ],
        data: Some(DATA),
        then: Some(&KEEP_RUNNING_2),
        ..case("keep-running", "test=keep-running")
    },
];
