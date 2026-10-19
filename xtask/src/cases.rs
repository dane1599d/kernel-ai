//! The kernel self-test cases. Each boots the kernel once with a command line; the
//! kernel decides pass or fail and reports it through isa-debug-exit.

use crate::buildsvc::BuildService;
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
}

const fn case(name: &'static str, cmdline: &'static str) -> TestCase {
    TestCase {
        name,
        cmdline,
        keys: &[],
        expect: &[],
        build: false,
        after: None,
        prepare: None,
        min_timeout: None,
    }
}

/// "kai" and Enter.
const KAI: &[&str] = &["k", "a", "i", "ret"];

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
/// script), each followed by Enter. Then, once the terminal shows the question, "yes"
/// and Enter. Kept unformatted so each row is one of those steps.
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
    &["y", "e", "s", "ret"],
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
        expect: &["edit: the edit service restarted; restored notes e1 with 1 edit"],
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
            "? run it? it changes nothing. yes/no",
            "run script 1? yes/no> ",
            "= CPUs: ",
            // screenlog copies the typed text the screen shows. It matches the
            // `policy rrxz` row too, so only the kernel's policy check proves the edits.
            "|kai> policy rr",
            "kai> xyz^C",
        ],
        ..case("terminal", "test=terminal screenlog")
    },
];

/// The M5 exit test: a 10-minute spawn, exit and sleep storm.
pub const SOAK_CASES: &[TestCase] = &[case("storm-10min", "test=storm-600")];
