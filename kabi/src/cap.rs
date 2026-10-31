//! Capabilities, a coarse bit set fixed when a process is spawned.

/// Spawn, wait for and kill services; keep the supervisor state. The kernel
/// gives it only to the supervisor; it can never be passed on.
pub const SUPERVISE: u64 = 1 << 0;
/// Serve requests sent to the service named like the process.
pub const SERVE: u64 = 1 << 1;
/// Make HTTP requests.
pub const NET: u64 = 1 << 2;
/// Call other services.
pub const CALL: u64 = 1 << 3;
/// Change the scheduling policy.
pub const POLICY: u64 = 1 << 4;
/// Fetch versions of the component named like the process and report its ledger
/// events. The kernel grants it only to processes running the `comphost` program.
pub const COMPONENT: u64 = 1 << 5;
/// Upload versions into the store, read any component's versions, and send to
/// the build service. The kernel grants it only to the service named `edit`
/// running the `edit` program, and only the kernel may call a holder.
pub const PUBLISH: u64 = 1 << 6;
/// Map the model read-only. The kernel grants it only to the service named
/// `llm` running the `llm` program.
pub const MODEL: u64 = 1 << 7;
/// Start worker threads (L6). The kernel grants it only to the service named
/// `llm` running the `llm` program.
pub const THREADS: u64 = 1 << 8;
/// Look up host names through [`super::call::RESOLVE`] (ADR 0001). Apart from
/// `NET`, so a process that may make HTTP requests to its one server cannot
/// carry data out in the names it looks up (`<data>.attacker.example`).
pub const RESOLVE: u64 = 1 << 9;
/// Stop processes the supervisor does not run, by name ([`super::call::PROC_STOP`]):
/// a Job handle with STOP. The kernel grants it only to the service named `shell`.
pub const STOP: u64 = 1 << 10;
/// Start `scriptrun` as one's own child and wait for it ([`super::call::JOB_SPAWN`],
/// [`super::call::JOB_WAIT`]): a Job handle with CREATE and WAIT that names that
/// program. The kernel grants it only to the service named `shell` (ADR 0001 D6).
pub const SCRIPT: u64 = 1 << 11;
/// Read and write the `kai-data` partition ([`super::call::PART_READ`]): a
/// `Partition` handle with READ and WRITE. The kernel grants it only to the
/// service named `fs` running the `fs` program (ADR 0004 §7).
pub const PARTITION: u64 = 1 << 12;
/// Files under `/home/user` (ADR 0004 §6-7): a `Dir` handle on the `fs` service
/// for that folder, from which it opens what lies beneath; may hold `Dir` and
/// `File` handles `fs` hands out.
pub const FILES: u64 = 1 << 13;
/// `/system/state` (P2 task 12): a `Dir` handle on the `fs` service for that
/// folder only, where the shell keeps its log, grants and settings. The kernel
/// grants it only to the service named `shell` running the `shell` program.
///
/// STARTUP lists FILES's `Dir` before SYSTEM_STATE's, so a process holding both
/// tells them apart by order: the first `Dir` is `/home/user`, the second
/// `/system/state`. HANDLE_INFO carries no badge.
pub const SYSTEM_STATE: u64 = 1 << 14;
/// Own the screen ([`super::call::DISPLAY_CLAIM`]): a `Display` handle. The
/// kernel grants it only to the service named `desktop` running the program
/// `desktop` (ADR 0005 §2). Numbered at merge, after P2's PARTITION, FILES and
/// SYSTEM_STATE.
pub const DISPLAY: u64 = 1 << 15;
/// Read the kernel's input queue ([`super::call::INPUT_READ`]): an `Input`
/// handle. Granted as [`DISPLAY`] is.
pub const INPUT: u64 = 1 << 16;
/// Read the kernel console's rows ([`super::call::TERM_READ`]): a `Term`
/// handle. The kernel grants it only to the service named `term` running the
/// program `term` (P3 task 8).
pub const TERM: u64 = 1 << 17;
/// Write the inactive slot and the boot record ([`super::call::SLOT_WRITE`],
/// [`super::call::BOOTREC_SET`]): an `Update` handle with READ and WRITE. The
/// kernel grants it only to the service named `updated` running the program
/// `updated` (ADR 0008 §10). Numbered at merge, after the terminal window's
/// `TERM` (1 << 17, #252).
pub const UPDATE: u64 = 1 << 18;
/// Answer asks on `desktop`'s card ([`super::call::ASK_READ`],
/// [`super::call::ASK_ANSWER`]): an `Ask` handle. The kernel grants it only to
/// the service named `desktop` running the program `desktop` (ADR 0005 §7);
/// nothing else can answer an ask, and only the kernel mints the `ONCE`.
/// Numbered at merge, after `UPDATE` (1 << 18, #268).
pub const ASK: u64 = 1 << 19;
/// Send the edit service an open or an edit command for an app that holds the
/// `edit` grant ([`super::call::EDIT_CALL`]): an `Edit` handle with CALL. The
/// kernel grants it only to the service named `comphost` running the `comphost`
/// program (P4 task 11). Numbered at merge, after `ASK` (1 << 19).
pub const EDIT: u64 = 1 << 20;
/// Every capability a supervisor may grant.
pub const GRANTABLE: u64 = SERVE
    | NET
    | CALL
    | POLICY
    | COMPONENT
    | PUBLISH
    | MODEL
    | THREADS
    | RESOLVE
    | STOP
    | SCRIPT
    | PARTITION
    | FILES
    | SYSTEM_STATE
    | DISPLAY
    | INPUT
    | TERM
    | UPDATE
    | ASK
    | EDIT;
