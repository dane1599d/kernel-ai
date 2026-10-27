//! Live components, without system calls or WebAssembly: the artifact format, the
//! ledger rules the kernel enforces, and the component host's swap, probation and
//! rollback. The kernel uses the ledger, `comphost` uses everything over wasmi and
//! the COMP_* calls, and xtask runs the same host against a fake kernel.

#![no_std]

extern crate alloc;

pub mod actions;
pub mod artifact;
pub mod boot;
pub mod call;
pub mod cbor;
pub mod control;
pub mod diff;
pub mod host;
pub mod ledger;
pub mod local;
pub mod location;
pub mod manifest;
mod probation;
pub mod scenario;
pub mod settings;
mod start;
pub mod state;
pub mod store;
mod swap;
#[cfg(test)]
mod tests;
mod try_run;
pub mod txn;
mod undo;
pub mod uploads;
pub mod why;

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
    /// Calls the export `func` names with typed arguments (ADR 0003 section 2):
    /// `lifecycle 2` only; `handle` goes when `lifecycle 1` goes.
    fn call(
        &mut self,
        func: &appabi::Func,
        args: &[appabi::Val],
    ) -> Result<Option<appabi::Val>, Fault> {
        let _ = (func, args);
        Err(Fault::new(
            Reason::Abi,
            "a lifecycle 1 version has no typed calls",
        ))
    }
    /// Opens the next call's transaction: its storage reads see `storage`, and
    /// its clock, seed and first ticket are `inputs` (ADR 0003 section 4). A
    /// `lifecycle 1` version has no storage or effects.
    fn begin(&mut self, storage: &txn::Storage, inputs: txn::Inputs) {
        let _ = (storage, inputs);
    }
    /// Closes the transaction [`Instance::begin`] opened: the call's writes and
    /// effects, which the host commits or drops.
    fn finish(&mut self) -> txn::Txn {
        txn::Txn::default()
    }
    /// The storage its lifecycle calls (`init`, `migrate`) may read, as the host
    /// holds it when the instance is loaded: an app that keeps its data there
    /// can show it at once. Writes stay for action calls.
    fn view(&mut self, storage: &txn::Storage) {
        let _ = storage;
    }
}
