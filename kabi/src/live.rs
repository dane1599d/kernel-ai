//! Live components (M9a): what passes between a component host and the kernel's
//! version store and ledger, and the contract every component version implements.
//!
//! A component is a plain WebAssembly module run on wasmi by `comphost`, a ring-3
//! process named after the component. The kernel never runs WebAssembly: it keeps
//! the versions (from boot modules), and a ledger of which version is active, with
//! that version's last state checkpoint, so a host that dies restarts with its data.
//!
//! # The contract, `kai-lifecycle-1`
//!
//! Written as WIT for reference; it is bound by hand as core WebAssembly, with `i32`
//! values unless noted and results packed as `i64` `(ptr << 32) | len`:
//!
//! ```text
//! interface lifecycle {
//!     // Memory for an input; the callee owns and frees it. 0 means no memory.
//!     kai-alloc: func(len: u32) -> u32;
//!     // The state schema this version reads and writes (at least 1).
//!     kai-schema: func() -> u32;
//!     // Start empty. 0 is success.
//!     kai-init: func() -> u32;
//!     // Start from `state`, a CBOR [schema, body] written by any version: this is
//!     // the migration. 0 is success, 1 an unsupported schema, 2 bad state.
//!     kai-import: func(state: list<u8>, from-schema: u32) -> u32;
//!     // The state as CBOR [schema, body]; to-schema 0 means its own. -1 when it
//!     // cannot write that schema. Read-only.
//!     kai-export: func(to-schema: s32) -> s64;
//!     // Answer one request. The reply stays valid until the next call.
//!     kai-handle: func(request: list<u8>) -> list<u8>;
//!     // 0 healthy, otherwise a reason above 0. Read-only.
//!     kai-health: func() -> u32;
//!     // 0 when the version's own checks pass. Read-only.
//!     kai-self-test: func() -> u32;
//! }
//! // The only import: one log line, cut to 160 bytes.
//! kai.log: func(text: list<u8>);
//! ```
//!
//! A component has no clock, no randomness and no access to other services, so
//! running a request on two versions (probation runs both) has no side effects.

/// Kinds of [`Event`] a host reports.
pub mod kind {
    /// The host started on the active version (from the checkpoint, or fresh).
    pub const RESTORED: u64 = 1;
    /// The active version's state changed.
    pub const CHECKPOINT: u64 = 2;
    /// A swap to a candidate version begins; carries the state at the update point.
    pub const BEGIN: u64 = 3;
    /// The candidate passed probation and is now the active version.
    pub const COMMIT: u64 = 4;
    /// The candidate was dropped; the active version stays.
    pub const ROLLBACK: u64 = 5;
    /// The active version failed on a request and was reloaded.
    pub const FAULT: u64 = 6;
    /// The checkpoint could not be restored; the component waits for `ctl reset`.
    pub const DISABLED: u64 = 7;
    /// The user chose to start a disabled component afresh.
    pub const RESET: u64 = 8;

    pub const fn name(kind: u64) -> &'static str {
        match kind {
            RESTORED => "restored",
            CHECKPOINT => "checkpoint",
            BEGIN => "begin",
            COMMIT => "commit",
            ROLLBACK => "rollback",
            FAULT => "fault",
            DISABLED => "disabled",
            RESET => "reset",
            _ => "unknown",
        }
    }
}

/// Why a swap rolled back or a version failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u64)]
pub enum Reason {
    /// The artifact's manifest is missing, malformed or for something else.
    Manifest = 1,
    /// The module did not validate or compile.
    Compile = 2,
    /// The module needs an import the host does not offer.
    Link = 3,
    /// A missing export, a wrong signature, or a result outside the rules.
    Abi = 4,
    /// The state could not be exported, or not in the schema claimed.
    Export = 5,
    /// The new version cannot read the old state and the old cannot write one it can.
    NoMigration = 6,
    /// The migration failed.
    Migrate = 7,
    SelfTest = 8,
    Health = 9,
    Trap = 10,
    /// It ran out of fuel: a loop that did not end.
    Fuel = 11,
    /// It asked for more memory than its limit.
    Memory = 12,
    /// A host function refused its arguments.
    Host = 13,
    /// The user rolled it back.
    Manual = 14,
    /// The host died during the swap.
    HostRestart = 15,
}

impl Reason {
    pub const ALL: [Reason; 15] = [
        Reason::Manifest,
        Reason::Compile,
        Reason::Link,
        Reason::Abi,
        Reason::Export,
        Reason::NoMigration,
        Reason::Migrate,
        Reason::SelfTest,
        Reason::Health,
        Reason::Trap,
        Reason::Fuel,
        Reason::Memory,
        Reason::Host,
        Reason::Manual,
        Reason::HostRestart,
    ];

    pub fn from_raw(raw: u64) -> Option<Self> {
        Self::ALL.into_iter().find(|reason| *reason as u64 == raw)
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Manifest => "manifest",
            Self::Compile => "compile",
            Self::Link => "link",
            Self::Abi => "abi",
            Self::Export => "export",
            Self::NoMigration => "no-migration",
            Self::Migrate => "migrate",
            Self::SelfTest => "self-test",
            Self::Health => "health",
            Self::Trap => "trap",
            Self::Fuel => "fuel",
            Self::Memory => "memory",
            Self::Host => "host",
            Self::Manual => "manual",
            Self::HostRestart => "host-restart",
        }
    }
}

