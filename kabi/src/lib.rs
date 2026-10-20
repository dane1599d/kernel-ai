//! The system-call interface between the kernel and ring-3 programs: call numbers,
//! error codes, capabilities and the structures passed by pointer. Shared by the
//! kernel and the `user` runtime so both sides agree on every number and layout.
//!
//! Calling convention: `syscall` with the number in RAX and arguments in RDI, RSI,
//! RDX, R10, R8 and R9. The result comes back in RAX: a value, or a negative
//! [`Error`] code. RCX and R11 are clobbered.

#![no_std]

extern crate alloc;

pub mod live;
pub mod model;
pub mod plan;
pub mod probe;
pub mod thread;

/// System-call numbers.
pub mod call {
    /// `exit(code: i32) -> !`
    pub const EXIT: u64 = 0;
    /// `write(ptr, len) -> len`: text to the console, prefixed with the program name.
    pub const WRITE: u64 = 1;
    /// `sleep_ms(ms)`
    pub const SLEEP: u64 = 2;
    /// `yield()`
    pub const YIELD: u64 = 3;
    /// `brk(grow_bytes) -> heap end`: grows the heap by whole pages; 0 just reports.
    pub const BRK: u64 = 4;
    /// `time_ns() -> nanoseconds since boot`
    pub const TIME: u64 = 5;
    /// `args(buf, cap) -> len`: the argument string the program was started with.
    pub const ARGS: u64 = 6;
    /// `spawn(&SpawnRequest) -> pid` (needs [`super::cap::SUPERVISE`])
    pub const SPAWN: u64 = 7;
    /// `wait_child(&mut ExitRecord, timeout_ms) -> 1 if a record was written, 0 on timeout`
    pub const WAIT_CHILD: u64 = 8;
    /// `kill(pid)`
    pub const KILL: u64 = 9;
    /// `state_save(ptr, len)`: replaces the kernel-held supervisor state.
    pub const STATE_SAVE: u64 = 10;
    /// `state_load(buf, cap) -> len`
    pub const STATE_LOAD: u64 = 11;
    /// `plan(buf, cap) -> len`: the services to run, one `name args...` per line.
    pub const PLAN: u64 = 12;
    /// `svc_recv(buf, cap, timeout_ms, &mut caller_pid) -> len`: the next request for
    /// this service, and who sent it (0 = the kernel) (needs [`super::cap::SERVE`]).
    pub const SVC_RECV: u64 = 13;
    /// `svc_reply(ptr, len)`: answers the request taken by the last `svc_recv`.
    pub const SVC_REPLY: u64 = 14;
    /// `http(&HttpRequest) -> response len`: sends the request bytes to the LLM
    /// server and returns what it sends back before closing the connection
    /// (needs [`super::cap::NET`]).
    pub const HTTP: u64 = 15;
    /// `list(buf, cap) -> len`: running services, one `pid name` per line, so a
    /// restarted supervisor can find the services it already started.
    pub const LIST: u64 = 16;
    /// `call(name_ptr, name_len, &CallRequest) -> reply len`: sends a request to
    /// another service and waits for its reply (needs [`super::cap::CALL`]). A
    /// process may not call itself, a service that can itself call, nor the edit
    /// service, which only the kernel drives.
    pub const CALL: u64 = 17;
    /// `metrics(&mut Metrics) -> 0`: scheduler, memory and process numbers.
    pub const METRICS: u64 = 18;
    /// `policy(id)`: installs scheduling policy [`super::policy`] `id` on every CPU
    /// (needs [`super::cap::POLICY`]).
    pub const POLICY: u64 = 19;
    /// `interrupted() -> 1 or 0`: whether the user pressed Ctrl-C since the last
    /// call; reading clears it.
    pub const INTERRUPTED: u64 = 20;
    /// `comp_fetch(label_ptr, label_len, buf, cap) -> len`: copies version `label` of
    /// the caller's component into `buf` when it fits; the length either way.
    /// Needs [`cap::COMPONENT`].
    pub const COMP_FETCH: u64 = 21;
    /// `comp_event(&live::Event) -> 0`: reports a ledger event; the kernel checks
    /// it against the ledger rules and refuses what they do not allow.
    pub const COMP_EVENT: u64 = 22;
    /// `comp_restore(&mut live::Restore, buf, cap) -> len`: the ledger, and the
    /// active version's checkpoint copied into `buf` when it fits.
    pub const COMP_RESTORE: u64 = 23;
    /// `comp_put(&live::Put) -> 0`: uploads a version of component `name` into the
    /// store, under the label the store assigns next (`e1`, `e2`, ...), with the
    /// label it was built from and the git commit holding its source. The kernel
    /// checks the artifact against the upload rules (see [`crate::live::MAX_UPLOAD`]).
    /// Needs [`super::cap::PUBLISH`].
    pub const COMP_PUT: u64 = 24;
    /// `comp_control(&live::Control) -> reply len`: sends `ctl <request>` to a live
    /// component as the kernel (caller 0), waiting at most 60 s. Only `status`,
    /// `source [file]` and `try <version>` are relayed (see
    /// [`crate::live::relayable`]); anything else is Invalid, and a name that is
    /// not a live component's host is NotFound. Needs [`super::cap::PUBLISH`].
    pub const COMP_CONTROL: u64 = 25;
    /// `comp_read(&live::Read) -> len`: a component's [`crate::live::Info`], or the bytes
    /// of one of its versions copied into the buffer when they fit (the length
    /// either way). A read is not a fetch: it gives no right to report events.
    /// Needs [`super::cap::PUBLISH`].
    pub const COMP_READ: u64 = 26;
    /// `build(&HttpRequest) -> response len`: as [`HTTP`], but to the build service
    /// named by `build=` on the kernel command line, never to the LLM server.
    /// Needs [`super::cap::PUBLISH`].
    pub const BUILD: u64 = 27;
    /// `model_map() -> len`: maps the model the kernel loaded from the `kai-model`
    /// disk at [`super::layout::MODEL_BASE`], read-only and not executable, by
    /// linking the kernel's window of 2 MiB pages, which takes nothing from the
    /// page quota; returns its length in bytes, and calling it again maps nothing
    /// new. NotFound when no model is loaded, Invalid if the window cannot be
    /// linked. Needs [`super::cap::MODEL`].
    pub const MODEL_MAP: u64 = 28;
    /// `thread_spawn(entry, arg, cpu, stack_pages) -> index`: starts a worker of this
    /// process at `entry` with `arg` in RDI, pinned to CPU `cpu` (below the CPUs
    /// online), on a stack of `stack_pages` pages (see [`super::thread`]). Only the
    /// main thread of a holder of [`super::cap::THREADS`] may. Busy when it already
    /// runs [`super::thread::MAX_WORKERS`] workers (for a moment also after one
    /// exits) or the kernel's threads run out; NoMemory past the page quota.
    pub const THREAD_SPAWN: u64 = 29;
    /// `thread_exit() -> !`: ends the calling worker. Invalid from the main thread,
    /// which ends the whole process with [`EXIT`].
    pub const THREAD_EXIT: u64 = 30;
    /// `futex_wait(addr, expected, timeout_ms) -> 0 or 1`: sleeps while the aligned
    /// 32-bit word at `addr` in this process holds `expected`, until a
    /// [`FUTEX_WAKE`] for it (0), `timeout_ms` (0, or more than an hour, means
    /// none; TimedOut) or a kill (Dead); 1 at once when the word holds something else.
    pub const FUTEX_WAIT: u64 = 31;
    /// `futex_wake(addr, count) -> woken`: wakes up to `count` threads of this
    /// process waiting on the word at `addr`.
    pub const FUTEX_WAKE: u64 = 32;
}

