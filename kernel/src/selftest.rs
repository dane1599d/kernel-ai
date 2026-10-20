//! Which self-test the kernel command line selects, and the service plan each boot
//! starts with.

use alloc::format;
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
    /// M9d: mouse moves and clicks arrive in order with the keyboard's Ctrl.
    Pointer,
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
    /// M9b: a component is edited, built on the host and swapped in live.
    Edit,
    /// M9d: notes swaps to the version the boot manifest names.
    Manifest,
    /// M9d: notes starts on the good version, and the swap rolls back.
    ManifestGood,
    /// L3: the `kai-model` disk reads back exactly the file xtask attached.
    Blk,
    /// L3: the model is held in memory and mapped into `llm`, and survives it.
    Model,
    /// L4: `ask` is answered by the model on the disk, with no LLM server.
    LocalAsk,
    /// L5: the model on the disk writes a script the shell checks, with no server.
    LocalScript,
    /// L2: processes keep their vector state across switches and CPUs, and start clean.
    Fpu,
    /// L2: the isolation probe; ops 0-2 complete and every other op is stopped.
    Probe,
    /// L6: worker threads start, are refused, filtered and torn down.
    Threads,
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
            "test=pointer" => Some(SelfTest::Pointer),
            "test=sched" => Some(SelfTest::Sched),
            "test=watchdog" => Some(SelfTest::Watchdog),
            "test=smp" => Some(SelfTest::Smp),
            "test=tlb" => Some(SelfTest::Tlb),
            "test=net" => Some(SelfTest::Net),
            "test=terminal" => Some(SelfTest::Terminal),
            "test=shell" => Some(SelfTest::Shell),
            "test=scripts" => Some(SelfTest::Scripts),
            "test=live" => Some(SelfTest::Live),
            "test=edit" => Some(SelfTest::Edit),
            "test=manifest" => Some(SelfTest::Manifest),
            "test=manifest-good" => Some(SelfTest::ManifestGood),
            "test=blk" => Some(SelfTest::Blk),
            "test=model" => Some(SelfTest::Model),
            "test=local-ask" => Some(SelfTest::LocalAsk),
            "test=local-script" => Some(SelfTest::LocalScript),
            "test=fpu" => Some(SelfTest::Fpu),
            "test=probe" => Some(SelfTest::Probe),
            "test=threads" => Some(SelfTest::Threads),
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
            | SelfTest::Edit
            | SelfTest::Manifest
            | SelfTest::ManifestGood
            | SelfTest::LocalAsk
            | SelfTest::LocalScript
    );
    if !wanted {
        return None;
    }
    // The live, edit and manifest cases need only their services, with probation
    // ended by calls; the live case also needs llm for `:why`, and the others the
    // edit service, which syncs with git.
    let mut plan = match test {
        SelfTest::Live => format!("{}\n{}", kabi::plan::NOTES_TEST, crate::llm::plan_line()),
        SelfTest::Edit | SelfTest::Manifest | SelfTest::ManifestGood => {
            let mut plan = match test {
                SelfTest::Edit => String::from(kabi::plan::NOTES_EDIT_TEST),
                _ => format!(
                    "{} {}",
                    crate::boot_manifest::plan_line(),
                    kabi::plan::CALLS_ONLY
                ),
            };
            if let Some(edit) = crate::build_service::plan_line() {
                plan.push('\n');
                plan.push_str(&edit);
            }
            plan
        }
        _ => crate::llm::plan_line(),
    };
    // The net case checks only the llm path, so it runs without the shell.
    if !matches!(
        test,
        SelfTest::Net
            | SelfTest::Live
            | SelfTest::Edit
            | SelfTest::Manifest
            | SelfTest::ManifestGood
    ) {
        plan.push('\n');
        plan.push_str(kabi::plan::SHELL);
    }
    if test == SelfTest::None {
        plan.push('\n');
        plan.push_str(&crate::boot_manifest::plan_line());
        if let Some(edit) = crate::build_service::plan_line() {
            plan.push('\n');
            plan.push_str(&edit);
        }
    }
    Some(plan)
}
