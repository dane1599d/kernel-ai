//! Handles (ADR 0001 D1): a process holds one for each thing it may use, and the
//! kernel checks it on every call. A handle can be narrowed or passed on, never
//! widened. P1 task 5 brings the table, the startup handles a process gets from
//! its capability bits, and the calls that read and narrow them; the grant tree
//! and handle IPC follow in later tasks.

use bitflags::bitflags;

/// The ABI version (ADR 0001 D10): additions bump the minor, a removal or a
/// changed meaning bumps the major. Every in-tree ELF carries it in a "KAI" note,
/// and the loader refuses another major or a newer minor.
pub const ABI: (u32, u32) = (1, 0);

/// The name of the ELF note that carries [`ABI`], and its type.
pub const NOTE_NAME: &[u8; 4] = b"KAI\0";
pub const NOTE_TYPE: u32 = 1;

/// Entries a table starts with, and the most it grows to.
pub const TABLE_START: usize = 256;
pub const TABLE_MAX: usize = 4096;
/// Bits of a handle that index the table; the rest are the slot's generation.
pub const INDEX_BITS: u32 = 12;
/// Startup handles a process gets at most, one or two per capability bit.
pub const MAX_STARTUP: usize = 16;

/// A handle as ring 3 holds it. 0 is never valid. Bits 0-11 are the table index
/// and bits 12-31 the slot's generation, so a closed handle stays dead even after
/// its slot is reused.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Handle(pub u32);

impl Handle {
    pub const NONE: Handle = Handle(0);

    /// The handle for table entry `index` at `generation` (1 or more).
    pub const fn new(index: usize, generation: u32) -> Handle {
        Handle((generation << INDEX_BITS) | index as u32)
    }

    pub const fn index(self) -> usize {
        (self.0 & ((1 << INDEX_BITS) - 1)) as usize
    }

    pub const fn generation(self) -> u32 {
        self.0 >> INDEX_BITS
    }
}

/// What a handle names (ADR 0001 D2). The numbers are ABI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u32)]
pub enum Kind {
    /// One process: its own, for THREADS.
    Process = 1,
    /// The right to start and stop processes.
    Job = 2,
    /// A server's receive side, for its own service name.
    Endpoint = 3,
    /// A client route to a service.
    Service = 4,
    /// A node in the grant tree.
    Grant = 5,
    /// The scheduler policy.
    Sched = 6,
    /// The model window.
    Model = 7,
    /// One live component's versions and ledger.
    Component = 8,
    /// The component store.
    Store = 9,
    /// The build service.
    Build = 10,
    /// A fixed network destination.
    NetDest = 11,
    /// Shared pages.
    Shm = 12,
    /// One disk partition, `kai-data` ([`crate::part`]): the `fs` service's only.
    Partition = 13,
    /// A folder, served by `fs`; badge (volume, object id) ([`crate::fs`]).
    Dir = 14,
    /// A file, served by `fs`; badge (volume, object id).
    File = 15,
    /// The screen ([`crate::display`]): only `desktop`'s. Numbered at merge,
    /// after P2's Partition (13) and the fs service's Dir and File (14, 15).
    Display = 16,
    /// The kernel's input queue: only `desktop`'s.
    Input = 17,
    /// The slots and the boot record ([`crate::update`]): only `updated`'s.
    /// Numbered at merge, after the terminal window's `Term` (18, #252).
    Update = 19,
}

impl Kind {
    pub const ALL: [Kind; 18] = [
        Kind::Process,
        Kind::Job,
        Kind::Endpoint,
        Kind::Service,
        Kind::Grant,
        Kind::Sched,
        Kind::Model,
        Kind::Component,
        Kind::Store,
        Kind::Build,
        Kind::NetDest,
        Kind::Shm,
        Kind::Partition,
        Kind::Dir,
        Kind::File,
        Kind::Display,
        Kind::Input,
        Kind::Update,
    ];

    pub fn from_raw(raw: u32) -> Option<Kind> {
        Kind::ALL.into_iter().find(|kind| *kind as u32 == raw)
    }

    pub const fn name(self) -> &'static str {
        match self {
            Kind::Process => "process",
            Kind::Job => "job",
            Kind::Endpoint => "endpoint",
            Kind::Service => "service",
            Kind::Grant => "grant",
            Kind::Sched => "sched",
            Kind::Model => "model",
            Kind::Component => "component",
            Kind::Store => "store",
            Kind::Build => "build",
            Kind::NetDest => "netdest",
            Kind::Shm => "shm",
            Kind::Partition => "partition",
            Kind::Dir => "dir",
            Kind::File => "file",
            Kind::Display => "display",
            Kind::Input => "input",
            Kind::Update => "update",
        }
    }
}