/// Errors, returned in RAX as their negative value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i64)]
pub enum Error {
    /// The process lacks the capability for this call.
    Permission = 1,
    /// A pointer argument is not mapped, not user memory, or not writable.
    Fault = 2,
    Invalid = 3,
    /// The page quota or physical memory is exhausted.
    NoMemory = 4,
    TimedOut = 5,
    /// No such process, service or call.
    NotFound = 6,
    Busy = 7,
    Network = 8,
    TooBig = 9,
    /// The other side died before answering.
    Dead = 10,
    /// The user pressed Ctrl-C.
    Interrupted = 11,
    /// The service is not running.
    Unavailable = 12,
}

impl Error {
    pub const ALL: [Error; 12] = [
        Error::Permission,
        Error::Fault,
        Error::Invalid,
        Error::NoMemory,
        Error::TimedOut,
        Error::NotFound,
        Error::Busy,
        Error::Network,
        Error::TooBig,
        Error::Dead,
        Error::Interrupted,
        Error::Unavailable,
    ];

    /// The value placed in RAX.
    pub const fn to_raw(self) -> u64 {
        (-(self as i64)) as u64
    }

    /// Splits a raw result into a value or an error.
    pub fn from_raw(raw: u64) -> Result<u64, Error> {
        let signed = raw as i64;
        if signed >= 0 {
            return Ok(raw);
        }
        let known = Self::ALL
            .into_iter()
            .find(|error| (*error as i64).checked_neg() == Some(signed));
        Err(known.unwrap_or(Error::Invalid))
    }
}

