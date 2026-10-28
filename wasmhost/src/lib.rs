//! Runs component versions on wasmi as [`livecore::Instance`]s. Nothing else in
//! the system names a wasmi type.
//!
//! Each version gets its own engine, since a wasmi engine keeps compiled code until
//! it is dropped: dropping a version frees all of it. Modules are compiled eagerly
//! (lazy translation would charge the first call for compiling), start functions
//! and post-2.0 proposals are refused, and every call gets fresh fuel, so a loop
//! that never ends is a fault rather than a hang.
//!
//! Two hazards in dev builds, where ring-3 programs are built under `cargo xtask
//! test`: wasmi's default dispatch grows the native stack with every instruction
//! when debug assertions are on (hence `portable-dispatch`), and its host-function
//! glue crashes when compiled without optimisation (hence opt-level 2 for this
//! crate). This crate's API is therefore not generic and not inline, so no wasmi
//! generic is compiled in an unoptimised caller.

#![no_std]

extern crate alloc;

mod canonical;
pub mod desk;
#[cfg(test)]
mod desk_tests;
pub mod edit;
#[cfg(test)]
mod edit_tests;
mod effects;
pub mod grants;
mod instance;
pub mod linear;
pub mod link;
pub mod settings;
#[cfg(test)]
mod settings_tests;
pub mod surface;
#[cfg(test)]
mod surface_tests;
mod tables;
#[cfg(test)]
mod tests;
mod ui;
#[cfg(test)]
mod ui_tests;
#[cfg(test)]
mod ui_txn_tests;
mod uival;
#[cfg(test)]
mod user_tests;

use alloc::format;
use alloc::string::ToString;
pub use canonical::{CoreInstance, load_core, load_core_granted};
pub use edit::EditRequest;
pub use grants::Grants;
pub use instance::WasmInstance;
use livecore::{Fault, Reason};
pub use settings::{Kind as SettingsKind, SettingsRequest};
pub use tables::{EXPORTS, IMPORTS};
pub use ui::{ActRefused, AppEvent, MAX_UPDATES, Update};
use wasmi::{CompilationMode, Config, EnforcedLimits, Engine, Module};

/// Resources one instance may use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Fuel for one `kai_handle` call.
    pub fuel_call: u64,
    /// Fuel for any other call, and for instantiation.
    pub fuel_life: u64,
    /// Most linear memory, in bytes.
    pub memory_bytes: usize,
}

/// Where a version's log lines go; `None` drops them.
pub type LogSink = fn(&str);

/// Longest log line kept, in bytes.
pub const MAX_LOG_BYTES: usize = 160;
/// Most log lines one call may write.
pub const MAX_LOG_LINES: u32 = 8;

/// The engine settings every version is compiled with.
fn config() -> Config {
    let mut config = Config::default();
    config
        .consume_fuel(true)
        .compilation_mode(CompilationMode::Eager)
        .allow_start_fn(false)
        .ignore_custom_sections(true)
        .set_max_recursion_depth(1024)
        .set_max_cached_stacks(1)
        .enforced_limits(EnforcedLimits::strict())
        // What wasm32-unknown-unknown emits, and nothing newer.
        .wasm_mutable_global(true)
        .wasm_sign_extension(true)
        .wasm_saturating_float_to_int(true)
        .wasm_multi_value(true)
        .wasm_bulk_memory(true)
        .wasm_reference_types(true)
        .wasm_multi_memory(false)
        .wasm_tail_call(false)
        .wasm_extended_const(false)
        .wasm_custom_page_sizes(false)
        .wasm_wide_arithmetic(false);
    config
}

/// Compiles and instantiates one version. `log` receives its log lines.
#[inline(never)]
pub fn load(wasm: &[u8], limits: Limits, log: Option<LogSink>) -> Result<WasmInstance, Fault> {
    if wasm.len() > kabi::live::MAX_ARTIFACT {
        return Err(Fault::new(Reason::Compile, "the module is too large"));
    }
    let engine = Engine::new(&config());
    let module = Module::new(&engine, wasm)
        .map_err(|error| Fault::new(Reason::Compile, error.to_string()))?;
    WasmInstance::new(&engine, &module, limits, log)
}

