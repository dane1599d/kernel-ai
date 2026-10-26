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
/// Every capability a supervisor may grant.
pub const GRANTABLE: u64 =
    SERVE | NET | CALL | POLICY | COMPONENT | PUBLISH | MODEL | THREADS | RESOLVE | STOP;
