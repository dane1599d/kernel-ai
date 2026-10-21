//! The kernel self-test cases. Each boots the kernel once with a command line; the
//! kernel decides pass or fail and reports it through isa-debug-exit.

use crate::buildsvc::BuildService;
use crate::disk::Fixture;
use std::time::Duration;

/// A check on the host after a case passed.
pub type AfterCheck = fn(&BuildService) -> Result<(), String>;
/// Changes to the build service's repository before the boot.
pub type Prepare = fn(&BuildService) -> Result<(), String>;

pub struct TestCase {
    pub name: &'static str,
    pub cmdline: &'static str,
    /// QEMU `sendkey` names, or `!` and a monitor command, in stages: stage N is
    /// sent when the kernel logs "ready for keys" for the Nth time.
    pub keys: &'static [&'static [&'static str]],
    /// Texts that must all appear on the serial console for the case to pass.
    pub expect: &'static [&'static str],
    /// Boot with a build service (`build=` and `build_token=`) on a fresh snapshot
    /// of the working tree.
    pub build: bool,
    /// Checks on the host once the case has passed, given its build service.
    pub after: Option<AfterCheck>,
    /// Refs to prepare before the boot manifest is chosen.
    pub prepare: Option<Prepare>,
    /// The case needs at least this long, whatever --timeout says: the edit case
    /// waits on builds whose own limits nest inside 300 s.
    pub min_timeout: Option<Duration>,
    /// A fixture attached as the `kai-model` disk, with its length and hash on the
    /// command line.
    pub disk: Option<Fixture>,
    /// Boot with the mock LLM server on the command line (`llm=`). Without it, the
    /// case also fails if the mock gets any request while it runs.
    pub server: bool,
    /// Guest memory in MiB when the case needs more than its disk does (`bigmap`
    /// needs RAM above 4 GiB); `--mem` still wins if larger.
    pub memory_mib: Option<u64>,
    /// Attach the disk as an NVMe controller (`-device nvme`) instead of
    /// virtio-blk.
    pub nvme: bool,
    /// Boot from the GPT disk image (`image/`) built from the ESP, not from QEMU's
    /// directory ESP.
    pub image: bool,
}

pub(crate) const fn case(name: &'static str, cmdline: &'static str) -> TestCase {
    TestCase {
        name,
        cmdline,
        keys: &[],
        expect: &[],
        build: false,
        after: None,
        prepare: None,
        min_timeout: None,
        disk: None,
        server: true,
        memory_mib: None,
        nvme: false,
        image: false,
    }
}

/// "kai" and Enter.
const KAI: &[&str] = &["k", "a", "i", "ret"];

/// "ask hi" and Enter, at the terminal, which sends it through the shell to llm.
const ASK_HI: &[&str] = &["a", "s", "k", "spc", "h", "i", "ret"];

/// Moves the mouse right 38 and down 40 and clicks; then holds Ctrl for 1.5 s and
/// clicks again inside the hold. Mouse events skip the key queue that delays the
/// Ctrl release, so the second click lands while Ctrl is down.
const POINTER: &[&str] = &[
    "!mouse_move 38 40",
    "!mouse_button 1",
    "!mouse_button 0",
    "!sendkey ctrl 1500",
    "!mouse_button 1",
    "!mouse_button 0",
];

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
    &["n", "o", "ret"],
];