/// Capabilities, a coarse bit set fixed when a process is spawned.
pub mod cap {
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
    /// Every capability a supervisor may grant.
    pub const GRANTABLE: u64 = SERVE | NET | CALL | POLICY | COMPONENT | PUBLISH | MODEL | THREADS;
}

/// Why a process ended, as reported to the supervisor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub enum ExitReason {
    /// Called `exit`; `detail` is the exit code.
    Exited = 0,
    /// A CPU exception in ring 3; `detail` is the vector.
    Crashed = 1,
    /// Ran longer than its CPU burst limit without blocking.
    Runaway = 2,
    /// Asked for more memory than its quota allows.
    OutOfMemory = 3,
    /// Killed by the supervisor or the kernel.
    Killed = 4,
    /// The program could not be loaded (a bad ELF, or no memory for it).
    LoadFailed = 5,
}

impl ExitReason {
    pub fn from_raw(raw: u64) -> Option<Self> {
        [
            Self::Exited,
            Self::Crashed,
            Self::Runaway,
            Self::OutOfMemory,
            Self::Killed,
            Self::LoadFailed,
        ]
        .into_iter()
        .find(|reason| *reason as u64 == raw)
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Exited => "exited",
            Self::Crashed => "crashed",
            Self::Runaway => "runaway",
            Self::OutOfMemory => "out of memory",
            Self::Killed => "killed",
            Self::LoadFailed => "failed to load",
        }
    }
}

pub const MAX_NAME: usize = 24;
pub const MAX_ARGS: usize = 256;
/// Largest supervisor state the kernel keeps.
pub const MAX_STATE: usize = 4096;
/// Largest request or reply passed through a service endpoint.
pub const MAX_MESSAGE: usize = 8192;

/// Argument of [`call::SPAWN`].
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct SpawnRequest {
    /// The service: it names the process and its endpoint.
    pub name_ptr: u64,
    pub name_len: u64,
    pub args_ptr: u64,
    pub args_len: u64,
    pub limits: Limits,
    /// The program to run; length 0 means the program named like the service.
    pub program_ptr: u64,
    pub program_len: u64,
}

/// Per-process limits, set by the supervisor at spawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct Limits {
    /// Most user pages (heap, stack and program image together).
    pub max_pages: u64,
    /// Most CPU time, in milliseconds, between two blocking calls.
    pub max_burst_ms: u64,
    /// Capability bits; only [`cap::GRANTABLE`] ones are accepted.
    pub caps: u64,
    /// Stack size in pages (0 = [`layout::STACK_PAGES`]), counted in `max_pages`.
    pub stack_pages: u64,
}

/// Filled in by [`call::WAIT_CHILD`].
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct ExitRecord {
    pub pid: u64,
    /// An [`ExitReason`].
    pub reason: u64,
    /// Exit code, exception vector or the limit that was exceeded.
    pub detail: u64,
    pub name_len: u64,
    pub name: [u8; MAX_NAME],
}

impl ExitRecord {
    pub const fn empty() -> Self {
        Self {
            pid: 0,
            reason: 0,
            detail: 0,
            name_len: 0,
            name: [0; MAX_NAME],
        }
    }

    pub fn name(&self) -> &str {
        let len = (self.name_len as usize).min(MAX_NAME);
        core::str::from_utf8(&self.name[..len]).unwrap_or("?")
    }
}

/// Argument of [`call::HTTP`]. The kernel sends the request to the LLM server named
/// on its command line and nowhere else: ring 3 never chooses the destination.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct HttpRequest {
    pub request_ptr: u64,
    pub request_len: u64,
    pub response_ptr: u64,
    pub response_cap: u64,
    pub timeout_ms: u64,
}

/// Fixed places in every user address space.
pub mod layout {
    /// Programs are linked here.
    pub const IMAGE_BASE: u64 = 0x0040_0000;
    /// The heap grows up from here.
    pub const HEAP_BASE: u64 = 0x0000_0010_0000_0000;
    /// Most the heap may grow, whatever the page quota.
    pub const HEAP_MAX: u64 = 0x0000_0001_0000_0000;
    /// The stack grows down from here; the page below the stack stays unmapped.
    pub const STACK_TOP: u64 = 0x0000_7FFF_FFFF_0000;
    /// Default stack size.
    pub const STACK_PAGES: u64 = 16;
    /// Largest stack a process may ask for.
    pub const MAX_STACK_PAGES: u64 = 1024;
    /// The model is mapped here, read-only: the start of a PML4 slot of its own,
    /// so its page tables hold nothing else.
    pub const MODEL_BASE: u64 = 0x0000_2000_0000_0000;
    /// Largest model that fits the window: one PML4 slot, 512 GiB.
    pub const MODEL_MAX: u64 = 0x0000_0080_0000_0000;
    /// First address that is not user space.
    pub const USER_END: u64 = 0x0000_8000_0000_0000;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_round_trip_through_rax() {
        for error in Error::ALL {
            assert_eq!(Error::from_raw(error.to_raw()), Err(error));
        }
        assert_eq!(Error::from_raw(42), Ok(42));
        assert_eq!(Error::from_raw(u64::MAX - 999), Err(Error::Invalid));
    }

