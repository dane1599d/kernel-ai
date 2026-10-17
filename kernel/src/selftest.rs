//! Which self-test the kernel command line selects, and the service plan each boot
//! starts with.

use alloc::string::String;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SelfTest {
    None,
    Boot,
    PageFault,
    DoubleFault,
    WriteProtect,
    StackOverflow,
    Timer,
    Keyboard,
    Sched,
    Watchdog,
    Smp,
    Tlb,
    /// Spawn/exit/sleep storm for this many seconds.
    Storm(u64),
    Net,
    Terminal,
    /// M8a: the Rhai shell answers a fixed list of lines correctly.
    Shell,
    /// M8b: LLM-written scripts are checked, shown, and run only after a yes.
    Scripts,
    /// M9a: a live component swaps versions, migrates its state and rolls back.
    Live,
}

const DEFAULT_STORM_SECONDS: u64 = 60;

pub fn from_cmdline(cmdline: &str) -> SelfTest {
    cmdline
        .split_whitespace()
        .find_map(|arg| match arg {
            "test" => Some(SelfTest::Boot),
            "test=page-fault" => Some(SelfTest::PageFault),
            "test=double-fault" => Some(SelfTest::DoubleFault),
            "test=write-protect" => Some(SelfTest::WriteProtect),
            "test=stack-overflow" => Some(SelfTest::StackOverflow),
            "test=timer" => Some(SelfTest::Timer),
            "test=keyboard" => Some(SelfTest::Keyboard),
            "test=sched" => Some(SelfTest::Sched),
            "test=watchdog" => Some(SelfTest::Watchdog),
            "test=smp" => Some(SelfTest::Smp),
            "test=tlb" => Some(SelfTest::Tlb),
            "test=net" => Some(SelfTest::Net),
            "test=terminal" => Some(SelfTest::Terminal),
            "test=shell" => Some(SelfTest::Shell),
            "test=scripts" => Some(SelfTest::Scripts),
            "test=live" => Some(SelfTest::Live),
            other => other.strip_prefix("test=storm").map(|rest| {
                let seconds = rest.strip_prefix('-').and_then(|s| s.parse().ok());
                SelfTest::Storm(seconds.unwrap_or(DEFAULT_STORM_SECONDS))
            }),
        })
        .unwrap_or(SelfTest::None)
}

/// The service plan for this boot, or None for the boots that run no user space.
/// Not for `test`: its exact frame and heap leak checks must not race processes
/// starting up.
pub fn plan(test: SelfTest) -> Option<String> {
    let wanted = matches!(
        test,
        SelfTest::None
            | SelfTest::Net
            | SelfTest::Terminal
            | SelfTest::Shell
            | SelfTest::Scripts
            | SelfTest::Live
    );
    if !wanted {
        return None;
    }
    // The live case needs only its component, run with probation ended by calls.
    let mut plan = match test {
        SelfTest::Live => String::from(kabi::plan::NOTES_TEST),
        _ => crate::llm::plan_line(),
    };
    // The net case checks only the llm path, so it runs without the shell.
    if !matches!(test, SelfTest::Net | SelfTest::Live) {
        plan.push('\n');
        plan.push_str(kabi::plan::SHELL);
    }
    if test == SelfTest::None {
        plan.push('\n');
        plan.push_str(kabi::plan::NOTES);
        if let Some(edit) = crate::build_service::plan_line() {
            plan.push('\n');
            plan.push_str(&edit);
        }
    }
    Some(plan)
}