/// A version as comphost runs it: `lifecycle 1` (hand-bound `kai-lifecycle-1`) or
/// `lifecycle 2` (`kai:live` through the canonical ABI), as its manifest says.
pub enum LiveInstance {
    V1(WasmInstance),
    V2(CoreInstance),
}

/// Loads an artifact by its manifest's `lifecycle`; a module with no manifest is
/// `lifecycle 1`. A `lifecycle 2` version gets its `kai.actions` rows, and does not
/// load when they do not parse.
#[inline(never)]
pub fn load_live(
    artifact: &[u8],
    limits: Limits,
    log: Option<LogSink>,
) -> Result<LiveInstance, Fault> {
    load_live_granted(artifact, limits, log, Grants::default())
}

/// As [`load_live`], for a version whose package holds `grants` (`kai:settings`,
/// `kai:edit`).
#[inline(never)]
pub fn load_live_granted(
    artifact: &[u8],
    limits: Limits,
    log: Option<LogSink>,
    grants: Grants,
) -> Result<LiveInstance, Fault> {
    let parsed = match livecore::artifact::parse(artifact) {
        Ok(parsed) => Some(parsed),
        // A manifest that is there must hold, or the version would load as
        // something it does not say it is.
        Err(why) if has_manifest(artifact) => return Err(Fault::new(Reason::Link, why)),
        Err(_) => None,
    };
    match parsed.as_ref().map_or(1, |a| a.manifest.lifecycle) {
        2 => {
            let rows = parsed.and_then(|a| a.actions).unwrap_or_default();
            let actions = livecore::actions::parse(rows)
                .map_err(|why| Fault::new(Reason::Link, format!("kai.actions: {why}")))?;
            let mut instance = load_core_granted(artifact, limits, log, grants)?;
            instance.set_actions(actions);
            Ok(LiveInstance::V2(instance))
        }
        _ => load(artifact, limits, log).map(LiveInstance::V1),
    }
}

fn has_manifest(artifact: &[u8]) -> bool {
    matches!(
        livecore::artifact::custom_section(artifact, kabi::live::MANIFEST_SECTION),
        Ok(Some(_))
    )
}

macro_rules! either {
    ($self:ident, $i:ident => $e:expr) => {
        match $self {
            LiveInstance::V1($i) => $e,
            LiveInstance::V2($i) => $e,
        }
    };
}

impl livecore::Instance for LiveInstance {
    fn schema(&mut self) -> Result<u32, Fault> {
        either!(self, i => i.schema())
    }
    fn init(&mut self) -> Result<u32, Fault> {
        either!(self, i => i.init())
    }
    fn import(&mut self, state: &[u8], from_schema: u32) -> Result<u32, Fault> {
        either!(self, i => i.import(state, from_schema))
    }
    fn export(&mut self, to_schema: u32) -> Result<Option<alloc::vec::Vec<u8>>, Fault> {
        either!(self, i => i.export(to_schema))
    }
    fn handle(&mut self, request: &[u8]) -> Result<alloc::vec::Vec<u8>, Fault> {
        either!(self, i => i.handle(request))
    }
    fn health(&mut self) -> Result<u32, Fault> {
        either!(self, i => i.health())
    }
    fn self_test(&mut self) -> Result<u32, Fault> {
        either!(self, i => i.self_test())
    }
    fn quiet(&mut self, quiet: bool) {
        either!(self, i => i.quiet(quiet))
    }
    fn call(
        &mut self,
        func: &appabi::Func,
        args: &[appabi::Val],
    ) -> Result<Option<appabi::Val>, Fault> {
        either!(self, i => i.call(func, args))
    }
    fn begin(&mut self, storage: &livecore::txn::Storage, inputs: livecore::txn::Inputs) {
        either!(self, i => i.begin(storage, inputs))
    }
    fn finish(&mut self) -> livecore::txn::Txn {
        either!(self, i => i.finish())
    }

    fn view(&mut self, storage: &livecore::txn::Storage) {
        either!(self, i => i.view(storage))
    }
}
