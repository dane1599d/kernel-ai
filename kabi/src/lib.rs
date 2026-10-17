//! The system-call interface between the kernel and ring-3 programs: call numbers,
//! error codes, capabilities and the structures passed by pointer. Shared by the
//! kernel and the `user` runtime so both sides agree on every number and layout.
//!
//! Calling convention: `syscall` with the number in RAX and arguments in RDI, RSI,
//! RDX, R10, R8 and R9. The result comes back in RAX: a value, or a negative
//! [`Error`] code. RCX and R11 are clobbered.

#![no_std]

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
    /// `svc_recv(buf, cap, timeout_ms) -> len`: the next request for this service
    /// (needs [`super::cap::SERVE`]).
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
}

impl Error {
    pub const ALL: [Error; 10] = [
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
    /// Every capability a supervisor may grant.
    pub const GRANTABLE: u64 = SERVE | NET;
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
    pub name_ptr: u64,
    pub name_len: u64,
    pub args_ptr: u64,
    pub args_len: u64,
    pub limits: Limits,
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
    pub const STACK_PAGES: u64 = 16;
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

/// The isolation probe: a test program that, on its Nth start, tries operation
/// `N % OPS.len()`, each of which ring 3 must not be allowed to do. The kernel's
/// test checks each attempt was stopped by one of the listed exception vectors.
pub mod probe {
    /// (operation, vectors that may stop it).
    pub const OPS: [(&str, &[u64]); 17] = [
        ("x87 instruction", &[7]),
        ("SSE instruction", &[6]),
        ("AVX instruction", &[6]),
        ("cli", &[13]),
        ("hlt", &[13]),
        ("in from a port", &[13]),
        ("rdmsr", &[13]),
        ("write cr3", &[13]),
        // Stopped only when UMIP is on; otherwise the probe exits normally.
        ("sgdt", &[13]),
        ("swapgs", &[13]),
        ("int 0x80", &[11, 13]),
        ("int3", &[13]),
        // #GP on Intel (SYSENTER_CS is 0), #UD on AMD in long mode.
        ("sysenter", &[6, 13]),
        ("write own code", &[14]),
        ("run own data", &[14]),
        ("read kernel memory", &[14]),
        ("syscall with a kernel stack pointer", &[13]),
    ];
    pub const SGDT: usize = 8;
}

// The fixed layout must leave the image, heap and stack apart, inside user space,
// and the structures passed by pointer must keep their sizes.
const _: () = {
    use layout::*;
    assert!(IMAGE_BASE < HEAP_BASE && HEAP_BASE + HEAP_MAX < STACK_TOP - STACK_PAGES * 4096);
    assert!(STACK_TOP < USER_END);
    assert!(core::mem::size_of::<ExitRecord>() == 32 + MAX_NAME);
    assert!(core::mem::size_of::<SpawnRequest>() == 7 * 8);
    assert!(core::mem::size_of::<HttpRequest>() == 5 * 8);
};
