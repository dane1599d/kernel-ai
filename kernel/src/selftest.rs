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
    Rule,
    PkgLocal,
    Convo,
    FsAct,
    FsUndo,
    FsTrash,
    /// `fs-undo-reboot`'s first boot, then its second (`reread`).
    FsUndoReboot,
    FsUndoRebooted,
    /// `app-route` (P4): the agent calls an app's row through the supervisor.
    AppRoute,
    /// `app-terminal` (P4 task 4): the agent types into the Terminal app and a
    /// person's Enter runs the line.
    AppTerminal,
    /// `profile-switch` (P3 task 12): profiles through the supervisor.
    Profile,
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
    /// P4 task 12: the `c>` prompt through the shell and `cc` (ADR 0012).
    CPrompt,
    /// M8b: LLM-written scripts are checked and run by their tier.
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
    /// P1 lane C: S03-1's view rebuild and layout figures in hard-float ring 3.
    AppBench,
    /// P4 lane U: spikes S08-3 and S11-1's `kaituf` figures in soft-float ring 3.
    TufBench,
    /// M9d: notes swaps to the version the boot manifest names.
    Manifest,
    /// M9d: notes starts on the good version, and the swap rolls back.
    ManifestGood,
    /// L3: the `kai-model` disk reads back exactly the file xtask attached.
    Blk,
    /// P2 task 1: the `kai-data` disk takes writes and a flush, and reads them back.
    BlkWrite,
    /// S04-3 (ADR 0004): raw commits to `kai-data`, flushed in order.
    BlkFlush,
    /// P2 task 3: only `fs` holds a `Partition`, and it reaches `kai-data` alone.
    Part,
    /// P2 task 8, `fs-reboot`: files written through a `Dir` handle, read back
    /// in a second boot (`reread`).
    Files {
        reread: bool,
    },
    /// An app's state survives a reboot through its folder (P4): boot 1, then
    /// boot 2 (`reread`).
    AppSaved {
        reread: bool,
    },
    /// P2 task 8, `fs-restart`: `fs` killed mid-action leaves nothing half done,
    /// and a `File` handle works again.
    FilesRestart,
    /// `fs-dead-client`: a client killed between Begin and Commit leaves
    /// nothing, and the next Begin goes through.
    FilesDeadClient,
    /// P2 task 9, `fs-actions`: 200 file actions behind blklogwrites, replayed
    /// on the host.
    FilesActions,
    /// P2 task 9, `fs-powercut`: boot `boot` checks every file after the last
    /// cut, then makes actions until xtask cuts the power; `last` ends the case.
    FilesChurn {
        boot: u64,
        last: bool,
    },
    /// P2 task 13, `pkg-store`: kaipkg's store in `pkg` over `fs`, across three
    /// boots on one data disk; and P4 task 1, `pkg-image`: the image's signed
    /// repository as generation 1.
    PkgStore(crate::pkg_tests::Step),
    /// P2 task 10, `model-store`: with no `kai-model` disk, `fs` fills the runs
    /// the boot reserved from `kai-modelhint 1` with the store's sealed model;
    /// with `bad`, the hint's FNV-1a64 is not the file's, and the kernel refuses
    /// it.
    ModelStore {
        bad: bool,
    },
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
    /// Each process's pages and CPU time through METRICS (for P4's Monitor).
    ProcUsage,
    /// `Shm` and spike S01-1's M6 (P1 task 14).
    Shm,
    /// Anonymous memory reserved, committed and given back (lane M).
    Mem,
    /// Resource profiles: a quota stops a commit, a class shows, floors hold (P3 task 12).
    Profile,
    /// The raised table limits (P3 task 4, ADR 0003 §6): 40 processes at once.
    ManyProcesses,
    /// `desktop` claims the screen; Ctrl+Alt+F12 switches it (P3 task 2).
    DisplayClaim,
    /// Windows open, move and close (P3 task 6).
    DeskWindows,
    /// `desktop` killed and restarted; clients reconnect (P3 task 6).
    DeskRestart,
    DeskAsk,
    /// An app suspended and resumed through its host (P3 task 11).
    AppSuspend,
    /// `event-wake` (P4): a post wakes the shell, never in place of a line.
    EventWake,
    /// An app not started at boot, started by the first request to it (P3 task 12b).
    AppOnDemand,
    /// The OS boots to the desktop, whose screen matches its golden (P3 task 6).
    DeskBoot,
    /// The AI panel takes focus and queues a typed line (P3 task 7).
    DeskPanel,
    /// `desk-panel-shell` (P3 task 7): the shell takes the panel's line.
    DeskPanelShell,
    /// `desk-catalog` (P4 task 3): the Apps sheet lists `pkg`'s catalogue.
    DeskCatalog,
    /// An app's kai:ui window on the desktop, closed from its control (P3 task 9).
    UiWindow,
    /// The editor app's window on the desktop, opened by the agent's door, typed
    /// into and built in the background (P4 task 11).
    AppEditor,
    /// The System monitor's window on the desktop, given a sample of the machine
    /// and closed from its control (P4 task 9).
    UiMonitor,
    /// An unused app naps and a click wakes it (P3 task 12b).
    DeskNap,
    /// A saved appearance repaints `desktop` at once and holds across its
    /// restart (P4 task 8).
    DeskAppearance,
    /// Settings' keep-running switch, shell only, kept across a reboot (P4
    /// task 7): boot 1, then boot 2 (`reread`).
    KeepRunning {
        reread: bool,
    },
    /// Asks answered on `desktop`'s card, and on the console when it dies (P3 task 7).
    DeskCard,
    /// A standing instruction fired by Notes' window minimized and restored (P3 task 11).
    DeskStanding,
    /// No standing instruction from text the AI read, and no firing outside
    /// the rule allowed (P3's exit case).
    DeskStandingRefused,
    /// S05-1's budgets timed on the real `desktop` (P3's exit case).
    DeskPerf,
    /// The keyboard reaches an app's widgets (key input for uikit apps).
    UiKeys,
    DeskTerm,
    /// `desk-agent` (P3 task 10): the agent opens Notes and acts inside it.
    DeskAgent,
    /// The v0.1 system as the default boot plans it: the desktop, the Apps
    /// sheet's catalogue, and an app opened (`desk-default`).
    DeskDefault,
    /// The same system with no memory left for an app: opening it says so
    /// (`desk-no-memory`).
    DeskNoMemory,
    /// The Settings app on the desktop, its window kept from the agent (P4 task 7).
    AppSettings,
    /// `test=desk-tour`: screenshots of the desktop and its apps, not a gate.
    DeskTour,
    /// `desk-takeover` (P3 task 10): a key in the agent's window stops it.
    DeskTakeover,
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
    /// A panic or failed check that resets the machine (ADR 0008 "fault").
    PanicReset,
    /// UEFI runtime variables (ADR 0008).
    Efi,
    /// A reset through UEFI ResetSystem.
    EfiReset,
    /// The boot lever on a slot image, driven by `slot-do=` (ADR 0008, lane U).
    Slot,
    /// Probation's commit on a slot image (ADR 0008 §5).
    Probation,
    /// `updated step=check` on a slot image: the Update calls, then a stage of
    /// the other slot and a restart into it.
    Update,
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

