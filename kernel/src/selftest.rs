//! Which self-test the kernel command line selects, and the service plan each boot
//! starts with.

use alloc::format;
use alloc::string::String;

/// Which of the one door's boot cases runs (`test=act-*`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ActCase {
    Plain,
    Run,
    Never,
    Vague,
    Undo,
    Rollback,
    Ask,
    Inject,
}

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
    /// P3: the display shows a test pattern exactly, and virtio-gpu moves its cursor.
    Display,
    /// P7: keys and pointer reports arrive from a USB keyboard, mouse or tablet.
    Usb,
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
    /// P1 task 7: one tagged call through the one door. `act-plain`: plain
    /// requests become the right calls; `act-run`: the run tier runs with no prompt
    /// and writes the result line; `act-never`: never-tier calls are refused;
    /// `act-vague`: a vague line runs as `Taken as`, a line not in English asks.
    Act(ActCase),
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
    /// S02-2: Enter to the busy sign and to done, with the model on the disk.
    Latency,
    /// PROC_STOP and the supervisor's `stop` and `start` (P1 task 15).
    ProcStop,
    /// `Shm` and spike S01-1's M6 (P1 task 14).
    Shm,
    /// `scriptrun` through a Job, and spike S01-1's M3 and M5 (P1 task 14).
    Scriptrun,
    /// L2: processes keep their vector state across switches and CPUs, and start clean.
    Fpu,
    /// L2: the isolation probe; ops 0-2 complete and every other op is stopped.
    Probe,
    /// P1: handle tables, startup handles and need() on handles.
    Handles,
    /// L6: worker threads start, are refused, filtered and torn down.
    Threads,
    /// L6: processes killed mid-run, every thread blocked or computing, free every frame.
    ThreadsKill,
    /// THREAD_PIN: a pinned thread stays, an unpinned one migrates.
    ThreadPin,
    /// L6: a matvec over the thread pool equals the single-thread result.
    Matvec,
    /// L6: keys reach their thread within 20 ms while ring-3 workers compute.
    ComputeKeys,
    /// L6: the model window, aliased past 4 GiB, reads right and frees exactly.
    Bigmap,
    /// M10a: a seeded probe whose digest must repeat from boot to boot under icount.
    IcountProbe,
    /// M10a-3: kernel-owned trials on the workload revert or keep by the rules.
    Reboot,
    Devices,
    TuneRevert,
    TuneStall,
    /// M10a-4: the shell's `tune` with mock answers, through the kernel's trials.
    Tune,
    /// M10a-4: `tune` on the generated model on the disk, golden from the host.
    TuneLocal,
    /// M10a-4: the founder's demo through the terminal (tune-model.yml).
    TuneDemo,
    /// M10a-5: the exit test's observe or measure boot, under icount.
    TuneAb,
    /// P5: DHCP, ping, the resolver and its cache, RESOLVE from ring 3, renewal.
    NetSetup,
    /// P5: the fallback address when DHCP is silent.
    NetFallback,
    /// P5: `ip=static` at boot.
    NetStatic,
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
            "test=display" => Some(SelfTest::Display),
            "test=usb" => Some(SelfTest::Usb),
            "test=sched" => Some(SelfTest::Sched),
            "test=watchdog" => Some(SelfTest::Watchdog),
            "test=smp" => Some(SelfTest::Smp),
            "test=tlb" => Some(SelfTest::Tlb),
            "test=net" => Some(SelfTest::Net),
            "test=terminal" => Some(SelfTest::Terminal),
            "test=shell" => Some(SelfTest::Shell),
            "test=scripts" => Some(SelfTest::Scripts),
            "test=act-plain" => Some(SelfTest::Act(ActCase::Plain)),
            "test=act-run" => Some(SelfTest::Act(ActCase::Run)),
            "test=act-never" => Some(SelfTest::Act(ActCase::Never)),
            "test=act-vague" => Some(SelfTest::Act(ActCase::Vague)),
            "test=act-undo" => Some(SelfTest::Act(ActCase::Undo)),
            "test=act-rollback" => Some(SelfTest::Act(ActCase::Rollback)),
            "test=act-ask" => Some(SelfTest::Act(ActCase::Ask)),
            "test=act-inject" => Some(SelfTest::Act(ActCase::Inject)),
            "test=live" => Some(SelfTest::Live),
            "test=edit" => Some(SelfTest::Edit),
            "test=manifest" => Some(SelfTest::Manifest),
            "test=manifest-good" => Some(SelfTest::ManifestGood),
            "test=blk" => Some(SelfTest::Blk),
            "test=model" => Some(SelfTest::Model),
            "test=local-ask" => Some(SelfTest::LocalAsk),
            "test=local-script" => Some(SelfTest::LocalScript),
            "test=latency" => Some(SelfTest::Latency),
            "test=proc-stop" => Some(SelfTest::ProcStop),
            "test=shm" => Some(SelfTest::Shm),
            "test=scriptrun" => Some(SelfTest::Scriptrun),
            "test=fpu" => Some(SelfTest::Fpu),
            "test=probe" => Some(SelfTest::Probe),
            "test=handles" => Some(SelfTest::Handles),
            "test=threads" => Some(SelfTest::Threads),
            "test=threads-kill" => Some(SelfTest::ThreadsKill),
            "test=thread-pin" => Some(SelfTest::ThreadPin),
            "test=matvec" => Some(SelfTest::Matvec),
            "test=compute-keys" => Some(SelfTest::ComputeKeys),
            "test=bigmap" => Some(SelfTest::Bigmap),
            "test=icount-probe" => Some(SelfTest::IcountProbe),
            "test=reboot" => Some(SelfTest::Reboot),
            "test=devices" => Some(SelfTest::Devices),
            "test=tune-revert" => Some(SelfTest::TuneRevert),
            "test=tune-stall" => Some(SelfTest::TuneStall),
            "test=tune" => Some(SelfTest::Tune),
            "test=tune-local" => Some(SelfTest::TuneLocal),
            "test=tune-demo" => Some(SelfTest::TuneDemo),
            "test=tune-ab" => Some(SelfTest::TuneAb),
            "test=net-setup" => Some(SelfTest::NetSetup),
            "test=net-fallback" => Some(SelfTest::NetFallback),
            "test=net-static" => Some(SelfTest::NetStatic),
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
            | SelfTest::Act(_)
            | SelfTest::Live
            | SelfTest::Edit
            | SelfTest::Manifest
            | SelfTest::ManifestGood
            | SelfTest::LocalAsk
            | SelfTest::LocalScript
            | SelfTest::Latency
            | SelfTest::ProcStop
            | SelfTest::TuneRevert
            | SelfTest::TuneStall
            | SelfTest::Tune
            | SelfTest::TuneLocal
            | SelfTest::TuneDemo
            | SelfTest::TuneAb
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
    let workload = crate::workload::plan_lines();
    if !workload.is_empty() {
        plan.push('\n');
        plan.push_str(&workload);
    }
    if test == SelfTest::ProcStop {
        plan.push('\n');
        plan.push_str(crate::stop_tests::PLAN);
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
