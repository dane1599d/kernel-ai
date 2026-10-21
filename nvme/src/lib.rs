//! NVMe logic that does not touch hardware, so it can be unit-tested on the host:
//! the controller registers, the 64-byte commands and 16-byte completions, the
//! queues' head, tail and phase bookkeeping, PRP lists, and the Identify data the
//! kernel reads. The kernel's driver (`kernel/src/nvme/`) does the register and
//! memory accesses; everything it decides comes from here.
//!
//! Written from the NVM Express Base Specification 2.0; no code was taken from
//! another driver.

#![no_std]

pub mod command;
pub mod identify;
pub mod prp;
pub mod queue;
pub mod regs;

pub use command::{Command, Completion};
pub use identify::{Controller, Namespace};
pub use prp::Prp;
pub use queue::{CompletionQueue, SubmissionQueue};

/// The memory page size the driver sets (CC.MPS = 0): 4 KiB.
pub const PAGE: u64 = 4096;
