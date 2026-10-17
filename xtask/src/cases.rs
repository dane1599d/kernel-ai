//! The kernel self-test cases. Each boots the kernel once with a command line; the
//! kernel decides pass or fail and reports it through isa-debug-exit.

pub struct TestCase {
    pub name: &'static str,
    pub cmdline: &'static str,
    /// QEMU `sendkey` names typed when the kernel logs "ready for keys".
    pub keys: &'static [&'static str],
    /// Texts that must all appear on the serial console for the case to pass.
    pub expect: &'static [&'static str],
}

const fn case(name: &'static str, cmdline: &'static str) -> TestCase {
    TestCase {
        name,
        cmdline,
        keys: &[],
        expect: &[],
    }
}

/// "kai" and Enter.
const KAI: &[&str] = &["k", "a", "i", "ret"];

/// "policy rr", Enter, "ask hello", Enter.
const TERMINAL_SCRIPT: &[&str] = &[
    "p", "o", "l", "i", "c", "y", "spc", "r", "r", "ret", "a", "s", "k", "spc", "h", "e", "l", "l",
    "o", "ret",
];

pub const TEST_CASES: &[TestCase] = &[
    case("boot", "test"),
    case("page-fault", "test=page-fault"),
    case("double-fault", "test=double-fault"),
    case("write-protect", "test=write-protect"),
    case("stack-overflow", "test=stack-overflow"),
    case("timer", "test=timer"),
    TestCase {
        keys: KAI,
        ..case("keyboard", "test=keyboard")
    },
    TestCase {
        keys: KAI,
        ..case("sched", "test=sched")
    },
    case("watchdog", "test=watchdog"),
    case("smp", "test=smp"),
    case("tlb", "test=tlb"),
    case("storm", "test=storm"),
    case("net", "test=net"),
    case("shell", "test=shell"),
    TestCase {
        keys: TERMINAL_SCRIPT,
        expect: &[crate::mock_llm::REPLY],
        ..case("terminal", "test=terminal")
    },
];

/// The M5 exit test: a 10-minute spawn, exit and sleep storm.
pub const SOAK_CASES: &[TestCase] = &[case("storm-10min", "test=storm-600")];
