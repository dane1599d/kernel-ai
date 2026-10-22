//! Processes: what the supervisor spawns them with, and how they end.

use crate::MAX_NAME;

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

pub const MAX_ARGS: usize = 256;
/// Largest supervisor state the kernel keeps.
pub const MAX_STATE: usize = 4096;

/// Argument of [`crate::call::SPAWN`].
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
    /// Capability bits; only [`crate::cap::GRANTABLE`] ones are accepted.
    pub caps: u64,
    /// Stack size in pages (0 = [`crate::layout::STACK_PAGES`]), counted in `max_pages`.
    pub stack_pages: u64,
}

/// Filled in by [`crate::call::WAIT_CHILD`].
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_reasons_round_trip() {
        for raw in 0..6 {
            assert_eq!(ExitReason::from_raw(raw).map(|r| r as u64), Some(raw));
        }
        assert_eq!(ExitReason::from_raw(6), None);
    }
}
