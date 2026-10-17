//! The kernel self-test cases. Each boots the kernel once with a command line; the
//! kernel decides pass or fail and reports it through isa-debug-exit.

use crate::buildsvc::BuildService;

/// A check on the host after a case passed.
pub type AfterCheck = fn(&BuildService) -> Result<(), String>;

pub struct TestCase {
    pub name: &'static str,
    pub cmdline: &'static str,
    /// QEMU `sendkey` names, in stages: stage N is typed when the kernel logs
    /// "ready for keys" for the Nth time.
    pub keys: &'static [&'static [&'static str]],
    /// Texts that must all appear on the serial console for the case to pass.
    pub expect: &'static [&'static str],
    /// Boot with a build service (`build=` and `build_token=`) on a fresh snapshot
    /// of the working tree.
    pub build: bool,
    /// Checks on the host once the case has passed, given its build service.
    pub after: Option<AfterCheck>,
}

const fn case(name: &'static str, cmdline: &'static str) -> TestCase {
    TestCase {
        name,
        cmdline,
        keys: &[],
        expect: &[],
        build: false,
        after: None,
    }
}

/// "kai" and Enter.
const KAI: &[&str] = &["k", "a", "i", "ret"];

/// "policy rr", "ask hello" and "show me the cpus please" (plain English, so the
/// shell asks the LLM for a script), each followed by Enter. Then, once the terminal
/// shows the question, "yes" and Enter.
const TERMINAL_SCRIPT: &[&[&str]] = &[
    &[
        "p", "o", "l", "i", "c", "y", "spc", "r", "r", "ret", "a", "s", "k", "spc", "h", "e", "l",
        "l", "o", "ret", "s", "h", "o", "w", "spc", "m", "e", "spc", "t", "h", "e", "spc", "c",
        "p", "u", "s", "spc", "p", "l", "e", "a", "s", "e", "ret",
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
        build: true,
        after: Some(crate::buildsvc::verify::after_edit_case),
        ..case("edit", "test=edit")
    },
    TestCase {
        keys: TERMINAL_SCRIPT,
        expect: &[
            crate::mock_llm::REPLY,
            "? run it? it changes nothing. yes/no",
            "run script 1? yes/no> ",
            "= CPUs: ",
        ],
        ..case("terminal", "test=terminal")
    },
];

/// The M5 exit test: a 10-minute spawn, exit and sleep storm.
pub const SOAK_CASES: &[TestCase] = &[case("storm-10min", "test=storm-600")];