/// [`Event`] flags.
pub mod flag {
    /// BEGIN: try a version that failed before.
    pub const FORCE: u64 = 1;
    /// ROLLBACK: both versions failed on the request, so the candidate is not blamed.
    pub const POISON: u64 = 2;
    /// FAULT: the active version was reloaded from the checkpoint.
    pub const RESTARTED: u64 = 4;
}

/// Ledger phases, as [`Restore::phase`].
pub mod phase {
    /// No version has run yet.
    pub const EMPTY: u64 = 0;
    pub const STEADY: u64 = 1;
    /// A swap was under way: the checkpoint still belongs to the active version.
    pub const SWAPPING: u64 = 2;
}

/// Longest version label, `[a-z0-9-]{1,16}`.
pub const MAX_LABEL: usize = 16;
/// Largest artifact (module, manifest and source together).
pub const MAX_ARTIFACT: usize = 1024 * 1024;
/// Largest state checkpoint.
pub const MAX_CHECKPOINT: usize = 64 * 1024;
/// Longest detail text on an event.
pub const MAX_DETAIL: usize = 96;
/// Most components the kernel keeps versions and a ledger for.
pub const MAX_COMPONENTS: usize = 8;
/// Most versions of one component.
pub const MAX_VERSIONS: usize = 16;
/// Custom section holding the manifest.
pub const MANIFEST_SECTION: &str = "kai.manifest";
/// Custom section holding the bundled source.
pub const SOURCE_SECTION: &str = "kai.source";
/// Requests starting with this are control requests, accepted only from the kernel.
pub const CONTROL_PREFIX: &str = "ctl ";

/// Argument of [`crate::call::COMP_EVENT`].
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct Event {
    /// A [`kind`].
    pub kind: u64,
    /// A [`Reason`] for ROLLBACK, FAULT and DISABLED; 0 otherwise.
    pub reason: u64,
    /// The version: RESTORED's active, BEGIN's, COMMIT's and ROLLBACK's candidate.
    pub label_ptr: u64,
    pub label_len: u64,
    /// The schema of `blob` (RESTORED, CHECKPOINT, BEGIN and COMMIT).
    pub schema: u64,
    /// Requests served by the candidate during probation (COMMIT, ROLLBACK).
    pub served: u64,
    /// Requests where the candidate answered `err` and the active version `ok`.
    pub regressions: u64,
    /// [`flag`] bits.
    pub flags: u64,
    /// State, CBOR `[schema, body]`, at most [`MAX_CHECKPOINT`].
    pub blob_ptr: u64,
    pub blob_len: u64,
    /// Text for the log, at most [`MAX_DETAIL`].
    pub detail_ptr: u64,
    pub detail_len: u64,
}

/// Filled in by [`crate::call::COMP_RESTORE`].
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct Restore {
    /// A [`phase`].
    pub phase: u64,
    /// Schema of the checkpoint.
    pub schema: u64,
    /// Commits so far.
    pub generation: u64,
    pub checkpoint_len: u64,
    /// 1 when the component is disabled.
    pub flags: u64,
    pub active_len: u64,
    pub candidate_len: u64,
    pub active: [u8; MAX_LABEL],
    pub candidate: [u8; MAX_LABEL],
}

impl Restore {
    pub const DISABLED: u64 = 1;

    pub const fn empty() -> Self {
        Self {
            phase: phase::EMPTY,
            schema: 0,
            generation: 0,
            checkpoint_len: 0,
            flags: 0,
            active_len: 0,
            candidate_len: 0,
            active: [0; MAX_LABEL],
            candidate: [0; MAX_LABEL],
        }
    }

    pub fn active(&self) -> &str {
        label_text(&self.active, self.active_len)
    }

    pub fn candidate(&self) -> &str {
        label_text(&self.candidate, self.candidate_len)
    }
}

fn label_text(bytes: &[u8; MAX_LABEL], len: u64) -> &str {
    let len = (len as usize).min(MAX_LABEL);
    core::str::from_utf8(&bytes[..len]).unwrap_or("")
}

/// Whether `label` is a version label: 1 to 16 of `a-z`, `0-9` and `-`.
pub fn valid_label(label: &str) -> bool {
    (1..=MAX_LABEL).contains(&label.len())
        && label
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

const _: () = {
    assert!(core::mem::size_of::<Event>() == 12 * 8);
    assert!(core::mem::size_of::<Restore>() == 7 * 8 + 2 * MAX_LABEL);
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasons_round_trip_and_have_names() {
        for reason in Reason::ALL {
            assert_eq!(Reason::from_raw(reason as u64), Some(reason));
            assert!(!reason.name().is_empty());
        }
        assert_eq!(Reason::from_raw(0), None);
        assert_eq!(Reason::from_raw(16), None);
    }

    #[test]
    fn labels_are_short_lower_case_words() {
        for good in ["1", "2", "3-trap", "e12", "abcdefghijklmnop"] {
            assert!(valid_label(good), "{good}");
        }
        for bad in ["", "V2", "a b", "../x", "abcdefghijklmnopq", "x_y"] {
            assert!(!valid_label(bad), "{bad}");
        }
    }

    #[test]
    fn restore_labels_read_back() {
        let mut restore = Restore::empty();
        restore.active[..2].copy_from_slice(b"v2");
        restore.active_len = 2;
        assert_eq!(restore.active(), "v2");
        assert_eq!(restore.candidate(), "");
    }
}