bitflags! {
    /// What a handle allows (ADR 0001 D1), with Zircon's names where one exists.
    /// There is no EXECUTE right. The bits are ABI.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
    pub struct Rights: u32 {
        const DUPLICATE = 1 << 0;
        const TRANSFER = 1 << 1;
        const INSPECT = 1 << 2;
        const WAIT = 1 << 3;
        const READ = 1 << 4;
        const WRITE = 1 << 5;
        const MAP = 1 << 6;
        const ENUMERATE = 1 << 7;
        /// On a Job that names one program: start it ([`crate::call::JOB_SPAWN`]).
        const CREATE = 1 << 8;
        const DELETE = 1 << 9;
        const CALL = 1 << 10;
        const RECV = 1 << 11;
        const MINT = 1 << 12;
        const REVOKE = 1 << 13;
        const SPAWN = 1 << 14;
        const CONNECT = 1 << 15;
        const RESOLVE = 1 << 16;
        const SET_POLICY = 1 << 17;
        const THREADS = 1 << 18;
        const USER = 1 << 19;
        /// On a Job: stop processes ([`crate::call::PROC_STOP`]).
        const STOP = 1 << 20;
    }
}

bitflags! {
    /// Flags of a table entry.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
    pub struct Flags: u32 {
        /// Consumed by its first use.
        const ONCE = 1 << 0;
    }
}

/// Filled in by [`crate::call::ABI_INFO`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct AbiInfo {
    pub major: u32,
    pub minor: u32,
    /// The highest call number this kernel serves.
    pub max_call: u32,
    /// The most entries a handle table holds.
    pub table_max: u32,
}

/// One handle as [`crate::call::HANDLE_INFO`] and [`crate::call::STARTUP`]
/// report it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct HandleInfo {
    pub handle: u32,
    /// A [`Kind`] number.
    pub kind: u32,
    /// [`Rights`] bits.
    pub rights: u32,
    /// [`Flags`] bits.
    pub flags: u32,
}

impl HandleInfo {
    pub fn kind(&self) -> Option<Kind> {
        Kind::from_raw(self.kind)
    }

    pub fn rights(&self) -> Rights {
        Rights::from_bits_truncate(self.rights)
    }
}

/// Most handles one message moves.
pub const MAX_TRANSFER: usize = 4;

/// Argument of [`crate::call::CALL_H`].
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct CallH {
    pub request_ptr: u64,
    pub request_len: u64,
    pub reply_ptr: u64,
    pub reply_cap: u64,
    pub timeout_ms: u64,
    /// Handles to move to the server: `handles_len` u32 values.
    pub handles_ptr: u64,
    pub handles_len: u64,
    /// Where the handles the reply carries go: room for `handles_out_cap` u32s.
    pub handles_out_ptr: u64,
    pub handles_out_cap: u64,
}

/// Filled in by [`crate::call::RECV_H`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct RecvInfo {
    /// The calling process, or 0 for the kernel.
    pub caller_pid: u64,
    /// The badge of the route the call came through; 0 from the kernel.
    pub badge: u64,
    /// The rights of that route.
    pub rights: u32,
    /// How many of `handles` the request carried.
    pub handle_count: u32,
    pub handles: [u32; MAX_TRANSFER],
}

/// Fixed network destinations (ADR 0001 D2 `NetDest`): the object a `NetDest`
/// handle names.
pub mod netdest {
    /// The LLM server named on the command line, `llm=`.
    pub const LLM: u64 = 1;
    /// The kernel's DNS resolver.
    pub const RESOLVER: u64 = 2;
}

const _: () = {
    assert!(TABLE_MAX == 1 << INDEX_BITS);
    assert!(core::mem::size_of::<AbiInfo>() == 16);
    assert!(core::mem::size_of::<HandleInfo>() == 16);
    assert!(core::mem::size_of::<CallH>() == 9 * 8);
    assert!(core::mem::size_of::<RecvInfo>() == 24 + 4 * MAX_TRANSFER);
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_handle_splits_into_index_and_generation() {
        let handle = Handle::new(4095, 7);
        assert_eq!((handle.index(), handle.generation()), (4095, 7));
        assert_eq!(Handle::new(0, 1).0, 1 << INDEX_BITS);
        assert_eq!(Handle::NONE.generation(), 0);
    }

    #[test]
    fn kinds_round_trip() {
        for kind in Kind::ALL {
            assert_eq!(Kind::from_raw(kind as u32), Some(kind));
        }
        assert_eq!(Kind::from_raw(0), None);
        assert_eq!(Kind::from_raw(20), None);
    }
}