    #[test]
    fn exit_reasons_round_trip() {
        for raw in 0..6 {
            assert_eq!(ExitReason::from_raw(raw).map(|r| r as u64), Some(raw));
        }
        assert_eq!(ExitReason::from_raw(6), None);
    }
}

// The fixed layout must leave the image, heap and stack apart, inside user space,
// and the structures passed by pointer must keep their sizes.
const _: () = {
    use layout::*;
    assert!(IMAGE_BASE < HEAP_BASE && HEAP_BASE + HEAP_MAX < STACK_TOP - STACK_PAGES * 4096);
    assert!(STACK_TOP < USER_END);
    assert!(core::mem::size_of::<ExitRecord>() == 32 + MAX_NAME);
    assert!(core::mem::size_of::<SpawnRequest>() == 10 * 8);
    assert!(core::mem::size_of::<CallRequest>() == 5 * 8);
    assert!(core::mem::size_of::<Metrics>() == 13 * 8 + MAX_METRIC_PROCESSES * (16 + MAX_NAME));
    assert!(HEAP_BASE + HEAP_MAX < STACK_TOP - MAX_STACK_PAGES * 4096);
    assert!(HEAP_BASE + HEAP_MAX <= MODEL_BASE && MODEL_BASE.is_multiple_of(MODEL_MAX));
    assert!(MODEL_MAX == 1 << 39 && MODEL_BASE + MODEL_MAX <= STACK_TOP - MAX_STACK_PAGES * 4096);
    assert!(core::mem::size_of::<HttpRequest>() == 5 * 8);
};

/// Argument of [`call::CALL`].
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct CallRequest {
    pub request_ptr: u64,
    pub request_len: u64,
    pub reply_ptr: u64,
    pub reply_cap: u64,
    pub timeout_ms: u64,
}

/// Scheduling policies for [`call::POLICY`].
pub mod policy {
    pub const INTERACTIVE: u64 = 0;
    pub const ROUND_ROBIN: u64 = 1;
}

pub const MAX_METRIC_PROCESSES: usize = 16;

/// One running process in [`Metrics`].
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct ProcessInfo {
    pub pid: u64,
    pub name_len: u64,
    pub name: [u8; MAX_NAME],
}

impl ProcessInfo {
    pub const fn empty() -> Self {
        Self {
            pid: 0,
            name_len: 0,
            name: [0; MAX_NAME],
        }
    }

    pub fn name(&self) -> &str {
        let len = (self.name_len as usize).min(MAX_NAME);
        core::str::from_utf8(&self.name[..len]).unwrap_or("?")
    }
}

/// Filled in by [`call::METRICS`]: a snapshot at most a few milliseconds old.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct Metrics {
    pub cpus: u64,
    /// The installed policy on CPU 0, a [`policy`] id.
    pub policy: u64,
    /// The policy actually running on CPU 0 (the watchdog may have it on fallback).
    pub running_policy: u64,
    pub context_switches: u64,
    pub ejections: u64,
    pub migrations: u64,
    pub free_frames: u64,
    pub total_frames: u64,
    pub heap_allocations: u64,
    pub heap_bytes: u64,
    pub uptime_ns: u64,
    /// Entries of `processes` in use (the supervisor is not listed).
    pub process_count: u64,
    /// Processes left out because the list is full.
    pub processes_omitted: u64,
    pub processes: [ProcessInfo; MAX_METRIC_PROCESSES],
}

impl Metrics {
    pub const fn empty() -> Self {
        Self {
            cpus: 0,
            policy: 0,
            running_policy: 0,
            context_switches: 0,
            ejections: 0,
            migrations: 0,
            free_frames: 0,
            total_frames: 0,
            heap_allocations: 0,
            heap_bytes: 0,
            uptime_ns: 0,
            process_count: 0,
            processes_omitted: 0,
            processes: [ProcessInfo::empty(); MAX_METRIC_PROCESSES],
        }
    }
}