pub const TEST_CASES: &[TestCase] = &[
    case("boot", "test"),
    case("page-fault", "test=page-fault"),
    case("double-fault", "test=double-fault"),
    case("write-protect", "test=write-protect"),
    case("stack-overflow", "test=stack-overflow"),
    case("timer", "test=timer"),
    TestCase {
        keys: &[KAI],
        ..case("keyboard", "test=keyboard")
    },
    TestCase {
        keys: &[POINTER],
        expect: &["pointer: at 38 40 cell 2 5", "pointer: click 1 ctrl"],
        ..case("pointer", "test=pointer pointerlog")
    },
    TestCase {
        keys: &[KAI],
        ..case("sched", "test=sched")
    },
    case("watchdog", "test=watchdog"),
    case("smp", "test=smp"),
    case("tlb", "test=tlb"),
    case("storm", "test=storm"),
    case("net", "test=net"),
    case("shell", "test=shell"),
    case("scripts", "test=scripts"),
    case("live", "test=live"),
    TestCase {
        expect: &[
            "edit: the edit service restarted; restored notes e1 with 1 edit",
            "edit: the edit service restarted; restored notes 2 with 1 edit",
            "[edit] built e3 from 2 (commit ",
        ],
        build: true,
        after: Some(crate::buildsvc::verify::after_edit_case),
        min_timeout: Some(Duration::from_secs(300)),
        ..case("edit", "test=edit")
    },
    TestCase {
        build: true,
        prepare: Some(crate::boot_smoke::prepare_manifest),
        after: Some(crate::boot_smoke::after_manifest),
        min_timeout: Some(Duration::from_secs(300)),
        ..case("manifest", "test=manifest")
    },
    TestCase {
        build: true,
        prepare: Some(crate::boot_smoke::prepare_manifest_good),
        after: Some(crate::boot_smoke::after_manifest_good),
        min_timeout: Some(Duration::from_secs(300)),
        ..case("manifest-good", "test=manifest-good")
    },
    TestCase {
        keys: TERMINAL_SCRIPT,
        expect: &[
            crate::mock_llm::REPLY,
            // The read-only script ran without a question.
            "= ran it (read-only):",
            "= CPUs: ",
            "kai> hello",
            crate::mock_llm::HELLO,
            "? run it? it CHANGES set_policy. yes/no",
            "run script 3? yes/no> no",
            "= script 3 not run",
            // screenlog copies the typed text the screen shows. It matches the
            // `policy rrxz` row too, so only the kernel's policy check proves the edits.
            "|kai> policy rr",
            "kai> xyz^C",
        ],
        ..case("terminal", "test=terminal screenlog")
    },
    TestCase {
        disk: Some(Fixture::Raw),
        expect: &[
            "blk: kai-model reads back byte for byte",
            "model: no GGUF magic; the kai-model disk is ignored",
        ],
        ..case("blk", "test=blk")
    },
    TestCase {
        disk: Some(Fixture::Gguf),
        expect: &[
            "model: llm (pid",
            "model: restarted llm (pid",
            "model: every process gone; USER_FRAMES 0, model frames still allocated",
        ],
        ..case("model", "test=model")
    },
    case("fpu", "test=fpu"),
    case("probe", "test=probe"),
    // M10a-2's probe runs here too, so the case itself is gated under KVM and TCG;
    // `cargo xtask icount` checks its determinism under icount.
    TestCase {
        expect: &["icount-probe: digest "],
        ..case("icount-probe", "test=icount-probe")
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
    // The live scenario with the model on the disk and no server: `:why` is
    // answered by the local model.
    TestCase {
        disk: Some(Fixture::LiveLocal),
        server: false,
        expect: &["live: :why notes was answered by the local model"],
        ..case("live-local", "test=live")
    },
    // The same disk checks over NVMe, the driver VirtualBox and real PCs need.
    TestCase {
        disk: Some(Fixture::Raw),
        nvme: true,
        expect: &[
            "blk: read 3146505 bytes of 6146 sectors over nvme",
            "blk: kai-model reads back byte for byte",
            "model: no GGUF magic; the kai-model disk is ignored",
        ],
        ..case("nvme", "test=blk")
    },
    TestCase {
        disk: Some(Fixture::Gguf),
        nvme: true,
        expect: &["nvme: namespace 1, ", "model: llm (pid"],
        ..case("nvme-model", "test=model")
    },
    // The boot disk VirtualBox gets: Limine and the kernel on a FAT32 ESP in a GPT
    // image, written by xtask, booted the way firmware boots a real disk.
    TestCase {
        image: true,
        expect: &["boot-ok"],
        ..case("image", "test")
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
];

/// "show me the cpus please" and Enter: plain English, so the shell asks llm for a
/// script.
#[rustfmt::skip]
const SHOW_CPUS: &[&str] = &[
    "s", "h", "o", "w", "spc", "m", "e", "spc", "t", "h", "e", "spc",
    "c", "p", "u", "s", "spc", "p", "l", "e", "a", "s", "e", "ret",
];

/// The M5 exit test: a 10-minute spawn, exit and sleep storm.
pub const SOAK_CASES: &[TestCase] = &[case("storm-10min", "test=storm-600")];
