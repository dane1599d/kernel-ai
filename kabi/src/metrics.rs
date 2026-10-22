//! What [`crate::call::METRICS`] fills in.

use crate::MAX_NAME;

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

/// Filled in by [`crate::call::METRICS`]: a snapshot at most a few milliseconds old.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct Metrics {
    pub cpus: u64,
    /// The installed policy on CPU 0, a [`crate::policy`] id.
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
