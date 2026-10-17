//! Live components, without system calls or WebAssembly: the artifact format, the
//! ledger rules the kernel enforces, and the component host's swap, probation and
//! rollback. The kernel uses the ledger, `comphost` uses everything over wasmi and
//! the COMP_* calls, and xtask runs the same host against a fake kernel.

#![no_std]

extern crate alloc;

pub mod artifact;
pub mod control;
pub mod diff;
pub mod host;
pub mod ledger;
pub mod local;
mod probation;
pub mod scenario;
pub mod settings;
pub mod state;
pub mod store;
mod swap;
#[cfg(test)]
mod tests;
pub mod uploads;

use alloc::string::String;
use alloc::vec::Vec;
pub use kabi::live::Reason;

/// How a version failed: why, and what the runtime said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fault {
    pub reason: Reason,
    pub detail: String,
}

impl Fault {
    pub fn new(reason: Reason, detail: impl Into<String>) -> Self {
        let mut detail: String = detail.into();
        detail.truncate(detail.floor_char_boundary(kabi::live::MAX_DETAIL));
        Self { reason, detail }
    }
}

/// One running version of a component: the `kai-lifecycle-1` exports. Results
/// other than faults are the raw codes the contract defines.
pub trait Instance {
    fn schema(&mut self) -> Result<u32, Fault>;
    /// 0 on success.
    fn init(&mut self) -> Result<u32, Fault>;
    /// 0 on success, 1 an unsupported schema, 2 bad state.
    fn import(&mut self, state: &[u8], from_schema: u32) -> Result<u32, Fault>;
    /// The state in `to_schema` (0 = its own), or `None` when it cannot write it.
    fn export(&mut self, to_schema: u32) -> Result<Option<Vec<u8>>, Fault>;
    fn handle(&mut self, request: &[u8]) -> Result<Vec<u8>, Fault>;
    /// 0 when healthy.
    fn health(&mut self) -> Result<u32, Fault>;
    /// 0 when its own checks pass.
    fn self_test(&mut self) -> Result<u32, Fault>;
    /// Drops (true) or keeps (false) its log lines; the standby on probation is quiet.
    fn quiet(&mut self, quiet: bool);
}
