//! Which self-test the kernel command line selects, and the service plan each boot
//! starts with.

mod cmdline;
mod plan;
mod updated;

pub use cmdline::from_cmdline;
pub use plan::plan;

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
    /// `act-stop` (#887): the person's Stop while llm answers stops the line.
    Stop,
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
    /// `app-files` (P4 task 5): the agent changes files, the Files window shows
    /// each state, and `undo` puts them back.
    AppFiles,
    /// `app-monitor` (P4 task 9): the agent sorts the Monitor by memory, and
    /// stopping a process asks.
    AppMonitor,
    /// `app-models` (P4 task 10): the agent lists, imports and uses a model,
    /// and `undo` puts the one before back.
    AppModels,
    /// `profile-switch` (P3 task 12): profiles through the supervisor.
    Profile,
    /// `net-pf` (P5 task 11): read content goes out only to a host the line
    /// names; any other asks and names the source.
    NetPf,
    /// `net-pf-page` (P5 task 11): a page fetched through `fetch` from a host
    /// the line typed is shown, and what it says starts nothing.
    NetPfPage,
    /// `net-pf-query` (P5 task 11): a quarantined read of that page, whose
    /// answer obeys the page and is only shown.
    NetPfQuery,
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
    /// `c-build` (P4 task 13): `cc build` installs a C app from `local`, and
    /// `undo` switches back.
    CBuild,
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
    /// `desk-wheel` (Kai Desk PR 9): the wheel moves Start's All apps list.
    DeskWheel,
    /// `desk-panel-shell` (P3 task 7): the shell takes the panel's line.
    DeskPanelShell,
    /// `desk-card-wake` (Kai builds apps, follow-up 2): the terminal's prompt
    /// wakes for the shell's waiting card, and waits while another ask is up.
    DeskCardWake,
    /// `desk-catalog` (P4 task 3): the Apps sheet lists `pkg`'s catalogue.
    DeskCatalog,
    /// An app's kai:ui window on the desktop, closed from its control (P3 task 9).
    UiWindow,
    /// A closed app's window stays closed until asked, then comes back
    /// (backlog item 2).
    UiWindowShow,
    /// The editor app's window on the desktop, opened by the agent's door, typed
    /// into and built in the background (P4 task 11).
    AppEditor,
    /// The editor opens a file of the person's own, saves it through the file door and
    /// the shell, and `undo` puts it back (P4 task 11).
    AppEditorFiles,
    /// Ctrl-B in the editor builds the C component whose folder the open file is in,
    /// through the file door and the shell, and `undo` takes it back (P4 task 11).
    AppEditorBuild,
    /// The System monitor's window on the desktop, given a sample of the machine
    /// and closed from its control (P4 task 9).
    UiMonitor,
    /// The Files window on the desktop, shown and closed from its control (P4 task 5).
    UiFiles,
    /// The model manager's window on the desktop, given `updated`'s model list
    /// and closed from its control (P4 task 10).
    UiModels,
    /// A person's keys in the Models window switch the model through the
    /// shell's `model_use` row, and `undo` switches it back (#556).
    UiModelsUse,
    /// An unused app naps and a click wakes it (P3 task 12b).
    DeskNap,
    /// A window in use never naps; one that lost the focus does (backlog
    /// item 1).
    DeskNapBusy,
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
    /// An allowed standing instruction taken back by `undo`, then by
    /// `rules_remove`: its grant gone, and its event fires nothing.
    DeskStandingUndo,
    /// A send's desk card, its text read through in the Kai pane, its hold and undo.
    DeskSends,
    /// S05-1's budgets timed on the real `desktop` (P3's exit case).
    DeskPerf,
    /// A profile applied exactly, switched by a focus change, undone (P3's exit case).
    DeskProfile,
    /// Every resource-profile floor refused by the kernel (P3's exit case).
    DeskProfileFloor,
    /// The keyboard reaches an app's widgets (key input for uikit apps).
    /// `busy`: while Notes is called every 5 ms (comphost's poll under load).
    UiKeys {
        busy: bool,
    },
    DeskTerm,
    /// `app-notes` (P4): Kai makes notes in Notes' window, they are saved, and
    /// `undo` takes one back; boot 2 (`reread`) finds them on disk.
    AppNotes {
        reread: bool,
    },
    /// `desk-agent` (P3 task 10): the agent opens Notes and acts inside it.
    DeskAgent,
    /// The v0.1 system as the default boot plans it: the desktop, the Apps
    /// sheet's catalogue, and an app opened (`desk-default`).
    DeskDefault,
    /// The same system with no memory left for an app: opening it says so
    /// (`desk-no-memory`).
    DeskNoMemory,
    /// The same system while llm says it is not warm for `llm.cold_ms`: the
    /// panel says "Kai is getting ready" until it is (`desk-warming`, founder
    /// test 2).
    DeskWarming,
    /// The Settings app on the desktop, its window kept from the agent (P4 task 7).
    AppSettings,
    /// `test=desk-tour`: screenshots of the desktop and its apps, not a gate.
    DeskTour,
    /// `test=founder-walk` (founder test 2): the founder's VirtualBox walk on
    /// the default system, as keys and clicks.
    FounderWalk,
    /// `test=pane-states`: the Kai pane through each of its states, from
    /// lines typed into it, on the default system.
    PaneStates,
    /// `desk-takeover` (P3 task 10): a key in the agent's window stops it.
    DeskTakeover,
    /// `desk-reopen` (#922): Notes closed from its control opens again.
    DeskReopen,
    /// `desk-start` (Kai Desk PR 8): a Super tap, "not" and Enter open Notes
    /// from Start.
    DeskStart,
    /// `desk-start-esc`: the same with Ctrl+Esc.
    DeskStartEsc,
    /// `scriptrun` through a Job, and spike S01-1's M3 and M5 (P1 task 14).
    Scriptrun,
    /// L2: processes keep their vector state across switches and CPUs, and start clean.
    Fpu,
    /// L2: the isolation probe; ops 0-2 complete and every other op is stopped.
    Probe,
    /// P1: handle tables, startup handles and need() on handles.
    Handles,
    /// IPC round trips timed: 0 B, 1 KiB and 7 KiB, kernel to ring 3 and ring 3
    /// to ring 3.
    IpcRtt,
    /// SHELL_INTERRUPT: the person's Stop ends the shell's call in flight.
    ShellInterrupt,
    /// CALL_SUBMIT and CALL_POLL: a process serves while its own call is in flight.
    AsyncCall,
    /// L6: worker threads start, are refused, filtered and torn down.
    Threads,
    /// L6: processes killed mid-run, every thread blocked or computing, free every frame.
    ThreadsKill,
    /// THREAD_PIN: a pinned thread stays, an unpinned one migrates.
    ThreadPin,
    /// L6: a clock jump past a runaway's limit is not run time.
    RunawayStall,
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
    /// POWER_REBOOT from ring 3, refused without `cap::POWER` (backlog item 13).
    PowerReboot,
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
    /// P5 task 1: what a `NetDest` handle allows.
    NetDest,
    /// P5 task 2: TCP streams on a `NetDest`.
    NetStream,
    /// P5 task 12's harness: xtask's HTTPS mock reached from ring 3.
    HttpsMock,
    /// P5 task 6, the roadmap's P5 exit: a download cut and resumed, across a
    /// reboot (`again`), and bad certificates and hashes refused; `small`
    /// under TCG (`accel=tcg`).
    NetDownload {
        again: bool,
        small: bool,
    },
    /// P5 task 12: the `fetch` service on its caller's handles.
    FetchService,
    /// P5 task 13: an app's HTTPS through `kai:net`, asked per host on the card.
    NetGrant,
    /// The founder's test 2, item 7: an app's reads answered while the model
    /// thinks, its changes held until the line ends.
    WaitReads,
    /// P5 task 3: ENTROPY's sample, which two boots must not share.
    EntropyBoots,
}