/// The self-test the command line names. It runs at the top of `kmain`, before
/// the heap exists, so nothing it calls may allocate.
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
            "test=c-prompt" => Some(SelfTest::CPrompt),
            "test=scripts" => Some(SelfTest::Scripts),
            "test=act-plain" => Some(SelfTest::Act(ActCase::Plain)),
            "test=act-run" => Some(SelfTest::Act(ActCase::Run)),
            "test=act-never" => Some(SelfTest::Act(ActCase::Never)),
            "test=act-vague" => Some(SelfTest::Act(ActCase::Vague)),
            "test=act-undo" => Some(SelfTest::Act(ActCase::Undo)),
            "test=act-rollback" => Some(SelfTest::Act(ActCase::Rollback)),
            "test=act-ask" => Some(SelfTest::Act(ActCase::Ask)),
            "test=act-inject" => Some(SelfTest::Act(ActCase::Inject)),
            "test=act-rule" => Some(SelfTest::Act(ActCase::Rule)),
            "test=pkg-local" => Some(SelfTest::Act(ActCase::PkgLocal)),
            "test=act-convo" => Some(SelfTest::Act(ActCase::Convo)),
            "test=fs-act" => Some(SelfTest::Act(ActCase::FsAct)),
            "test=fs-undo" => Some(SelfTest::Act(ActCase::FsUndo)),
            "test=fs-trash" => Some(SelfTest::Act(ActCase::FsTrash)),
            "test=app-route" => Some(SelfTest::Act(ActCase::AppRoute)),
            "test=app-terminal" => Some(SelfTest::Act(ActCase::AppTerminal)),
            "test=profile-switch" => Some(SelfTest::Act(ActCase::Profile)),
            "test=fs-undo-reboot" => Some(SelfTest::Act(
                match cmdline.split_whitespace().any(|w| w == "reread") {
                    true => ActCase::FsUndoRebooted,
                    false => ActCase::FsUndoReboot,
                },
            )),
            "test=live" => Some(SelfTest::Live),
            "test=edit" => Some(SelfTest::Edit),
            "test=appbench" => Some(SelfTest::AppBench),
            "test=tuf-bench" => Some(SelfTest::TufBench),
            "test=manifest" => Some(SelfTest::Manifest),
            "test=manifest-good" => Some(SelfTest::ManifestGood),
            "test=blk" => Some(SelfTest::Blk),
            "test=blk-write" => Some(SelfTest::BlkWrite),
            "test=blk-flush" => Some(SelfTest::BlkFlush),
            "test=part" => Some(SelfTest::Part),
            "test=model-store" => Some(SelfTest::ModelStore {
                bad: cmdline.split_whitespace().any(|w| w == "bad"),
            }),
            "test=files" => Some(SelfTest::Files {
                reread: cmdline.split_whitespace().any(|w| w == "reread"),
            }),
            "test=app-saved" => Some(SelfTest::AppSaved {
                reread: cmdline.split_whitespace().any(|w| w == "reread"),
            }),
            "test=files-restart" => Some(SelfTest::FilesRestart),
            "test=files-dead-client" => Some(SelfTest::FilesDeadClient),
            "test=files-actions" => Some(SelfTest::FilesActions),
            "test=files-churn" => Some(SelfTest::FilesChurn {
                boot: cmdline
                    .split_whitespace()
                    .find_map(|w| w.strip_prefix("boot="))
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(0),
                last: cmdline.split_whitespace().any(|w| w == "last"),
            }),
            "test=pkg-store" => Some(SelfTest::PkgStore(crate::pkg_tests::Step::from_cmdline(
                cmdline,
            ))),
            "test=pkg-image" => Some(SelfTest::PkgStore(
                crate::pkg_tests::Step::image_from_cmdline(cmdline),
            )),
            "test=pkg-install" => Some(SelfTest::PkgStore(
                crate::pkg_tests::Step::mirror_from_cmdline(cmdline),
            )),
            "test=model" => Some(SelfTest::Model),
            "test=local-ask" => Some(SelfTest::LocalAsk),
            "test=local-script" => Some(SelfTest::LocalScript),
            "test=latency" => Some(SelfTest::Latency),
            "test=proc-stop" => Some(SelfTest::ProcStop),
            "test=proc-usage" => Some(SelfTest::ProcUsage),
            "test=shm" => Some(SelfTest::Shm),
            "test=mem" => Some(SelfTest::Mem),
            "test=profile" => Some(SelfTest::Profile),
            "test=many-processes" => Some(SelfTest::ManyProcesses),
            "test=display-claim" => Some(SelfTest::DisplayClaim),
            "test=desk-windows" => Some(SelfTest::DeskWindows),
            "test=desk-restart" => Some(SelfTest::DeskRestart),
            "test=desk-ask" => Some(SelfTest::DeskAsk),
            "test=app-suspend" => Some(SelfTest::AppSuspend),
            "test=event-wake" => Some(SelfTest::EventWake),
            "test=app-on-demand" => Some(SelfTest::AppOnDemand),
            "test=desk-boot" => Some(SelfTest::DeskBoot),
            "test=desk-panel" => Some(SelfTest::DeskPanel),
            "test=desk-panel-shell" => Some(SelfTest::DeskPanelShell),
            "test=desk-catalog" => Some(SelfTest::DeskCatalog),
            "test=ui-window" => Some(SelfTest::UiWindow),
            "test=app-editor" => Some(SelfTest::AppEditor),
            "test=ui-monitor" => Some(SelfTest::UiMonitor),
            "test=desk-nap" => Some(SelfTest::DeskNap),
            "test=desk-appearance" => Some(SelfTest::DeskAppearance),
            "test=keep-running" => Some(SelfTest::KeepRunning {
                reread: cmdline.split_whitespace().any(|w| w == "reread"),
            }),
            "test=desk-card" => Some(SelfTest::DeskCard),
            "test=desk-standing" => Some(SelfTest::DeskStanding),
            "test=desk-standing-refused" => Some(SelfTest::DeskStandingRefused),
            "test=desk-perf" => Some(SelfTest::DeskPerf),
            "test=ui-keys" => Some(SelfTest::UiKeys),
            "test=desk-term" => Some(SelfTest::DeskTerm),
            "test=desk-agent" => Some(SelfTest::DeskAgent),
            "test=desk-default" => Some(SelfTest::DeskDefault),
            "test=desk-no-memory" => Some(SelfTest::DeskNoMemory),
            "test=app-settings" => Some(SelfTest::AppSettings),
            "test=desk-tour" => Some(SelfTest::DeskTour),
            "test=desk-takeover" => Some(SelfTest::DeskTakeover),
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
            "test=panic-reset" => Some(SelfTest::PanicReset),
            "test=efi" => Some(SelfTest::Efi),
            "test=efi-reset" => Some(SelfTest::EfiReset),
            "test=slot" => Some(SelfTest::Slot),
            "test=update" => Some(SelfTest::Update),
            "test=probation" => Some(SelfTest::Probation),
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
            | SelfTest::Slot
            | SelfTest::Update
            | SelfTest::Probation
            | SelfTest::Net
            | SelfTest::Terminal
            | SelfTest::Shell
            | SelfTest::CPrompt
            | SelfTest::Scripts
            | SelfTest::Act(_)
            | SelfTest::Live
            | SelfTest::AppSuspend
            | SelfTest::EventWake
            | SelfTest::AppOnDemand
            | SelfTest::Edit
            | SelfTest::Manifest
            | SelfTest::ManifestGood
            | SelfTest::LocalAsk
            | SelfTest::LocalScript
            | SelfTest::Latency
            | SelfTest::ProcStop
            | SelfTest::ProcUsage
            | SelfTest::Part
            | SelfTest::ModelStore { .. }
            | SelfTest::ManyProcesses
            | SelfTest::Files { .. }
            | SelfTest::AppSaved { .. }
            | SelfTest::FilesRestart
            | SelfTest::FilesDeadClient
            | SelfTest::PkgStore(_)
            | SelfTest::DisplayClaim
            | SelfTest::DeskWindows
            | SelfTest::DeskRestart
            | SelfTest::DeskAsk
            | SelfTest::FilesActions
            | SelfTest::FilesChurn { .. }
            | SelfTest::DeskBoot
            | SelfTest::DeskPanel
            | SelfTest::DeskPanelShell
            | SelfTest::DeskCatalog
            | SelfTest::UiWindow
            | SelfTest::AppEditor
            | SelfTest::UiMonitor
            | SelfTest::DeskNap
            | SelfTest::KeepRunning { .. }
            | SelfTest::DeskAppearance
            | SelfTest::DeskCard
            | SelfTest::DeskStanding
            | SelfTest::DeskStandingRefused
            | SelfTest::DeskPerf
            | SelfTest::UiKeys
            | SelfTest::DeskTerm
            | SelfTest::DeskAgent
            | SelfTest::DeskDefault
            | SelfTest::DeskNoMemory
            | SelfTest::AppSettings
            | SelfTest::DeskTour
            | SelfTest::DeskTakeover
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
    // The v0.1 system (`kabi::plan::system`): what the default boot plans, and
    // what `desk-default` boots unchanged. With a build service (`build=`, the
    // edit loop), the boot manifest's headless `notes`, which `:edit notes` and
    // the editor build, and the edit service follow; without one no user sees
    // `notes`, and it would cost 32 MiB of the admission budget at boot.
    if matches!(
        test,
        SelfTest::None | SelfTest::DeskDefault | SelfTest::DeskNoMemory
    ) {
        let files = crate::block::has_partition(diskmap::KAI_DATA_TYPE);
        let mut plan = kabi::plan::system::plan(&crate::llm::plan_line(), files);
        if let Some(edit) = crate::build_service::plan_line() {
            plan.push('\n');
            plan.push_str(&crate::boot_manifest::plan_line());
            plan.push('\n');
            plan.push_str(&edit);
        }
        return Some(plan);
    }
    // The live, edit and manifest cases need only their services, with probation
    // ended by calls; the live case also needs llm for `:why`, and the others the
    // edit service, which syncs with git.
    let mut plan = match test {
        SelfTest::Live => format!("{}\n{}", kabi::plan::NOTES_TEST, crate::llm::plan_line()),
        SelfTest::Part => String::from(kabi::plan::FS),
        SelfTest::AppSuspend => kabi::plan::notes("2", None),
        // notes is listed before the shell, whose call reaches it through the
        // supervisor, never a route.
        SelfTest::Act(ActCase::AppRoute) => format!(
            "{}
{}",
            crate::llm::plan_line(),
            kabi::plan::notes("2", None)
        ),
        SelfTest::AppSaved { .. } => crate::saved_tests::plan(),
        SelfTest::AppOnDemand => crate::demand_tests::plan(),
        // llm first, so it asks for the model while fs still fills it.
        SelfTest::ModelStore { bad: false } => {
            format!("{}\n{}", crate::llm::plan_line(), kabi::plan::FS)
        }
        SelfTest::ModelStore { bad: true } => String::from(kabi::plan::FS),
        SelfTest::Files { reread } => {
            crate::files_tests::plan(if reread { "read" } else { "write" })
        }
        SelfTest::FilesRestart => crate::files_tests::plan("restart"),
        SelfTest::FilesDeadClient => crate::files_tests::plan("hold"),
        SelfTest::FilesActions => crate::files_tests::plan("actions"),
        SelfTest::FilesChurn { boot, .. } => {
            crate::files_tests::plan(&alloc::format!("churn boot={boot}"))
        }
        SelfTest::PkgStore(step) => crate::pkg_tests::plan(step),
        SelfTest::DeskCatalog => crate::catalog_tests::plan(),
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
            | SelfTest::AppSuspend
            | SelfTest::AppOnDemand
            | SelfTest::Edit
            | SelfTest::Manifest
            | SelfTest::ManifestGood
            | SelfTest::Part
            | SelfTest::AppSaved { .. }
            | SelfTest::ModelStore { .. }
            | SelfTest::Files { .. }
            | SelfTest::FilesRestart
            | SelfTest::FilesDeadClient
            | SelfTest::FilesActions
            | SelfTest::FilesChurn { .. }
            | SelfTest::PkgStore(_)
            | SelfTest::DeskCatalog
    ) {
        // The file system service starts before the shell, once there is a
        // `kai-data` partition for it (ADR 0004).
        // With it, the shell gets `files`, its `Dir` for `/home/user`.
        let shell = match crate::block::has_partition(diskmap::KAI_DATA_TYPE) {
            true => {
                plan.push('\n');
                plan.push_str(kabi::plan::FS);
                kabi::plan::SHELL_FILES
            }
            false => kabi::plan::SHELL,
        };
        // The shell calls `cc` for the `c>` prompt, so cc comes first.
        plan.push('\n');
        plan.push_str(kabi::plan::CC);
        // On the desktop the shell has a route to `desktop`, for the panel:
        // `desktop` is planned first, as a route's target must be.
        let shell = match test {
            SelfTest::DeskPanelShell => {
                plan.push('\n');
                plan.push_str(crate::desk_tests::BOOT_PLAN);
                kabi::plan::SHELL_DESK
            }
            // The app is planned before the shell, whose call reaches it through
            // the supervisor, and after `desktop`, which its window needs.
            SelfTest::Act(ActCase::AppTerminal) => {
                plan.push('\n');
                plan.push_str(crate::app_terminal_tests::PLAN);
                kabi::plan::SHELL_DESK
            }
            SelfTest::DeskAgent | SelfTest::DeskTakeover | SelfTest::DeskStandingRefused => {
                plan.push('\n');
                plan.push_str(crate::agent_desk_tests::DESK);
                kabi::plan::SHELL_DESK
            }
            SelfTest::DeskTour => {
                plan.push('\n');
                plan.push_str(crate::desk_tour_tests::DESKTOP);
                crate::desk_tour_tests::SHELL
            }
            // Settings reads what the shell saved, so its shell has a state
            // directory as well as the desktop.
            SelfTest::AppSettings => {
                plan.push('\n');
                plan.push_str(crate::agent_desk_tests::DESK);
                crate::app_settings_tests::SHELL
            }
            SelfTest::KeepRunning { .. } => {
                plan.push('\n');
                plan.push_str(crate::keep_tests::PLAN);
                crate::keep_tests::SHELL
            }
            // The desktop with its card, before the shell with `files` and
            // `state` (the appearance is saved in `/system/state`) and its
            // route to `desktop`.
            SelfTest::DeskAppearance => {
                plan.push('\n');
                plan.push_str(crate::desk_tests::BOOT_PLAN);
                crate::keep_tests::SHELL
            }
            _ => shell,
        };
        plan.push('\n');
        plan.push_str(shell);
    }
    // `update` runs the update service's check once the shell is up.
    if test == SelfTest::Update {
        // `updated=update`: an update from the update disk in place of the
        // check; in both, a refusal restarts into the running slot.
        let step = crate::cmdline()
            .split_whitespace()
            .find_map(|w| w.strip_prefix("updated="))
            .unwrap_or("check");
        plan.push('\n');
        plan.push_str(kabi::plan::UPDATED);
        plan.push_str(" after=restart step=");
        plan.push_str(step);
        // `updated.<word>`: a word for `updated`'s line, as `restart=window`.
        for word in crate::cmdline()
            .split_whitespace()
            .filter_map(|w| w.strip_prefix("updated."))
        {
            plan.push(' ');
            plan.push_str(word);
        }
    }
    // `act-rule` stops processes the supervisor does not restart, and one
    // service it does.
    if test == SelfTest::Act(ActCase::Rule) {
        plan.push_str("\nspin-a program=spinner burst_ms=3600000 unsupervised\nspin-b program=spinner burst_ms=3600000 unsupervised\nspin-c program=spinner burst_ms=3600000 unsupervised\nspin-d program=spinner burst_ms=3600000");
    }
    // `profile-switch` sets two apps' classes and quotas.
    if test == SelfTest::Act(ActCase::Profile) {
        plan.push_str("\nspin-a program=spinner burst_ms=3600000 unsupervised\nspin-b program=spinner burst_ms=3600000 unsupervised");
    }
    // `act-convo` asks to stop two processes and declines both.
    if test == SelfTest::Act(ActCase::Convo) {
        plan.push_str(
            "
spin-a program=spinner burst_ms=3600000 unsupervised
spin-b program=spinner burst_ms=3600000 unsupervised",
        );
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
    if test == SelfTest::ProcUsage {
        plan.push('\n');
        plan.push_str(crate::usage_tests::PLAN);
    }
    if test == SelfTest::ManyProcesses {
        plan.push_str(&crate::many_tests::plan());
    }
    if test == SelfTest::DisplayClaim {
        plan.push('\n');
        plan.push_str(crate::claim_tests::PLAN);
    }
    if matches!(
        test,
        SelfTest::DeskWindows | SelfTest::DeskRestart | SelfTest::DeskAsk | SelfTest::DeskCard
    ) {
        plan.push('\n');
        plan.push_str(crate::desk_tests::PLAN);
    }
    if matches!(test, SelfTest::DeskBoot | SelfTest::DeskPanel) {
        plan.push('\n');
        plan.push_str(crate::desk_tests::BOOT_PLAN);
    }
    if test == SelfTest::UiWindow {
        plan.push('\n');
        plan.push_str(crate::ui_tests::PLAN);
    }
    if test == SelfTest::AppEditor {
        plan.push('\n');
        plan.push_str(&crate::app_editor_tests::plan());
    }
    if test == SelfTest::UiMonitor {
        plan.push('\n');
        plan.push_str(&crate::app_window_tests::monitor_plan());
    }
    if test == SelfTest::DeskNap {
        plan.push('\n');
        plan.push_str(crate::nap_tests::PLAN);
    }
    if test == SelfTest::DeskStanding {
        plan.push('\n');
        plan.push_str(crate::standing_tests::PLAN);
    }
    if test == SelfTest::DeskStandingRefused {
        plan.push('\n');
        plan.push_str(crate::standing_tests::NOTES);
    }
    if test == SelfTest::DeskPerf {
        plan.push('\n');
        plan.push_str(crate::desk_perf_tests::PLAN);
    }
    if test == SelfTest::UiKeys {
        plan.push('\n');
        plan.push_str(crate::keys_tests::PLAN);
    }
    if test == SelfTest::DeskTerm {
        plan.push('\n');
        plan.push_str(crate::term_tests::PLAN);
    }
    if matches!(test, SelfTest::DeskAgent | SelfTest::DeskTakeover) {
        plan.push('\n');
        plan.push_str(crate::agent_desk_tests::APPS);
    }
    if test == SelfTest::AppSettings {
        plan.push('\n');
        plan.push_str(crate::app_settings_tests::APPS);
    }
    if test == SelfTest::DeskTour {
        plan.push('\n');
        plan.push_str(crate::desk_tour_tests::APPS);
    }
    Some(plan)
}
