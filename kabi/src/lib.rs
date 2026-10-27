//! The system-call interface between the kernel and ring-3 programs: call numbers,
//! error codes, capabilities and the structures passed by pointer. Shared by the
//! kernel and the `user` runtime so both sides agree on every number and layout.
//!
//! Calling convention: `syscall` with the number in RAX and arguments in RDI, RSI,
//! RDX, R10, R8 and R9. The result comes back in RAX: a value, or a negative
//! [`Error`] code. RCX and R11 are clobbered.

#![no_std]

extern crate alloc;

pub mod call;
pub mod cap;
pub mod device;
mod error;
pub mod fs;
pub mod handle;
pub mod input;
mod ipc;
pub mod layout;
pub mod live;
mod metrics;
pub mod model;
pub mod net;
pub mod part;
pub mod plan;
pub mod policy;
pub mod probe;
mod process;
pub mod sched;
pub mod thread;

pub use error::Error;
pub use ipc::{CallRequest, HttpRequest, MAX_MESSAGE};
pub use metrics::{MAX_METRIC_PROCESSES, Metrics, ProcessInfo};
pub use process::{
    ExitReason, ExitRecord, JobSpawn, Limits, MAX_ARGS, MAX_PLAN, MAX_STATE, SpawnRequest,
};

pub const MAX_NAME: usize = 24;

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
